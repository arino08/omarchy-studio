//! Settings schema + apply pipeline (spec 01 §2–3).
//!
//! Every mutation flows: drift-check → snapshot(pre) → write(atomic, hash-
//! guarded) → reload → verify → snapshot(post) | rollback. Live preview
//! deliberately bypasses this and is reconciled on screen exit (save =
//! pipeline, cancel = `hyprctl reload`).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use crate::cmd::CommandRunner;
use crate::error::{Result, StudioError};
use crate::omarchy::{cmds, Component};
use crate::snapshot::{SnapshotId, SnapshotKind, SnapshotStore};

/// Schema-path id, e.g. `looknfeel.blur.size` (spec 01 §2).
pub type SettingId = String;

/// How risky an apply is — drives frontend confirmation UX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Instant + trivially reversible (palette tweak with live preview).
    Safe,
    /// Restarts a component (waybar, mako); brief visual blip.
    Restarty,
    /// Could degrade the session; arms the confirm-or-revert countdown.
    Risky,
}

/// One planned file mutation, produced by `configfs`.
#[derive(Debug, Clone)]
pub struct FileEdit {
    pub file: PathBuf,
    pub new_content: String,
    /// Hash of the content we parsed; the write refuses if the file changed
    /// on disk since then (spec 03 §5) — re-parse and re-plan instead.
    pub parsed_hash: u64,
}

impl FileEdit {
    /// Plan an edit of `file`, recording the content the plan was derived
    /// from. `old_content` is `None` for files being created.
    pub fn new(file: PathBuf, old_content: Option<&str>, new_content: String) -> Self {
        Self {
            file,
            new_content,
            parsed_hash: content_hash(old_content),
        }
    }
}

pub fn content_hash(content: Option<&str>) -> u64 {
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

/// Ordered post-write reload actions (executed via spec 02 §2 wrappers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadStep {
    HyprReload,
    ThemeRefresh,
    Restart(Component),
    MakoReload,
}

/// Post-reload checks; any failure triggers rollback (spec 01 §3).
#[derive(Debug, Clone)]
pub enum Verification {
    /// `hyprctl configerrors` must report nothing. [gate: has_configerrors]
    HyprctlConfigErrorsEmpty,
    /// Process must still be alive after the grace period (waybar watchdog).
    ProcessAlive { name: &'static str, grace: Duration },
    /// Arbitrary command must exit 0.
    CommandOk(crate::cmd::Cmd),
}

#[derive(Debug)]
pub struct ApplyPlan {
    /// Human summary, becomes the snapshot subject: `blur.size 8→12`.
    pub summary: String,
    /// Module id for snapshot attribution (`m4`, `themes`, …).
    pub module: String,
    pub edits: Vec<FileEdit>,
    pub reload: Vec<ReloadStep>,
    pub verify: Vec<Verification>,
    pub risk: Risk,
    /// Structured metadata recorded in the post snapshot trailer.
    pub trailers: Vec<(String, String)>,
}

/// What actually happened; frontends render this.
#[derive(Debug)]
pub struct ApplyReport {
    pub pre: SnapshotId,
    pub post: Option<SnapshotId>,
    pub files: Vec<PathBuf>,
    /// Files that carried hand-edits and were drift-snapshotted first.
    pub drifted: Vec<PathBuf>,
    pub dry_run: bool,
}

pub struct Pipeline<'a> {
    pub store: &'a SnapshotStore,
    pub runner: &'a dyn CommandRunner,
}

impl<'a> Pipeline<'a> {
    pub fn new(store: &'a SnapshotStore, runner: &'a dyn CommandRunner) -> Self {
        Self { store, runner }
    }

    /// Execute the plan. `dry_run` snapshots nothing, writes nothing, runs
    /// nothing — it only validates hash guards and reports what would happen.
    pub fn apply(&self, plan: &ApplyPlan, dry_run: bool) -> Result<ApplyReport> {
        let files: Vec<PathBuf> = plan.edits.iter().map(|e| e.file.clone()).collect();

        // Hash guard first — cheap, and identical for dry and real runs.
        for edit in &plan.edits {
            let on_disk = std::fs::read_to_string(&edit.file).ok();
            if content_hash(on_disk.as_deref()) != edit.parsed_hash {
                return Err(StudioError::ParseFailed {
                    file: edit.file.clone(),
                    line: None,
                    hint: "file changed on disk since it was read — reopen to re-plan".into(),
                });
            }
        }

        if dry_run {
            return Ok(ApplyReport {
                pre: String::new(),
                post: None,
                files,
                drifted: Vec::new(),
                dry_run: true,
            });
        }

        // Hand-edits since our last snapshot are preserved as history first.
        let drifted = self.store.detect_drift(&files);
        if !drifted.is_empty() {
            self.store.record(
                SnapshotKind::Drift,
                &format!("hand-edits before: {}", plan.summary),
                &drifted,
                &plan.module,
                &[],
            )?;
        }

        let pre = self
            .store
            .record(SnapshotKind::Pre, &plan.summary, &files, &plan.module, &[])?;

        // A snapshot can only restore files that existed when it was taken, so
        // "this file did not exist" has to be remembered separately — without
        // it a failed first-ever apply leaves the rejected file on disk while
        // reporting a successful rollback.
        let created: Vec<PathBuf> = plan
            .edits
            .iter()
            .filter(|e| !e.file.exists())
            .map(|e| e.file.clone())
            .collect();

        for edit in &plan.edits {
            crate::configfs::atomic_write(&edit.file, &edit.new_content)?;
        }

        if let Err(e) = self.reload_and_verify(plan) {
            // Roll back to the pre state and re-run reloads so the live
            // desktop matches the restored files again. Rollback failures
            // must be loud, never silent (spec 01 §3).
            let undone = self.store.restore(&pre).is_ok();
            let removed = created
                .iter()
                .all(|f| !f.exists() || std::fs::remove_file(f).is_ok());
            let rolled_back = undone && removed && self.run_reloads(plan).is_ok();
            let (step, stderr) = match e {
                StudioError::VerifyFailed { step, stderr, .. } => (step, stderr),
                other => ("reload".into(), format!("{other:?}")),
            };
            return Err(StudioError::VerifyFailed {
                step,
                stderr,
                rolled_back,
                snapshot_id: Some(pre),
            });
        }

        let post = self.store.record(
            SnapshotKind::Post,
            &plan.summary,
            &files,
            &plan.module,
            &plan.trailers,
        )?;

        Ok(ApplyReport {
            pre,
            post: Some(post),
            files,
            drifted,
            dry_run: false,
        })
    }

    fn reload_and_verify(&self, plan: &ApplyPlan) -> Result<()> {
        self.run_reloads(plan)?;
        for v in &plan.verify {
            self.check(v)?;
        }
        Ok(())
    }

    fn run_reloads(&self, plan: &ApplyPlan) -> Result<()> {
        for step in &plan.reload {
            let cmd = match step {
                ReloadStep::HyprReload => cmds::hypr_reload(),
                ReloadStep::ThemeRefresh => cmds::theme_refresh(),
                ReloadStep::Restart(c) => cmds::restart(*c),
                ReloadStep::MakoReload => cmds::makoctl_reload(),
            };
            let out = self.runner.run(&cmd)?;
            if !out.ok() {
                return Err(StudioError::VerifyFailed {
                    step: cmd.display(),
                    stderr: out.stderr,
                    rolled_back: false,
                    snapshot_id: None,
                });
            }
        }
        Ok(())
    }

    fn check(&self, v: &Verification) -> Result<()> {
        match v {
            Verification::HyprctlConfigErrorsEmpty => {
                let out = self.runner.run(&cmds::hypr_configerrors())?;
                let clean = out.ok()
                    && (out.stdout.trim().is_empty()
                        || out.stdout.to_lowercase().contains("no errors"));
                if !clean {
                    return Err(StudioError::VerifyFailed {
                        step: "hyprctl configerrors".into(),
                        stderr: out.stdout,
                        rolled_back: false,
                        snapshot_id: None,
                    });
                }
            }
            Verification::ProcessAlive { name, grace } => {
                std::thread::sleep(*grace);
                let out = self.runner.run(&cmds::process_alive(name))?;
                if !out.ok() {
                    return Err(StudioError::VerifyFailed {
                        step: format!("{name} alive after restart"),
                        stderr: format!("{name} is not running"),
                        rolled_back: false,
                        snapshot_id: None,
                    });
                }
            }
            Verification::CommandOk(cmd) => {
                let out = self.runner.run(cmd)?;
                if !out.ok() {
                    return Err(StudioError::VerifyFailed {
                        step: cmd.display(),
                        stderr: out.stderr,
                        rolled_back: false,
                        snapshot_id: None,
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{RealRunner, StubRunner};

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-engine-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(dir: &std::path::Path) -> SnapshotStore {
        // Snapshots use real git; the pipeline's own commands use the stub.
        SnapshotStore::open_or_init(dir.join("history"), Box::new(RealRunner)).unwrap()
    }

    fn plan_for(file: &std::path::Path, old: Option<&str>, new: &str) -> ApplyPlan {
        ApplyPlan {
            summary: "blur.size 8→12".into(),
            module: "m4".into(),
            edits: vec![FileEdit::new(file.to_path_buf(), old, new.to_string())],
            reload: vec![ReloadStep::HyprReload],
            verify: vec![Verification::HyprctlConfigErrorsEmpty],
            risk: Risk::Safe,
            trailers: vec![("Setting".into(), "blur.size".into())],
        }
    }

    #[test]
    fn happy_path_writes_reloads_verifies_snapshots() {
        let dir = scratch("happy");
        let s = store(&dir);
        let f = dir.join("looknfeel.conf");
        std::fs::write(&f, "blur = 8\n").unwrap();

        let stub = StubRunner::default()
            .with_ok("hyprctl reload", "")
            .with_ok("hyprctl configerrors", "no errors were found\n");
        let report = Pipeline::new(&s, &stub)
            .apply(&plan_for(&f, Some("blur = 8\n"), "blur = 12\n"), false)
            .unwrap();

        assert_eq!(std::fs::read_to_string(&f).unwrap(), "blur = 12\n");
        assert!(report.post.is_some());
        assert_eq!(stub.calls(), vec!["hyprctl reload", "hyprctl configerrors"]);
        // history: pre + post
        let kinds: Vec<_> = s.list(10).unwrap().iter().filter_map(|i| i.kind).collect();
        assert_eq!(kinds, vec![SnapshotKind::Post, SnapshotKind::Pre]);
    }

    #[test]
    fn refuses_when_file_changed_since_parse() {
        let dir = scratch("hash");
        let s = store(&dir);
        let f = dir.join("a.conf");
        std::fs::write(&f, "someone else edited this\n").unwrap();

        let stub = StubRunner::default();
        let err = Pipeline::new(&s, &stub)
            .apply(&plan_for(&f, Some("what we parsed\n"), "new\n"), false)
            .unwrap_err();
        match err {
            StudioError::ParseFailed { hint, .. } => assert!(hint.contains("changed on disk")),
            other => panic!("wrong variant: {other:?}"),
        }
        // nothing ran, nothing written
        assert!(stub.calls().is_empty());
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "someone else edited this\n"
        );
    }

    #[test]
    fn failed_verification_rolls_back_files_and_reloads() {
        let dir = scratch("rollback");
        let s = store(&dir);
        let f = dir.join("looknfeel.conf");
        std::fs::write(&f, "blur = 8\n").unwrap();

        let stub = StubRunner::default().with_ok("hyprctl reload", "").with_ok(
            "hyprctl configerrors",
            "error: invalid value at looknfeel.conf:1\n",
        );
        let err = Pipeline::new(&s, &stub)
            .apply(
                &plan_for(&f, Some("blur = 8\n"), "blur = nonsense\n"),
                false,
            )
            .unwrap_err();

        match err {
            StudioError::VerifyFailed {
                rolled_back,
                snapshot_id,
                ..
            } => {
                assert!(rolled_back);
                assert!(snapshot_id.is_some());
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // file restored, and hyprctl reload ran again to re-apply file truth
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "blur = 8\n");
        assert_eq!(
            stub.calls(),
            vec!["hyprctl reload", "hyprctl configerrors", "hyprctl reload"]
        );
    }

    #[test]
    fn a_failed_first_apply_removes_the_file_it_created() {
        let dir = scratch("rollback-created");
        let s = store(&dir);
        // The file does not exist yet — a snapshot can't record "absent", so
        // restoring the pre state alone would leave the rejected file behind
        // while we told the user their config was restored.
        let f = dir.join("looknfeel.conf");

        let stub = StubRunner::default().with_ok("hyprctl reload", "").with_ok(
            "hyprctl configerrors",
            "error: invalid value at looknfeel.conf:1\n",
        );
        let err = Pipeline::new(&s, &stub)
            .apply(&plan_for(&f, None, "gaps_in = 6\n"), false)
            .unwrap_err();

        match err {
            StudioError::VerifyFailed { rolled_back, .. } => assert!(rolled_back),
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(
            !f.exists(),
            "the rejected file should be gone, not left behind"
        );
    }

    #[test]
    fn hand_edits_are_drift_snapshotted_before_apply() {
        let dir = scratch("drift");
        let s = store(&dir);
        let f = dir.join("a.conf");

        // Studio knows the file as "v1" (a previous post), user edited to "v2".
        std::fs::write(&f, "v1\n").unwrap();
        s.record(
            SnapshotKind::Post,
            "earlier apply",
            std::slice::from_ref(&f),
            "m",
            &[],
        )
        .unwrap();
        std::fs::write(&f, "v2 hand edit\n").unwrap();

        let stub = StubRunner::default()
            .with_ok("hyprctl reload", "")
            .with_ok("hyprctl configerrors", "");
        let report = Pipeline::new(&s, &stub)
            .apply(&plan_for(&f, Some("v2 hand edit\n"), "v3\n"), false)
            .unwrap();

        assert_eq!(report.drifted, vec![f]);
        let kinds: Vec<_> = s.list(10).unwrap().iter().filter_map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SnapshotKind::Post,  // this apply
                SnapshotKind::Drift, // preserved hand-edit (pre deduped into it)
                SnapshotKind::Post,  // earlier apply
            ]
        );
    }

    #[test]
    fn dry_run_touches_nothing() {
        let dir = scratch("dry");
        let s = store(&dir);
        let f = dir.join("a.conf");
        std::fs::write(&f, "v1\n").unwrap();

        let stub = StubRunner::default();
        let report = Pipeline::new(&s, &stub)
            .apply(&plan_for(&f, Some("v1\n"), "v2\n"), true)
            .unwrap();

        assert!(report.dry_run);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v1\n");
        assert!(stub.calls().is_empty());
        assert!(s.list(10).unwrap().is_empty());
    }

    #[test]
    fn process_alive_failure_reports_the_process() {
        let dir = scratch("alive");
        let s = store(&dir);
        let f = dir.join("waybar.css");
        std::fs::write(&f, "old\n").unwrap();

        let stub = StubRunner::default()
            .with_ok("omarchy-restart-waybar", "")
            .with_fail("pgrep -x waybar");

        let plan = ApplyPlan {
            summary: "bar height".into(),
            module: "m5".into(),
            edits: vec![FileEdit::new(f.clone(), Some("old\n"), "new\n".into())],
            reload: vec![ReloadStep::Restart(Component::Waybar)],
            verify: vec![Verification::ProcessAlive {
                name: "waybar",
                grace: Duration::from_millis(1),
            }],
            risk: Risk::Restarty,
            trailers: vec![],
        };
        let err = Pipeline::new(&s, &stub).apply(&plan, false).unwrap_err();
        match err {
            StudioError::VerifyFailed {
                step, rolled_back, ..
            } => {
                assert!(step.contains("waybar"));
                assert!(rolled_back);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "old\n");
    }
}

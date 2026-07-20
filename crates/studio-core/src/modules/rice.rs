//! Rice migration bundle (ROADMAP 0.9.6): move a whole setup to another
//! machine, or back it up whole.
//!
//! A bundle is a tar.gz of three things: every config file Studio has ever
//! written (the snapshot store already knows exactly which, and which module
//! owns each), the user's own theme directories, and a `manifest.toml`
//! describing both. Nothing is guessed — a file Studio never wrote is not
//! Studio's to ship.
//!
//! Import is the half that matters. A blind `tar -x` over a live setup can
//! leave a desktop that won't come back; this replays every file through the
//! ordinary apply pipeline, so the target machine gets the same pre-snapshot,
//! drift detection, hash-guarded write, reload, verification and automatic
//! rollback as any other Studio change. Refusing to import is always an
//! option: nothing here is a point of no return.
//!
//! Two deliberate exclusions, both about what *doesn't* travel:
//!   * files outside `$HOME` (the power module's udev rule) — a user bundle
//!     never carries root-owned writes, so importing never needs sudo;
//!   * machine-specific files ([`Entry::machine`], e.g. `monitors.conf`,
//!     whose output names don't exist on the target) — carried in the bundle
//!     but skipped on import unless asked for.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cmd::{Cmd, CommandRunner};
use crate::engine::{ApplyPlan, ApplyReport, FileEdit, Pipeline, ReloadStep, Risk, Verification};
use crate::error::{Result, StudioError};
use crate::modules::themes::{ThemeStore, SCRATCH_SLUG};
use crate::omarchy::{Component, OmarchyPaths};
use crate::snapshot::SnapshotStore;

/// Bundle format version. Bumped when the layout changes incompatibly;
/// import refuses anything it doesn't understand rather than half-applying.
pub const FORMAT: u32 = 1;

const MANIFEST: &str = "manifest.toml";
/// Room for a big theme collection over a slow disk.
const TAR_TIMEOUT: Duration = Duration::from_secs(120);

/// One config file in a bundle, addressed relative to `$HOME` so it lands in
/// the right place on a machine with a different user name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to `$HOME`, e.g. `.config/hypr/looknfeel.conf`.
    pub home_rel: String,
    /// Module that wrote it, straight from the snapshot store's manifest.
    pub module: String,
    /// True when the file describes *this* machine and means nothing on
    /// another one. Skipped on import unless explicitly included.
    pub machine: bool,
}

impl Entry {
    fn path_in(&self, root: &Path) -> PathBuf {
        root.join("files").join(&self.home_rel)
    }
}

/// A bundle's `manifest.toml`, and the whole of what import reads before it
/// touches anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub format: u32,
    /// Studio version that wrote the bundle.
    pub studio_version: String,
    /// Omarchy version of the source machine — a warning on mismatch, never
    /// a block: the whole point is moving between installs.
    pub omarchy_version: String,
    /// ISO-8601 UTC, from `date -u`.
    pub created: String,
    pub entries: Vec<Entry>,
    /// Slugs of the user theme directories carried in `themes/`.
    pub themes: Vec<String>,
}

impl Bundle {
    /// Modules present, sorted — what `--only` can name.
    pub fn modules(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self.entries.iter().map(|e| e.module.as_str()).collect();
        set.into_iter().map(String::from).collect()
    }

    fn render(&self) -> String {
        let mut s = String::from("# omarchy-studio rice bundle\n");
        s.push_str(&format!("format = {}\n", self.format));
        s.push_str(&format!("studio_version = \"{}\"\n", self.studio_version));
        s.push_str(&format!("omarchy_version = \"{}\"\n", self.omarchy_version));
        s.push_str(&format!("created = \"{}\"\n", self.created));
        s.push_str(&format!(
            "themes = [{}]\n",
            self.themes
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for e in &self.entries {
            s.push_str("\n[[files]]\n");
            s.push_str(&format!("path = \"{}\"\n", e.home_rel));
            s.push_str(&format!("module = \"{}\"\n", e.module));
            s.push_str(&format!("machine = {}\n", e.machine));
        }
        s
    }

    fn parse(text: &str) -> Result<Self> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| StudioError::ParseFailed {
            file: PathBuf::from(MANIFEST),
            line: None,
            hint: format!("bundle manifest is not valid TOML: {e}"),
        })?;
        let bad = |hint: &str| StudioError::ParseFailed {
            file: PathBuf::from(MANIFEST),
            line: None,
            hint: hint.to_string(),
        };
        let str_of = |k: &str| -> String {
            doc.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let format = doc
            .get("format")
            .and_then(|v| v.as_integer())
            .ok_or_else(|| bad("manifest has no format — not a rice bundle"))?
            as u32;
        let themes = doc
            .get("themes")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut entries = Vec::new();
        if let Some(files) = doc.get("files").and_then(|v| v.as_array_of_tables()) {
            for t in files {
                let home_rel = t
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| bad("a [[files]] entry has no path"))?
                    .to_string();
                // A bundle is untrusted input: an entry that escapes $HOME
                // would let one write anywhere the user can.
                if !safe_rel(&home_rel) {
                    return Err(bad(&format!("unsafe path in bundle: {home_rel}")));
                }
                entries.push(Entry {
                    home_rel,
                    module: t
                        .get("module")
                        .and_then(|v| v.as_str())
                        .unwrap_or("other")
                        .to_string(),
                    machine: t.get("machine").and_then(|v| v.as_bool()).unwrap_or(false),
                });
            }
        }
        Ok(Bundle {
            format,
            studio_version: str_of("studio_version"),
            omarchy_version: str_of("omarchy_version"),
            created: str_of("created"),
            entries,
            themes,
        })
    }
}

/// Reject anything that isn't a plain relative path under `$HOME` — no
/// absolute paths, no `..`, no root escape. Applies to theme slugs too.
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// `$HOME`, for callers assembling the arguments below. The functions here
/// take the home directory explicitly rather than reading the environment,
/// so a test can point a whole export at a scratch tree.
pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| StudioError::External {
            cmd: "rice".into(),
            detail: "HOME is not set — can't tell which files are yours".into(),
        })
}

/// Files whose module makes them true of this machine only.
fn machine_specific(home_rel: &str, module: &str) -> bool {
    module == "monitors" || home_rel.ends_with("hypr/monitors.conf")
}

/// Does a tracked file belong in a rice at all? Being under `$HOME` isn't
/// enough — the store also remembers files Studio was pointed at from
/// elsewhere (a custom `[targets]` path under a scratch dir, anything a test
/// run touched), and those are nobody's rice. The roots a rice can draw
/// from are the config ones: `.config`, `.local/share`, and dotfiles sitting
/// directly in `$HOME` (themesync's `.bashrc` block).
fn portable(home_rel: &str) -> bool {
    let depth = Path::new(home_rel).components().count();
    depth == 1 || home_rel.starts_with(".config/") || home_rel.starts_with(".local/share/")
}

/// What an export would carry: every tracked file that still exists, sits in
/// a [`portable`] root under `$HOME`, and isn't Studio's own bookkeeping.
pub fn plan_export(
    store: &SnapshotStore,
    paths: &OmarchyPaths,
    home: &Path,
) -> Result<(Vec<Entry>, Vec<String>)> {
    let state = crate::studio_state_dir();
    let mut entries: Vec<Entry> = Vec::new();
    for (path, module) in store.tracked() {
        if !path.is_file() || path.starts_with(&state) {
            continue;
        }
        // Outside $HOME means root-owned (the udev rule) — never bundled.
        let Ok(rel) = path.strip_prefix(home) else {
            continue;
        };
        let home_rel = rel.to_string_lossy().to_string();
        if !safe_rel(&home_rel) || !portable(&home_rel) {
            continue;
        }
        entries.push(Entry {
            machine: machine_specific(&home_rel, &module),
            home_rel,
            module,
        });
    }
    entries.sort_by(|a, b| a.home_rel.cmp(&b.home_rel));
    entries.dedup_by(|a, b| a.home_rel == b.home_rel);

    let store = ThemeStore::from_paths(paths);
    let mut themes: Vec<String> = store
        .list()
        .into_iter()
        .filter(|t| t.user_dir.is_some() && t.slug != SCRATCH_SLUG && safe_rel(&t.slug))
        .map(|t| t.slug)
        .collect();
    themes.sort();
    Ok((entries, themes))
}

/// Write a bundle to `out`. Stages a tree under the cache dir, then hands it
/// to `tar` — the one archiver every Omarchy box already has.
pub fn export(
    store: &SnapshotStore,
    paths: &OmarchyPaths,
    home: &Path,
    out: &Path,
    runner: &dyn CommandRunner,
) -> Result<Bundle> {
    let (entries, themes) = plan_export(store, paths, home)?;
    // Unique per call, not just per process: two exports must never stage
    // into the same directory and delete each other's tree.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    // Stage beside the bundle we're writing, not in the shared cache dir.
    // The cache dir is process-wide, so exports from tests (or two users on
    // one machine) landed in the same place regardless of the paths they were
    // handed — which is also what made the export tests non-hermetic. Staging
    // here keeps it on the destination's filesystem too, so the tar reads
    // across no device boundary.
    let stage_root = out.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = stage_root {
        std::fs::create_dir_all(parent)?;
    }
    let stage = stage_root
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".rice-export-{}-{stamp}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;

    let bundle = Bundle {
        format: FORMAT,
        studio_version: crate::VERSION.to_string(),
        omarchy_version: paths.omarchy_version(),
        created: now_utc(runner),
        entries,
        themes,
    };

    for e in &bundle.entries {
        copy_into(&home.join(&e.home_rel), &e.path_in(&stage))?;
    }
    let theme_store = ThemeStore::from_paths(paths);
    for slug in &bundle.themes {
        let src = theme_store.user_dir.join(slug);
        copy_tree(&src, &stage.join("themes").join(slug))?;
    }
    std::fs::write(stage.join(MANIFEST), bundle.render())?;

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tar = Cmd::new("tar")
        .arg("-czf")
        .arg(out.display().to_string())
        .arg("-C")
        .arg(stage.display().to_string())
        .arg(".")
        .timeout(TAR_TIMEOUT);
    let res = runner.run(&tar);
    let _ = std::fs::remove_dir_all(&stage);
    let out_res = res?;
    if !out_res.ok() {
        return Err(StudioError::External {
            cmd: "tar -czf".into(),
            detail: out_res.stderr.trim().to_string(),
        });
    }
    Ok(bundle)
}

/// Unpack a bundle into `dest` and read its manifest. The caller owns
/// `dest` (a temp dir) and is responsible for cleaning it up.
pub fn unpack(bundle: &Path, dest: &Path, runner: &dyn CommandRunner) -> Result<Bundle> {
    if !bundle.is_file() {
        return Err(StudioError::External {
            cmd: "rice import".into(),
            detail: format!("no such bundle: {}", bundle.display()),
        });
    }
    std::fs::create_dir_all(dest)?;
    let tar = Cmd::new("tar")
        .arg("-xzf")
        .arg(bundle.display().to_string())
        .arg("-C")
        .arg(dest.display().to_string())
        .timeout(TAR_TIMEOUT);
    let out = runner.run(&tar)?;
    if !out.ok() {
        return Err(StudioError::External {
            cmd: "tar -xzf".into(),
            detail: out.stderr.trim().to_string(),
        });
    }
    let text = std::fs::read_to_string(dest.join(MANIFEST)).map_err(|_| StudioError::External {
        cmd: "rice import".into(),
        detail: "no manifest.toml inside — this isn't a Studio rice bundle".into(),
    })?;
    let b = Bundle::parse(&text)?;
    if b.format != FORMAT {
        return Err(StudioError::External {
            cmd: "rice import".into(),
            detail: format!(
                "bundle format {} — this Studio understands {FORMAT}; upgrade to import it",
                b.format
            ),
        });
    }
    Ok(b)
}

/// What an import would do, decided before anything is written.
#[derive(Debug, Default)]
pub struct ImportPlan {
    /// Files that would change, as pipeline edits.
    pub edits: Vec<FileEdit>,
    /// Already byte-identical — nothing to do.
    pub unchanged: Vec<String>,
    /// Deliberately left out, with the reason (`monitors.conf: describes
    /// the source machine's displays`).
    pub skipped: Vec<(String, String)>,
    /// Theme slugs that would be added.
    pub themes_new: Vec<String>,
    /// Theme slugs already on this machine — never overwritten.
    pub themes_existing: Vec<String>,
    /// Source Omarchy differs from ours — surfaced, never fatal.
    pub omarchy_mismatch: Option<String>,
}

/// Filters chosen by the caller.
#[derive(Debug, Default, Clone)]
pub struct ImportFilter {
    /// Restrict to these modules (empty = every module in the bundle).
    pub only: Vec<String>,
    /// Include the machine-specific files too.
    pub with_machine: bool,
    /// Include the bundle's themes.
    pub themes: bool,
}

/// Decide what to import. Reads the unpacked tree and the current on-disk
/// state; writes nothing.
pub fn plan_import(
    b: &Bundle,
    root: &Path,
    paths: &OmarchyPaths,
    home: &Path,
    filter: &ImportFilter,
) -> Result<ImportPlan> {
    let mut plan = ImportPlan::default();

    let ours = paths.omarchy_version();
    if !b.omarchy_version.is_empty() && !ours.is_empty() && b.omarchy_version != ours {
        plan.omarchy_mismatch = Some(format!(
            "bundle came from Omarchy {}, this machine runs {ours}",
            b.omarchy_version
        ));
    }

    for e in &b.entries {
        if !filter.only.is_empty() && !filter.only.contains(&e.module) {
            plan.skipped.push((
                e.home_rel.clone(),
                format!("module {} not selected", e.module),
            ));
            continue;
        }
        if e.machine && !filter.with_machine {
            plan.skipped.push((
                e.home_rel.clone(),
                "describes the source machine's hardware".into(),
            ));
            continue;
        }
        let src = e.path_in(root);
        let Ok(new) = std::fs::read_to_string(&src) else {
            plan.skipped.push((
                e.home_rel.clone(),
                "listed in the manifest but not in the bundle".into(),
            ));
            continue;
        };
        let dest = home.join(&e.home_rel);
        let old = std::fs::read_to_string(&dest).ok();
        if old.as_deref() == Some(new.as_str()) {
            plan.unchanged.push(e.home_rel.clone());
            continue;
        }
        plan.edits.push(FileEdit::new(dest, old.as_deref(), new));
    }

    if filter.themes {
        let store = ThemeStore::from_paths(paths);
        for slug in &b.themes {
            if !safe_rel(slug) || !root.join("themes").join(slug).is_dir() {
                continue;
            }
            if store.user_dir.join(slug).exists() {
                plan.themes_existing.push(slug.clone());
            } else {
                plan.themes_new.push(slug.clone());
            }
        }
    }
    Ok(plan)
}

/// Replay a planned import: every file through the apply pipeline (pre
/// snapshot, drift check, hash-guarded write, reload, verify, rollback on
/// failure), then the new themes copied in. Themes are additive and never
/// clobber a slug that already exists, so they stay outside the pipeline.
pub fn import(
    plan: &ImportPlan,
    paths: &OmarchyPaths,
    root: &Path,
    store: &SnapshotStore,
    runner: &dyn CommandRunner,
    dry_run: bool,
) -> Result<ApplyReport> {
    let files = plan.edits.len();
    let apply = ApplyPlan {
        summary: format!(
            "import rice bundle: {files} file{}, {} theme{}",
            if files == 1 { "" } else { "s" },
            plan.themes_new.len(),
            if plan.themes_new.len() == 1 { "" } else { "s" }
        ),
        module: "rice".into(),
        edits: plan.edits.clone(),
        reload: vec![
            ReloadStep::HyprReload,
            ReloadStep::Restart(Component::Waybar),
            ReloadStep::MakoReload,
        ],
        verify: vec![Verification::HyprctlConfigErrorsEmpty],
        risk: Risk::Risky,
        trailers: vec![("Rice-Files".into(), files.to_string())],
    };
    let report = Pipeline::new(store, runner).apply(&apply, dry_run)?;

    if !dry_run {
        let theme_store = ThemeStore::from_paths(paths);
        for slug in &plan.themes_new {
            copy_tree(
                &root.join("themes").join(slug),
                &theme_store.user_dir.join(slug),
            )?;
        }
    }
    Ok(report)
}

fn now_utc(runner: &dyn CommandRunner) -> String {
    runner
        .run(&Cmd::new("date").arg("-u").arg("+%Y-%m-%dT%H:%M:%SZ"))
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default()
}

fn copy_into(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    Ok(())
}

/// Recursive copy, skipping dot-entries (a theme dir cloned by
/// `omarchy-theme-install` carries a whole `.git`).
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::RealRunner;
    use crate::snapshot::SnapshotKind;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-rice-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(p: &Path, content: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// A scratch home with an Omarchy tree, plus a store that has tracked
    /// two config files: one portable, one machine-specific.
    fn rig(tag: &str) -> (PathBuf, OmarchyPaths, SnapshotStore) {
        let home = scratch(tag);
        let paths = OmarchyPaths {
            system: home.join("sys/omarchy"),
            config: home.join(".config/omarchy"),
            state: home.join(".local/state/omarchy"),
        };
        write(&paths.system.join("version"), "3.8.2\n");
        write(
            &home.join(".config/hypr/looknfeel.conf"),
            "# gaps\ngaps_in = 4\n",
        );
        write(
            &home.join(".config/hypr/monitors.conf"),
            "monitor=eDP-1,preferred,auto,1\n",
        );
        write(
            &paths.config.join("themes/my-rice/colors.toml"),
            "background = \"#0e1512\"\n",
        );
        // a .git the copy must not carry along
        write(&paths.config.join("themes/my-rice/.git/config"), "[core]\n");

        let store =
            SnapshotStore::open_or_init(home.join("history"), Box::new(RealRunner)).unwrap();
        store
            .record(
                SnapshotKind::Post,
                "seed",
                &[home.join(".config/hypr/looknfeel.conf")],
                "looknfeel",
                &[],
            )
            .unwrap();
        store
            .record(
                SnapshotKind::Post,
                "seed",
                &[home.join(".config/hypr/monitors.conf")],
                "monitors",
                &[],
            )
            .unwrap();
        (home, paths, store)
    }

    #[test]
    fn export_carries_tracked_files_and_user_themes_but_not_dotgit() {
        let (home, paths, store) = rig("export");
        let out = home.join("rice.tar.gz");
        let b = export(&store, &paths, &home, &out, &RealRunner).expect("export writes a bundle");

        assert_eq!(b.omarchy_version, "3.8.2");
        assert_eq!(b.themes, ["my-rice"]);
        assert_eq!(b.modules(), ["looknfeel", "monitors"]);
        assert!(
            b.entries
                .iter()
                .any(|e| e.home_rel.ends_with("monitors.conf") && e.machine),
            "monitors.conf travels, flagged machine-specific"
        );
        assert!(out.is_file(), "the tarball exists");

        let back = scratch("export-unpack");
        let read = unpack(&out, &back, &RealRunner).expect("our own bundle unpacks");
        assert_eq!(read, b, "manifest survives the round trip");
        assert!(back.join("files/.config/hypr/looknfeel.conf").is_file());
        assert!(back.join("themes/my-rice/colors.toml").is_file());
        assert!(
            !back.join("themes/my-rice/.git").exists(),
            "a theme's clone metadata is not part of the rice"
        );
    }

    #[test]
    fn import_skips_machine_files_and_leaves_existing_themes_alone() {
        let (src_home, src_paths, store) = rig("import-src");
        let out = src_home.join("rice.tar.gz");
        export(&store, &src_paths, &src_home, &out, &RealRunner).unwrap();

        // A different machine: no configs, but the same theme slug already
        // present with content of its own.
        let (dst_home, dst_paths, _) = rig("import-dst");
        std::fs::remove_file(dst_home.join(".config/hypr/looknfeel.conf")).unwrap();
        // A different machine really does have different displays.
        write(
            &dst_home.join(".config/hypr/monitors.conf"),
            "monitor=DP-1,3440x1440@100,0x0,1\n",
        );
        write(
            &dst_paths.config.join("themes/my-rice/colors.toml"),
            "background = \"#ffffff\"\n",
        );

        let root = scratch("import-unpack");
        let b = unpack(&out, &root, &RealRunner).unwrap();
        let filter = ImportFilter {
            themes: true,
            ..Default::default()
        };
        let plan = plan_import(&b, &root, &dst_paths, &dst_home, &filter).unwrap();

        assert_eq!(plan.edits.len(), 1, "only the portable file is an edit");
        assert!(plan.edits[0].file.ends_with("looknfeel.conf"));
        assert!(
            plan.skipped
                .iter()
                .any(|(f, _)| f.ends_with("monitors.conf")),
            "monitors.conf is skipped by default"
        );
        assert_eq!(plan.themes_existing, ["my-rice"], "never clobbered");
        assert!(plan.themes_new.is_empty());
        assert!(plan.omarchy_mismatch.is_none(), "same Omarchy version");

        // Asking for it explicitly brings the machine file back in.
        let all = ImportFilter {
            with_machine: true,
            ..Default::default()
        };
        let plan = plan_import(&b, &root, &dst_paths, &dst_home, &all).unwrap();
        assert_eq!(plan.edits.len(), 2);
    }

    #[test]
    fn import_replays_through_the_pipeline_and_dry_run_writes_nothing() {
        let (src_home, src_paths, store) = rig("replay-src");
        let out = src_home.join("rice.tar.gz");
        export(&store, &src_paths, &src_home, &out, &RealRunner).unwrap();

        let (dst_home, dst_paths, dst_store) = rig("replay-dst");
        let dest = dst_home.join(".config/hypr/looknfeel.conf");
        write(&dest, "# theirs\ngaps_in = 99\n");
        let root = scratch("replay-unpack");
        let b = unpack(&out, &root, &RealRunner).unwrap();
        // No reloads or verification off a scratch tree — this asserts the
        // file half of the replay, which is what a headless test can see.
        let plan = ImportPlan {
            edits: plan_import(&b, &root, &dst_paths, &dst_home, &ImportFilter::default())
                .unwrap()
                .edits,
            ..Default::default()
        };

        let apply = ApplyPlan {
            summary: "import".into(),
            module: "rice".into(),
            edits: plan.edits.clone(),
            reload: vec![],
            verify: vec![],
            risk: Risk::Risky,
            trailers: vec![],
        };
        let report = Pipeline::new(&dst_store, &RealRunner)
            .apply(&apply, true)
            .expect("dry run validates");
        assert!(report.dry_run);
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "# theirs\ngaps_in = 99\n",
            "a dry run leaves the target untouched"
        );

        let report = Pipeline::new(&dst_store, &RealRunner)
            .apply(&apply, false)
            .expect("real apply lands");
        assert!(report.post.is_some(), "the import is snapshotted");
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "# gaps\ngaps_in = 4\n",
            "the bundle's content won"
        );
    }

    fn bundle() -> Bundle {
        Bundle {
            format: FORMAT,
            studio_version: "0.9.0".into(),
            omarchy_version: "3.8.2".into(),
            created: "2026-07-17T10:00:00Z".into(),
            entries: vec![
                Entry {
                    home_rel: ".config/hypr/looknfeel.conf".into(),
                    module: "looknfeel".into(),
                    machine: false,
                },
                Entry {
                    home_rel: ".config/hypr/monitors.conf".into(),
                    module: "monitors".into(),
                    machine: true,
                },
                Entry {
                    home_rel: ".config/waybar/config.jsonc".into(),
                    module: "waybar".into(),
                    machine: false,
                },
            ],
            themes: vec!["my-rice".into()],
        }
    }

    #[test]
    fn manifest_round_trips() {
        let b = bundle();
        let parsed = Bundle::parse(&b.render()).expect("our own manifest parses");
        assert_eq!(parsed, b);
        assert_eq!(parsed.modules(), ["looknfeel", "monitors", "waybar"]);
    }

    #[test]
    fn a_manifest_that_isnt_ours_is_rejected_not_guessed() {
        assert!(Bundle::parse("format = = =").is_err(), "invalid TOML");
        assert!(
            Bundle::parse("name = \"some other archive\"").is_err(),
            "no format key — not a rice bundle"
        );
    }

    #[test]
    fn path_traversal_in_a_bundle_is_refused() {
        for evil in ["../../.ssh/authorized_keys", "/etc/passwd", ".."] {
            let text = format!("format = 1\n\n[[files]]\npath = \"{evil}\"\nmodule = \"x\"\n");
            assert!(
                Bundle::parse(&text).is_err(),
                "{evil} must not be importable"
            );
        }
        assert!(safe_rel(".config/hypr/looknfeel.conf"));
    }

    #[test]
    fn only_config_roots_are_rice_material() {
        assert!(portable(".config/hypr/looknfeel.conf"));
        assert!(portable(".local/share/applications/x.desktop"));
        assert!(portable(".bashrc"), "themesync's block travels");
        // A custom [targets] path, or anything a test run pointed us at,
        // is not part of anyone's rice.
        assert!(!portable(
            ".claude/jobs/78313bf8/tmp/wbtest/.config/waybar/config.jsonc"
        ));
        assert!(!portable("Projects/scratch/waybar/config.jsonc"));
    }

    #[test]
    fn export_leaves_out_files_tracked_from_scratch_dirs() {
        let (home, paths, store) = rig("scope");
        let stray = home.join("scratch/waybar/config.jsonc");
        write(&stray, "{}\n");
        store
            .record(SnapshotKind::Post, "seed", &[stray], "waybar", &[])
            .unwrap();

        let (entries, _) = plan_export(&store, &paths, &home).unwrap();
        assert!(
            entries.iter().all(|e| !e.home_rel.contains("scratch")),
            "a tracked file outside the config roots is not rice material"
        );
        assert!(entries
            .iter()
            .any(|e| e.home_rel.ends_with("looknfeel.conf")));
    }

    #[test]
    fn monitors_are_machine_specific_by_module_or_by_path() {
        assert!(machine_specific(".config/hypr/monitors.conf", "monitors"));
        assert!(machine_specific(".config/hypr/monitors.conf", "other"));
        assert!(!machine_specific(
            ".config/hypr/looknfeel.conf",
            "looknfeel"
        ));
    }
}

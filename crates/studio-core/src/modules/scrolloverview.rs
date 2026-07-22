//! Niri-style scrolling overview via the ScrollOverview Hyprland plugin.
//!
//! **Credit:** [ScrollOverview](https://github.com/yayuuu/hyprland-scroll-overview)
//! is written by **yayuuu** and licensed BSD-3-Clause. Studio does not vendor,
//! modify or redistribute any of its code — it drives `hyprpm`, the official
//! Hyprland plugin manager, exactly as the plugin's own README instructs, and
//! writes the config block and keybind the plugin documents. All credit for the
//! overview itself belongs to its author.
//!
//! Hyprland ships a native `scrolling` layout, but it is not the same thing and
//! (per the reference machine) not yet a comfortable niri substitute; this
//! plugin is what actually delivers the niri-like overview, so "niri mode" here
//! means *this plugin enabled*, not `general:layout = scrolling`.
//!
//! Everything is gated on `hyprpm` being present, and every install step is a
//! command the user could have run themselves — nothing is vendored or patched.

use std::path::PathBuf;
use std::time::Duration;

use crate::cmd::{find_in_path, Cmd, CommandRunner};
use crate::configfs::{CommentStyle, ManagedBlock};
use crate::error::Result;
use crate::omarchy::OmarchyPaths;

/// The upstream repository — what `hyprpm add` is pointed at.
pub const REPO: &str = "https://github.com/yayuuu/hyprland-scroll-overview.git";
/// Plugin name as `hyprpm` and Hyprland know it.
pub const PLUGIN: &str = "scrolloverview";
pub const AUTHOR: &str = "yayuuu";
pub const LICENSE: &str = "BSD-3-Clause";

/// Building a Hyprland plugin compiles C++ against the compositor headers.
const BUILD_TIMEOUT: Duration = Duration::from_secs(900);
const QUICK: Duration = Duration::from_secs(20);

/// Where the plugin's settings live — a Studio-managed block in the user's own
/// hypr config, never a vendored Omarchy file.
fn conf_path(paths: &OmarchyPaths) -> PathBuf {
    paths.hypr_config().join("scrolloverview.conf")
}

fn block() -> ManagedBlock {
    ManagedBlock::new("scrolloverview", CommentStyle::Hash)
}

/// How far along the install is. Drives what the UI offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// `hyprpm` isn't installed — nothing here can work.
    NoHyprpm,
    /// hyprpm is there, but the plugin repo hasn't been added.
    NotAdded,
    /// Added and built, but not loaded into Hyprland.
    Disabled,
    /// Loaded — niri mode is on.
    Enabled,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::NoHyprpm => "hyprpm not installed",
            State::NotAdded => "not installed",
            State::Disabled => "installed, off",
            State::Enabled => "on",
        }
    }

    /// Is the niri-style overview actually active right now?
    pub fn is_on(self) -> bool {
        self == State::Enabled
    }
}

/// Probe the current state from `hyprpm list`.
///
/// `hyprpm list` prints a repo/plugin tree with an `enabled: true|false` line
/// per plugin; a plugin that failed to build reports something else entirely,
/// which reads as "added but not enabled" — correct, since that's exactly what
/// the user needs to fix.
pub fn state(runner: &dyn CommandRunner) -> State {
    if find_in_path("hyprpm").is_none() {
        return State::NoHyprpm;
    }
    let Ok(out) = runner.run(&Cmd::new("hyprpm").arg("list").timeout(QUICK)) else {
        return State::NotAdded;
    };
    parse_state(&out.stdout)
}

/// Split out for testing: `hyprpm list` output → state.
fn parse_state(stdout: &str) -> State {
    // Strip ANSI colour so the `enabled:` value is readable.
    let plain = strip_ansi(stdout);
    // Lines are drawn as a tree (`  │ Plugin foo`, `  └─ enabled: true`), so
    // match on the keyword anywhere rather than at the start.
    let mut in_plugin = false;
    for line in plain.lines() {
        if let Some(rest) = line.split_once("Plugin ") {
            in_plugin = rest.1.trim() == PLUGIN;
            continue;
        }
        if in_plugin {
            if let Some(rest) = line.split_once("enabled:") {
                return if rest.1.trim().eq_ignore_ascii_case("true") {
                    State::Enabled
                } else {
                    State::Disabled
                };
            }
        }
    }
    State::NotAdded
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// `hyprpm add` — clones and builds the plugin. Slow (it compiles C++), and
/// needs `hyprpm update` to have installed matching headers first.
pub fn add(runner: &dyn CommandRunner) -> Result<String> {
    let out = runner.run(
        &Cmd::new("hyprpm")
            .args(["add", REPO])
            .timeout(BUILD_TIMEOUT),
    )?;
    if out.ok() {
        return Ok("plugin built".into());
    }
    Err(build_error(&out.stdout, &out.stderr))
}

/// Turn the overview on (`hyprpm enable`) or off (`hyprpm disable`).
pub fn set_enabled(runner: &dyn CommandRunner, on: bool) -> Result<String> {
    let verb = if on { "enable" } else { "disable" };
    let out = runner.run(&Cmd::new("hyprpm").args([verb, PLUGIN]).timeout(QUICK))?;
    if out.ok() {
        return Ok(if on {
            "niri mode on".into()
        } else {
            "back to plain Hyprland".into()
        });
    }
    Err(build_error(&out.stdout, &out.stderr))
}

/// hyprpm reports the actionable part on stdout as often as stderr, and its
/// two most common failures both have a specific fix worth naming.
fn build_error(stdout: &str, stderr: &str) -> crate::StudioError {
    let all = strip_ansi(&format!("{stdout}\n{stderr}"));
    let detail = if all.to_lowercase().contains("headers") {
        "Hyprland's plugin headers are missing or out of date. Run `hyprpm update` \
         in a terminal — it needs your password, so Studio can't run it for you."
            .to_string()
    } else if all.to_lowercase().contains("superuser") || all.contains("failed to create cache dir")
    {
        // hyprpm keeps its repos under /var/cache/hyprpm/<user>/, which is
        // root-owned, so adding a repo escalates. The build itself succeeds
        // first, which makes this look like a build failure when it isn't.
        "hyprpm needs your password to install the built plugin \
         (its cache lives under /var/cache). Run `hyprpm add` in a terminal — \
         Studio can't answer a sudo prompt."
            .to_string()
    } else {
        all.lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("hyprpm failed")
            .to_string()
    };
    crate::StudioError::External {
        cmd: "hyprpm".into(),
        detail,
    }
}

// ── settings ─────────────────────────────────────────────────────────────────

/// The plugin settings Studio exposes, with the plugin's own defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Overview scale, 0.1–0.9.
    pub scale: f64,
    /// `vertical` or `horizontal`.
    pub layout: String,
    /// Gap between workspace cards, px.
    pub workspace_gap: i64,
    /// Blur the overview backdrop.
    pub blur: bool,
    /// Trackpad swipe distance for the open/close gesture.
    pub gesture_distance: i64,
}

impl Default for Settings {
    fn default() -> Self {
        // Matches the plugin's documented defaults, except the gap: touching
        // cards read as one surface, and a small gap is what makes it look
        // like niri.
        Self {
            scale: 0.5,
            layout: "vertical".into(),
            workspace_gap: 100,
            blur: false,
            gesture_distance: 300,
        }
    }
}

impl Settings {
    /// Render the `plugin { scrolloverview { … } }` block body.
    pub fn render(&self) -> String {
        format!(
            "# Managed by Omarchy Studio — ScrollOverview by {AUTHOR} ({LICENSE}).\n\
             # Upstream: {REPO}\n\
             plugin {{\n    \
             {PLUGIN} {{\n        \
             scale = {:.2}\n        \
             layout = {}\n        \
             workspace_gap = {}\n        \
             blur = {}\n        \
             gesture_distance = {}\n    \
             }}\n\
             }}",
            self.scale, self.layout, self.workspace_gap, self.blur, self.gesture_distance,
        )
    }

    /// Read back what's in the managed block, falling back to defaults for
    /// anything absent so a partially hand-edited block still loads.
    pub fn load(paths: &OmarchyPaths) -> Self {
        let mut s = Self::default();
        let Ok(text) = std::fs::read_to_string(conf_path(paths)) else {
            return s;
        };
        let Some(body) = block().extract(&text) else {
            return s;
        };
        for line in body.lines() {
            let line = line.trim();
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "scale" => {
                    if let Ok(f) = v.parse() {
                        s.scale = f;
                    }
                }
                "layout" => s.layout = v.to_string(),
                "workspace_gap" => {
                    if let Ok(n) = v.parse() {
                        s.workspace_gap = n;
                    }
                }
                "blur" => s.blur = v == "true",
                "gesture_distance" => {
                    if let Ok(n) = v.parse() {
                        s.gesture_distance = n;
                    }
                }
                _ => {}
            }
        }
        s
    }

    /// Plan the settings file as a pipeline edit (E1), writing nothing.
    pub fn plan(&self, paths: &OmarchyPaths) -> Option<crate::engine::FileEdit> {
        let path = conf_path(paths);
        let on_disk = std::fs::read_to_string(&path).ok();
        let existing = on_disk.clone().unwrap_or_default();
        let updated = block().upsert(&existing, &self.render());
        (updated != existing)
            .then(|| crate::engine::FileEdit::new(path, on_disk.as_deref(), updated))
    }

    /// Write through the apply pipeline, so a block Hyprland refuses is rolled
    /// back like every other change Studio makes.
    pub fn apply(
        &self,
        paths: &OmarchyPaths,
        store: &crate::snapshot::SnapshotStore,
        runner: &dyn CommandRunner,
    ) -> Result<()> {
        let Some(edit) = self.plan(paths) else {
            return Ok(());
        };
        let plan = crate::engine::ApplyPlan {
            summary: "scroll overview settings".into(),
            module: PLUGIN.into(),
            edits: vec![edit],
            reload: vec![crate::engine::ReloadStep::HyprReload],
            verify: crate::engine::hypr_verification(runner),
            risk: crate::engine::Risk::Safe,
            trailers: Vec::new(),
        };
        crate::engine::Pipeline::new(store, runner).apply(&plan, false)?;
        Ok(())
    }
}

/// The line that has to be sourced for the settings to apply, and whether it
/// already is. Hyprland only reads files something `source =`s.
pub fn source_line(paths: &OmarchyPaths) -> String {
    format!("source = {}", conf_path(paths).display())
}

pub fn is_sourced(paths: &OmarchyPaths) -> bool {
    let needle = conf_path(paths).display().to_string();
    std::fs::read_to_string(paths.hypr_config().join("hyprland.conf"))
        .map(|c| c.contains(&needle))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &std::path::Path) -> OmarchyPaths {
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("sys/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "omarchy-studio-sco-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Real `hyprpm list` output, colour codes and all.
    const LIST: &str = "\u{1b}[0m→\u{1b}[0m Repository hyprland-plugins (by hyprwm):\n\
         \x20 │ Plugin hyprbars\n\
         \x20 └─ enabled: \u{1b}[31mfalse\n\
         \u{1b}[0m→\u{1b}[0m Repository hyprland-scroll-overview (by yayuuu):\n\
         \x20 │ Plugin scrolloverview\n\
         \x20 └─ enabled: \u{1b}[32mtrue\n";

    #[test]
    fn reads_enabled_out_of_real_hyprpm_output() {
        assert_eq!(parse_state(LIST), State::Enabled);
        assert!(parse_state(LIST).is_on());
    }

    #[test]
    fn a_disabled_plugin_is_installed_but_off() {
        let off = LIST.replace("\u{1b}[32mtrue", "\u{1b}[31mfalse");
        assert_eq!(parse_state(&off), State::Disabled);
    }

    #[test]
    fn another_repos_enabled_flag_is_not_ours() {
        // hyprbars is enabled, scrolloverview absent — must not read as ours.
        let other =
            "→ Repository hyprland-plugins (by hyprwm):\n  │ Plugin hyprbars\n  └─ enabled: true\n";
        assert_eq!(parse_state(other), State::NotAdded);
        assert_eq!(parse_state(""), State::NotAdded);
    }

    #[test]
    fn settings_round_trip_through_the_managed_block() {
        let dir = tmpdir("settings");
        let p = paths(&dir);
        let want = Settings {
            scale: 0.65,
            layout: "horizontal".into(),
            workspace_gap: 40,
            blur: true,
            gesture_distance: 250,
        };
        let edit = want.plan(&p).expect("a fresh file is a change");
        std::fs::write(&edit.file, &edit.new_content).unwrap();
        assert_eq!(Settings::load(&p), want);
        // Idempotent: writing the same settings again is not a change.
        assert!(want.plan(&p).is_none());
    }

    #[test]
    fn the_block_credits_the_plugin_author() {
        // The credit lives in the file the user actually reads, not just docs.
        let body = Settings::default().render();
        assert!(body.contains(AUTHOR), "{body}");
        assert!(body.contains(LICENSE), "{body}");
        assert!(body.contains(REPO), "{body}");
    }

    #[test]
    fn a_hand_edited_block_keeps_what_it_sets_and_defaults_the_rest() {
        let dir = tmpdir("partial");
        let p = paths(&dir);
        let path = conf_path(&p);
        std::fs::write(
            &path,
            block().upsert("", "plugin {\n  scrolloverview {\n    scale = 0.8\n  }\n}"),
        )
        .unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.scale, 0.8, "the value they set wins");
        assert_eq!(s.layout, "vertical", "the rest fall back to defaults");
    }

    #[test]
    fn a_root_owned_cache_says_to_run_it_in_a_terminal() {
        // The real message from the reference machine: the plugin builds, then
        // the install step can't write to /var/cache/hyprpm/<user>/.
        let e = build_error(
            "✔ built scrolloverview",
            "[ERR] addNewPluginRepo: failed to create cache dir",
        );
        let msg = format!("{e:?}");
        assert!(msg.contains("password"), "{msg}");
        assert!(msg.contains("terminal"), "{msg}");
    }

    #[test]
    fn header_failures_name_the_fix_rather_than_dumping_the_log() {
        let e = build_error("", "✖ Headers outdated, please run hyprpm update.");
        let msg = format!("{e:?}");
        assert!(msg.contains("hyprpm update"), "{msg}");
        // And it says why Studio can't just do it.
        assert!(msg.contains("password"), "{msg}");
    }
}

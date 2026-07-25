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

// ── scrolling-layout navigation ──────────────────────────────────────────────

/// The binds that make Hyprland's `scrolling` layout navigable.
///
/// Omarchy binds `SUPER+←/→` to `movefocus`, which **does nothing** in the
/// scrolling layout — verified on the reference machine: `movefocus r` left
/// focus where it was, `layoutmsg focus r` scrolled the tape. Studio's
/// override block is sourced last, so these win over the defaults while niri
/// mode is on, and removing them restores Omarchy's.
///
/// Each entry is (description, mods, key, dispatcher, arg).
pub fn nav_binds() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    vec![
        ("Scroll left", "SUPER", "LEFT", "layoutmsg", "focus l"),
        ("Scroll right", "SUPER", "RIGHT", "layoutmsg", "focus r"),
        (
            "Narrow column",
            "SUPER",
            "MINUS",
            "layoutmsg",
            "colresize -0.1",
        ),
        (
            "Widen column",
            "SUPER",
            "EQUAL",
            "layoutmsg",
            "colresize +0.1",
        ),
        (
            "Fit all columns",
            "SUPER SHIFT",
            "EQUAL",
            "layoutmsg",
            "fit all",
        ),
    ]
}

/// Are Studio's scrolling-navigation binds installed?
pub fn nav_binds_installed(paths: &OmarchyPaths) -> bool {
    let have = crate::modules::keybinds::read_overrides(paths);
    nav_binds().iter().all(|(desc, ..)| {
        have.iter().any(|o| {
            matches!(o, crate::modules::keybinds::Override::Set(cb)
                if cb.description.as_deref() == Some(*desc))
        })
    })
}

/// Install (or remove) the whole navigation set in one apply, so the keymap
/// never ends up half-converted.
pub fn set_nav_binds(
    paths: &OmarchyPaths,
    on: bool,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn CommandRunner,
) -> Result<usize> {
    use crate::modules::keybinds::{
        apply_overrides, mods_to_mask, read_overrides, ConfigBind, Override,
    };
    let ours: Vec<&str> = nav_binds().iter().map(|(d, ..)| *d).collect();
    // Drop any previous copy of ours first, so this is idempotent.
    let mut overrides: Vec<Override> = read_overrides(paths)
        .into_iter()
        .filter(|o| {
            !matches!(o, Override::Set(cb)
            if cb.description.as_deref().is_some_and(|d| ours.contains(&d)))
        })
        .collect();
    if on {
        for (desc, mods, key, dispatcher, arg) in nav_binds() {
            overrides.push(Override::Set(ConfigBind {
                flags: "bindd".into(),
                modmask: mods_to_mask(mods),
                key: key.to_string(),
                description: Some(desc.to_string()),
                dispatcher: dispatcher.to_string(),
                arg: arg.to_string(),
            }));
        }
    }
    let summary = if on {
        "niri navigation keybinds"
    } else {
        "remove niri navigation keybinds"
    };
    apply_overrides(paths, &overrides, store, runner, summary)?;
    Ok(if on { nav_binds().len() } else { 0 })
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

/// `~/.config/hypr/autostart.conf` — where the user's own `exec-once` lines
/// live, and thus where the plugin-autoload line belongs.
fn autostart_path(paths: &OmarchyPaths) -> PathBuf {
    paths.hypr_config().join("autostart.conf")
}

/// hyprpm plugins are built and enabled, but nothing loads them at boot — so
/// after a reboot the plugin is absent and every `scrolloverview:*` dispatcher
/// and its whole config block become "invalid", spraying config errors. This
/// `exec-once` runs `hyprpm reload -n` once at Hyprland startup, which is the
/// documented way to load enabled plugins.
const AUTOLOAD_LINE: &str = "exec-once = hyprpm reload -n";

fn autoload_block() -> ManagedBlock {
    ManagedBlock::new("plugin-autoload", CommentStyle::Hash)
}

/// Is the plugin-autoload line present?
pub fn autoloads(paths: &OmarchyPaths) -> bool {
    std::fs::read_to_string(autostart_path(paths))
        .map(|c| c.contains("hyprpm reload"))
        .unwrap_or(false)
}

/// Plan the autoload `exec-once` into autostart.conf.
fn plan_autoload(paths: &OmarchyPaths) -> Option<crate::engine::FileEdit> {
    let path = autostart_path(paths);
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    // Nothing to do if the user (or Omarchy) already loads plugins.
    if on_disk.contains("hyprpm reload") && !autoload_block().contains(&on_disk) {
        return None;
    }
    let updated = autoload_block().upsert(&on_disk, AUTOLOAD_LINE);
    (updated != on_disk).then(|| {
        crate::engine::FileEdit::new(
            path,
            std::fs::read_to_string(autostart_path(paths))
                .ok()
                .as_deref(),
            updated,
        )
    })
}

/// Plan the `source =` line into the user's `hyprland.conf`.
///
/// Hyprland only reads files something sources, so without this the settings
/// block is inert — it looks like Studio wrote them and nothing happened. The
/// line goes in its own managed block, appended last so it wins over anything
/// Omarchy sourced earlier.
pub fn plan_source(paths: &OmarchyPaths) -> Option<crate::engine::FileEdit> {
    let path = paths.hypr_config().join("hyprland.conf");
    let on_disk = std::fs::read_to_string(&path).ok()?;
    let src = ManagedBlock::new("scrolloverview-source", CommentStyle::Hash);
    let updated = src.upsert(&on_disk, &source_line(paths));
    (updated != on_disk)
        .then(|| crate::engine::FileEdit::new(path, Some(on_disk.as_str()), updated))
}

/// Ensure the settings file exists *and* is sourced, in one apply.
///
/// Both edits go in a single plan on purpose: Hyprland refuses to source a
/// file that doesn't exist ("source= globbing error: found no match"), so
/// writing the source line first fails verification and rolls back. Together
/// they either both land or neither does.
pub fn ensure_sourced(
    paths: &OmarchyPaths,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    let mut edits = Vec::new();
    // Seed the settings file first if it isn't there yet.
    if !conf_path(paths).exists() {
        edits.extend(Settings::load(paths).plan(paths));
    }
    edits.extend(plan_source(paths));
    // Also make the plugin load on boot — without this every reboot errors
    // until it's loaded by hand (exec-once can't create a file it references,
    // but hyprpm reload has no such dependency, so it's safe to add alone).
    edits.extend(plan_autoload(paths));
    if edits.is_empty() {
        return Ok(false);
    }
    let plan = crate::engine::ApplyPlan {
        summary: "source the scroll overview settings".into(),
        module: PLUGIN.into(),
        edits,
        reload: vec![crate::engine::ReloadStep::HyprReload],
        verify: crate::engine::hypr_verification(runner),
        risk: crate::engine::Risk::Risky,
        trailers: Vec::new(),
    };
    crate::engine::Pipeline::new(store, runner).apply(&plan, false)?;
    Ok(true)
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
    fn ensure_sourced_also_makes_the_plugin_load_on_boot() {
        // The reboot trap: enabled but not auto-loaded errors on every boot.
        let dir = tmpdir("autoload");
        let p = paths(&dir);
        std::fs::write(p.hypr_config().join("hyprland.conf"), "").unwrap();

        assert!(!autoloads(&p), "nothing loads plugins to begin with");
        // Apply just the autoload edit (source needs a runner; this is the
        // half that fixes the reboot errors).
        let edit = plan_autoload(&p).expect("a missing autoload line is a change");
        std::fs::write(&edit.file, &edit.new_content).unwrap();
        assert!(autoloads(&p), "now loads on boot");
        assert!(plan_autoload(&p).is_none(), "idempotent");
    }

    #[test]
    fn autoload_defers_to_an_existing_hyprpm_reload() {
        // If the user already loads plugins their own way, don't add a second.
        let dir = tmpdir("autoload-exists");
        let p = paths(&dir);
        std::fs::write(
            p.hypr_config().join("autostart.conf"),
            "exec-once = hyprpm reload -n  # mine\n",
        )
        .unwrap();
        assert!(autoloads(&p));
        assert!(plan_autoload(&p).is_none(), "their line is enough");
    }

    #[test]
    fn sourcing_is_idempotent_and_detected() {
        let dir = tmpdir("source");
        let p = paths(&dir);
        let hypr = p.hypr_config().join("hyprland.conf");
        std::fs::write(
            &hypr,
            "source = ~/.config/omarchy/current/theme/hyprland.conf\n",
        )
        .unwrap();

        assert!(!is_sourced(&p), "not sourced to begin with");
        let edit = plan_source(&p).expect("a missing source line is a change");
        std::fs::write(&edit.file, &edit.new_content).unwrap();
        // The settings file has to exist too, or Hyprland refuses the source.
        assert!(is_sourced(&p), "now sourced");
        // The user's own source lines survive, and re-running is a no-op.
        let after = std::fs::read_to_string(&hypr).unwrap();
        assert!(after.contains("current/theme/hyprland.conf"), "{after}");
        assert!(plan_source(&p).is_none(), "already sourced = no change");
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

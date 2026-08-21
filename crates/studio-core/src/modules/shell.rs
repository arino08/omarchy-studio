//! The Omarchy 4 "Quattro" shell (Quickshell) — bar layout, plugins, idle
//! (roadmap O4.5).
//!
//! Omarchy 4 dropped Waybar, Mako, SwayOSD and hypridle for one long-running
//! Quickshell process, `omarchy-shell`. Its bar, notifications, popups and
//! idle/lock timing all come from `~/.config/omarchy/shell.json` (layout +
//! idle + plugins) plus a themed `shell.toml` (fonts, spacing, per-surface
//! styling — not yet covered here, see ROADMAP O4.5).
//!
//! **Design choice: wrap, don't reinvent.** Omarchy ships sanctioned mutators
//! for the bar and its widgets — `omarchy bar move|put|set|position|
//! transparent|defaults` and `omarchy plugin enable|disable|list` — that
//! validate against the real widget catalog and reload the running shell
//! themselves. Studio wraps them (design pillar 1) rather than hand-editing
//! `shell.json`'s layout arrays the way it owns Waybar's `config.jsonc` —
//! there was no such command for the old Waybar, so Studio owned that model
//! outright; here it doesn't need to, and hand-editing would also have to
//! reimplement `omarchy-bar`'s own validation against the widget catalog.
//!
//! `idle.screensaver`/`idle.lock` have no dedicated command — Omarchy's own
//! docs say to set them directly in `shell.json` — so Studio edits those two
//! fields itself and reloads the shell explicitly.
//!
//! Reading is a plain `serde_json` round-trip with unknown fields preserved
//! (`#[serde(flatten)]`), the same pattern `modules::nova` uses for
//! `nova.json`: Studio doesn't need to understand every widget's settings to
//! carry them through untouched.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::cmd::CommandRunner;
use crate::configfs::atomic_write;
use crate::error::{Result, StudioError};
use crate::omarchy::{cmds, OmarchyPaths};

// ── shell.json model ────────────────────────────────────────────────────────

/// A bar lane. Order in `shell.json`'s `layout` object: left, center, right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Left,
    Center,
    Right,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Left, Section::Center, Section::Right];

    pub fn as_str(self) -> &'static str {
        match self {
            Section::Left => "left",
            Section::Center => "center",
            Section::Right => "right",
        }
    }

    pub fn parse(s: &str) -> Option<Section> {
        match s {
            "left" => Some(Section::Left),
            "center" => Some(Section::Center),
            "right" => Some(Section::Right),
            _ => None,
        }
    }
}

/// One entry in a bar lane: its widget id, plus whatever settings it carries
/// (`format`, `size`, `exec`, …). Never schema'd here — each widget defines
/// its own settings, and they round-trip verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarWidget {
    pub id: String,
    #[serde(flatten)]
    pub settings: Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BarLayout {
    pub left: Vec<BarWidget>,
    pub center: Vec<BarWidget>,
    pub right: Vec<BarWidget>,
}

impl BarLayout {
    pub fn section(&self, s: Section) -> &[BarWidget] {
        match s {
            Section::Left => &self.left,
            Section::Center => &self.center,
            Section::Right => &self.right,
        }
    }

    /// The lane holding `id`, if it's placed anywhere.
    pub fn find(&self, id: &str) -> Option<Section> {
        Section::ALL
            .into_iter()
            .find(|&s| self.section(s).iter().any(|w| w.id == id))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarSection {
    pub position: String,
    pub transparent: bool,
    #[serde(rename = "centerAnchor")]
    pub center_anchor: String,
    pub layout: BarLayout,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

impl Default for BarSection {
    fn default() -> Self {
        Self {
            position: "top".into(),
            transparent: false,
            center_anchor: String::new(),
            layout: BarLayout::default(),
            extra: Map::new(),
        }
    }
}

/// Seconds since user idle began. Defaults mirror Omarchy's shipped
/// `config/omarchy/shell.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Idle {
    pub screensaver: i64,
    pub lock: i64,
}

impl Default for Idle {
    fn default() -> Self {
        Self {
            screensaver: 150,
            lock: 300,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRef {
    pub id: String,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ShellConfig {
    pub version: i64,
    pub bar: BarSection,
    pub idle: Idle,
    pub plugins: Vec<PluginRef>,
    #[serde(rename = "disabledPlugins")]
    pub disabled_plugins: Vec<String>,
    /// Keys a newer Omarchy knows and this Studio doesn't — preserved
    /// verbatim, same rule as `modules::nova`.
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

/// The user's `~/.config/omarchy/shell.json`.
pub fn config_path(paths: &OmarchyPaths) -> PathBuf {
    paths.config.join("shell.json")
}

/// `$OMARCHY_PATH/config/omarchy/shell.json` — the shipped baseline, live
/// only when the user has no file of their own yet.
fn defaults_path(paths: &OmarchyPaths) -> PathBuf {
    paths.system.join("config/omarchy/shell.json")
}

/// The effective config, loaded (or defaulted) and editable in place.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    path: PathBuf,
    /// Whether the *user* file had content. `false` means what's loaded is
    /// Omarchy's own shipped defaults, shown read-only until something writes
    /// a user file — mirroring `omarchy-shell-config`'s own `source_file`.
    pub existed: bool,
    pub cfg: ShellConfig,
}

impl Shell {
    /// Load the effective config: the user's `shell.json` if it has content,
    /// else Omarchy's shipped defaults. Omarchy 4 doesn't deep-merge once a
    /// user file exists (bar README: "your file is canonical"), so showing
    /// "defaults" is only correct when there truly is no override yet.
    pub fn load(paths: &OmarchyPaths) -> Self {
        let path = config_path(paths);
        let user = std::fs::read_to_string(&path).unwrap_or_default();
        if !user.trim().is_empty() {
            return Self {
                path,
                existed: true,
                cfg: serde_json::from_str(&user).unwrap_or_default(),
            };
        }
        let defaults = std::fs::read_to_string(defaults_path(paths)).unwrap_or_default();
        Self {
            path,
            existed: false,
            cfg: serde_json::from_str(&defaults).unwrap_or_default(),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.path
    }

    /// Write `shell.json` (pretty, trailing newline, `version` pinned to 1 —
    /// matching the shape `omarchy-shell-config`'s own `NORMALIZE` jq pipeline
    /// produces). The shell hot-reloads layout changes on save; idle changes
    /// need an explicit [`reload`](Self::reload). Caller snapshots first.
    pub fn save(&mut self) -> Result<()> {
        self.cfg.version = 1;
        let json =
            serde_json::to_string_pretty(&self.cfg).map_err(|e| StudioError::ParseFailed {
                file: self.path.clone(),
                line: None,
                hint: e.to_string(),
            })?;
        atomic_write(&self.path, &format!("{json}\n"))
    }

    /// Ask the running shell to reread `shell.json`, falling back to a plugin
    /// rescan (which also re-reads config) when that request isn't wired up.
    pub fn reload(runner: &dyn CommandRunner) -> Result<()> {
        if runner.run(&cmds::shell_reload())?.ok() {
            return Ok(());
        }
        let out = runner.run(&cmds::shell_rescan_plugins())?;
        if out.ok() {
            return Ok(());
        }
        Err(StudioError::External {
            cmd: "omarchy-shell shell reloadConfig".into(),
            detail: out.stderr.trim().to_string(),
        })
    }
}

/// Set the idle timers and persist + reload. Caller snapshots before calling
/// (the convention `modules::swayosd`/`modules::nova` also use — a single
/// external command with nothing to hash-guard).
pub fn apply_idle(
    paths: &OmarchyPaths,
    screensaver: i64,
    lock: i64,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    let mut shell = Shell::load(paths);
    shell.cfg.idle.screensaver = screensaver;
    shell.cfg.idle.lock = lock;
    shell.save()?;
    Shell::reload(runner)?;
    Ok(shell.path)
}

// ── bar/plugin command placement ────────────────────────────────────────────

/// Where a widget goes, rendered as `omarchy bar`'s own placement flags.
/// Every field optional — an empty placement lets the command fall back to
/// its default spot (`put`'s "leave it where it already is", `enable`'s
/// "widget's usual section").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Placement {
    pub section: Option<Section>,
    pub index: Option<usize>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub from_section: Option<Section>,
    pub from_index: Option<usize>,
}

impl Placement {
    /// `--section left`, `--index 0`, … in the order `omarchy-bar` documents.
    pub fn to_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(s) = self.section {
            out.push("--section".into());
            out.push(s.as_str().into());
        }
        if let Some(i) = self.index {
            out.push("--index".into());
            out.push(i.to_string());
        }
        if let Some(id) = &self.before {
            out.push("--before".into());
            out.push(id.clone());
        }
        if let Some(id) = &self.after {
            out.push("--after".into());
            out.push(id.clone());
        }
        if let Some(s) = self.from_section {
            out.push("--from-section".into());
            out.push(s.as_str().into());
        }
        if let Some(i) = self.from_index {
            out.push("--from-index".into());
            out.push(i.to_string());
        }
        out
    }
}

// ── plugin catalog ──────────────────────────────────────────────────────────

/// One entry from `omarchy plugin list --json` — every discovered shell
/// plugin, not just bar widgets (services, panels, the bar host itself).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default, rename = "canDisable")]
    pub can_disable: bool,
    #[serde(default, rename = "firstParty")]
    pub first_party: bool,
    #[serde(default, rename = "clonedFrom")]
    pub cloned_from: String,
}

impl PluginInfo {
    pub fn is_bar_widget(&self) -> bool {
        self.kinds.iter().any(|k| k == "bar-widget")
    }
}

/// Parse `omarchy plugin list --json`'s stdout.
pub fn parse_plugin_list(stdout: &str) -> Result<Vec<PluginInfo>> {
    serde_json::from_str(stdout).map_err(|e| StudioError::External {
        cmd: "omarchy plugin list --json".into(),
        detail: e.to_string(),
    })
}

/// The bar-widget catalog: every plugin that can go on the bar, sorted by
/// name so a picker reads alphabetically rather than in discovery order.
pub fn bar_widget_catalog(runner: &dyn CommandRunner) -> Result<Vec<PluginInfo>> {
    let out = runner.run(&cmds::plugin_list_json())?;
    if !out.ok() {
        return Err(StudioError::External {
            cmd: "omarchy plugin list --json".into(),
            detail: out.stderr.trim().to_string(),
        });
    }
    let mut all = parse_plugin_list(&out.stdout)?;
    all.retain(PluginInfo::is_bar_widget);
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::StubRunner;

    fn paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-shell-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("cfg/omarchy")).unwrap();
        std::fs::create_dir_all(root.join("share/omarchy/config/omarchy")).unwrap();
        OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join("cfg/omarchy"),
            state: root.join("cfg/state"),
        }
    }

    const SAMPLE: &str = r#"{
      "version": 1,
      "bar": {
        "position": "top",
        "transparent": false,
        "centerAnchor": "omarchy.clock",
        "layout": {
          "left": [ { "id": "omarchy.menu" }, { "id": "omarchy.workspaces" } ],
          "center": [ { "id": "omarchy.clock", "format": "HH:mm" } ],
          "right": [ { "id": "omarchy.audio" }, { "id": "omarchy.power" } ]
        }
      },
      "idle": { "screensaver": 150, "lock": 300 },
      "plugins": [ { "id": "omni" } ],
      "disabledPlugins": [ "omarchy.lock" ]
    }"#;

    #[test]
    fn config_round_trips_and_preserves_unknown_widget_settings() {
        let cfg: ShellConfig = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(cfg.bar.position, "top");
        assert_eq!(cfg.bar.center_anchor, "omarchy.clock");
        assert_eq!(cfg.bar.layout.left.len(), 2);
        assert_eq!(cfg.bar.layout.left[0].id, "omarchy.menu");
        let clock = &cfg.bar.layout.center[0];
        assert_eq!(clock.id, "omarchy.clock");
        assert_eq!(
            clock.settings.get("format").and_then(|v| v.as_str()),
            Some("HH:mm")
        );
        assert_eq!(cfg.idle.screensaver, 150);
        assert_eq!(cfg.idle.lock, 300);
        assert_eq!(cfg.plugins[0].id, "omni");
        assert_eq!(cfg.disabled_plugins, vec!["omarchy.lock"]);

        // A field a newer Omarchy adds and this Studio doesn't know about
        // must survive a load → save cycle untouched.
        let mut with_extra: ShellConfig = serde_json::from_str(SAMPLE).unwrap();
        with_extra
            .extra
            .insert("futureField".into(), serde_json::json!(42));
        let out = serde_json::to_string(&with_extra).unwrap();
        assert!(out.contains("\"futureField\":42"));
    }

    #[test]
    fn layout_find_locates_a_widgets_lane() {
        let cfg: ShellConfig = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(cfg.bar.layout.find("omarchy.clock"), Some(Section::Center));
        assert_eq!(cfg.bar.layout.find("omarchy.power"), Some(Section::Right));
        assert_eq!(cfg.bar.layout.find("nonexistent"), None);
    }

    #[test]
    fn load_falls_back_to_shipped_defaults_when_the_user_has_no_file() {
        let p = paths("defaults");
        std::fs::write(
            defaults_path(&p),
            r#"{"version":1,"bar":{"position":"bottom","layout":{}},"idle":{"screensaver":90,"lock":180},"plugins":[]}"#,
        )
        .unwrap();

        let shell = Shell::load(&p);
        assert!(!shell.existed);
        assert_eq!(shell.cfg.bar.position, "bottom");
        assert_eq!(shell.cfg.idle.lock, 180);
    }

    #[test]
    fn load_prefers_the_users_own_file_over_defaults() {
        let p = paths("prefer-user");
        std::fs::write(
            defaults_path(&p),
            r#"{"version":1,"bar":{"position":"top","layout":{}},"idle":{"screensaver":150,"lock":300},"plugins":[]}"#,
        )
        .unwrap();
        std::fs::write(config_path(&p), SAMPLE).unwrap();

        let shell = Shell::load(&p);
        assert!(shell.existed);
        assert_eq!(shell.cfg.bar.position, "top");
        assert_eq!(shell.cfg.bar.center_anchor, "omarchy.clock");
    }

    #[test]
    fn apply_idle_starts_from_defaults_then_persists_and_reloads() {
        let p = paths("idle");
        std::fs::write(
            defaults_path(&p),
            r#"{"version":1,"bar":{"position":"top","layout":{"left":[{"id":"omarchy.menu"}]}},"idle":{"screensaver":150,"lock":300},"plugins":[]}"#,
        )
        .unwrap();
        let runner = StubRunner::default().with_ok("omarchy-shell shell reloadConfig", "");

        let path = apply_idle(&p, 60, 600, &runner).unwrap();
        assert_eq!(path, config_path(&p));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        let cfg: ShellConfig = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(cfg.idle.screensaver, 60);
        assert_eq!(cfg.idle.lock, 600);
        // The bar layout from defaults survived being carried through.
        assert_eq!(cfg.bar.layout.left[0].id, "omarchy.menu");
        assert_eq!(runner.calls(), vec!["omarchy-shell shell reloadConfig"]);
    }

    #[test]
    fn reload_falls_back_to_rescan_when_reloadconfig_is_unwired() {
        let runner = StubRunner::default()
            .with_fail("omarchy-shell shell reloadConfig")
            .with_ok("omarchy-shell -q shell rescanPlugins", "");
        Shell::reload(&runner).unwrap();
        assert_eq!(
            runner.calls(),
            vec![
                "omarchy-shell shell reloadConfig",
                "omarchy-shell -q shell rescanPlugins"
            ]
        );
    }

    #[test]
    fn placement_renders_only_the_flags_that_are_set() {
        assert_eq!(Placement::default().to_args(), Vec::<String>::new());
        let p = Placement {
            section: Some(Section::Center),
            index: Some(0),
            ..Default::default()
        };
        assert_eq!(p.to_args(), vec!["--section", "center", "--index", "0"]);
        let p = Placement {
            after: Some("omarchy.clock".into()),
            ..Default::default()
        };
        assert_eq!(p.to_args(), vec!["--after", "omarchy.clock"]);
    }

    const PLUGIN_LIST: &str = r#"[
      {"id":"omarchy.active-window","name":"Active window","kinds":["bar-widget"],"enabled":false,"active":false,"canDisable":true,"firstParty":true,"clonedFrom":""},
      {"id":"omarchy.background","name":"Background","kinds":["service"],"enabled":true,"active":false,"canDisable":true,"firstParty":true,"clonedFrom":""},
      {"id":"omarchy.audio","name":"Audio","kinds":["bar-widget"],"enabled":true,"active":false,"canDisable":true,"firstParty":true,"clonedFrom":""}
    ]"#;

    #[test]
    fn bar_widget_catalog_filters_to_bar_widgets_sorted_by_name() {
        let runner = StubRunner::default().with_ok("omarchy plugin list --json", PLUGIN_LIST);
        let cat = bar_widget_catalog(&runner).unwrap();
        assert_eq!(cat.len(), 2, "the service entry is excluded");
        assert_eq!(cat[0].name, "Active window");
        assert_eq!(cat[1].name, "Audio");
    }

    #[test]
    fn bar_widget_catalog_surfaces_a_failing_command() {
        let runner = StubRunner::default().with_fail("omarchy plugin list --json");
        assert!(bar_widget_catalog(&runner).is_err());
    }
}

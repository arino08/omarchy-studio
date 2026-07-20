//! Waybar module catalog (spec 05 §4, roadmap 0.4.2).
//!
//! A catalogue of the standard Waybar modules, each with a human label, a
//! grouping category, a one-line description, any packages it needs, and a
//! sensible default config snippet to drop in when the user adds it. Unknown
//! ids (the user's own `custom/*`) are handled gracefully — the catalog is for
//! guidance, never a gate.

/// A grouping for the module picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Workspaces,
    System,
    Hardware,
    Audio,
    Network,
    Clock,
    Tray,
    Custom,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Workspaces => "Workspaces & window",
            Group::System => "System",
            Group::Hardware => "Hardware",
            Group::Audio => "Audio",
            Group::Network => "Network",
            Group::Clock => "Clock",
            Group::Tray => "Tray",
            Group::Custom => "Custom",
        }
    }
}

/// One catalog entry.
#[derive(Debug, Clone, Copy)]
pub struct Module {
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub summary: &'static str,
    /// Package(s) this module needs to function, if any (dep-guidance).
    pub needs: &'static [&'static str],
    /// A default config object to insert with the module (JSONC, no key), or
    /// empty when the module needs no config.
    pub default_config: &'static str,
}

use Group::*;

pub const MODULES: &[Module] = &[
    m("hyprland/workspaces", "Workspaces", Workspaces, "The Hyprland workspace pills", &[], ""),
    m("hyprland/window", "Focused window", Workspaces, "Title of the active window", &[], ""),
    m("hyprland/submap", "Active submap", Workspaces, "Shows the current keybind submap", &[], ""),
    m("clock", "Clock", Clock, "Date and time", &[], "{\n  \"format\": \"{:%H:%M}\",\n  \"tooltip-format\": \"<big>{:%Y %B}</big>\\n<tt><small>{calendar}</small></tt>\"\n}"),
    m("battery", "Battery", Hardware, "Charge level and state", &[], "{\n  \"format\": \"{icon} {capacity}%\",\n  \"format-icons\": [\"\", \"\", \"\", \"\", \"\"]\n}"),
    m("cpu", "CPU", Hardware, "Processor load", &[], "{\n  \"format\": \" {usage}%\"\n}"),
    m("memory", "Memory", Hardware, "RAM usage", &[], "{\n  \"format\": \" {}%\"\n}"),
    m("temperature", "Temperature", Hardware, "CPU/GPU temperature", &["lm_sensors"], "{\n  \"critical-threshold\": 80,\n  \"format\": \" {temperatureC}°C\"\n}"),
    m("disk", "Disk", Hardware, "Filesystem usage", &[], "{\n  \"format\": \" {percentage_used}%\"\n}"),
    m("backlight", "Brightness", Hardware, "Screen backlight level", &[], "{\n  \"format\": \"{icon} {percent}%\",\n  \"format-icons\": [\"\", \"\"]\n}"),
    m("idle_inhibitor", "Keep awake", System, "Toggle to stop the screen sleeping", &[], "{\n  \"format\": \"{icon}\",\n  \"format-icons\": { \"activated\": \"\", \"deactivated\": \"\" }\n}"),
    m("power-profiles-daemon", "Power profile", System, "Balanced / performance / saver", &["power-profiles-daemon"], ""),
    m("pulseaudio", "Volume", Audio, "Output volume and device", &[], "{\n  \"format\": \"{icon} {volume}%\",\n  \"format-muted\": \" muted\",\n  \"on-click\": \"pavucontrol\"\n}"),
    m("wireplumber", "Volume (WirePlumber)", Audio, "Volume via WirePlumber", &["wireplumber"], "{\n  \"format\": \"{icon} {volume}%\"\n}"),
    m("mpris", "Now playing", Audio, "Current media track", &["playerctl"], "{\n  \"format\": \"{player_icon} {title}\"\n}"),
    m("network", "Network", Network, "Wi-Fi / ethernet status", &[], "{\n  \"format-wifi\": \" {signalStrength}%\",\n  \"format-ethernet\": \" wired\",\n  \"format-disconnected\": \"⚠ offline\"\n}"),
    m("bluetooth", "Bluetooth", Network, "Bluetooth status and devices", &[], "{\n  \"format\": \" {status}\"\n}"),
    m("tray", "System tray", Tray, "Status icons from running apps", &[], "{\n  \"spacing\": 10\n}"),
    m("custom/*", "Custom command", Custom, "A script-driven module you define", &[], "{\n  \"exec\": \"your-command\",\n  \"interval\": 5,\n  \"format\": \"{}\"\n}"),
    m("group/*", "Module group", Custom, "A collapsible group of modules", &[], "{\n  \"orientation\": \"horizontal\",\n  \"modules\": []\n}"),
];

const fn m(
    id: &'static str,
    label: &'static str,
    group: Group,
    summary: &'static str,
    needs: &'static [&'static str],
    default_config: &'static str,
) -> Module {
    Module {
        id,
        label,
        group,
        summary,
        needs,
        default_config,
    }
}

/// Look up a module by exact id, falling back to the `custom/*` and `group/*`
/// catalog entries for any `custom/…` / `group/…` id the user has invented.
pub fn lookup(id: &str) -> Option<&'static Module> {
    if let Some(exact) = MODULES.iter().find(|m| m.id == id) {
        return Some(exact);
    }
    if id.starts_with("custom/") {
        return MODULES.iter().find(|m| m.id == "custom/*");
    }
    if id.starts_with("group/") {
        return MODULES.iter().find(|m| m.id == "group/*");
    }
    None
}

/// The human label for a module id (falls back to the id itself).
pub fn label_for(id: &str) -> String {
    match lookup(id) {
        // for custom/group, show the user's own suffix so rows stay distinct
        Some(m) if m.id.ends_with("/*") => id.to_string(),
        Some(m) => m.label.to_string(),
        None => id.to_string(),
    }
}

pub fn in_group(g: Group) -> impl Iterator<Item = &'static Module> {
    MODULES.iter().filter(move |m| m.group == g)
}

// ── config model ─────────────────────────────────────────────────────────────

use std::path::PathBuf;

use crate::configfs::jsonc::JsoncDoc;
use crate::error::{Result, StudioError};
use crate::omarchy::{Component, OmarchyPaths};

/// The three Waybar module lanes, in bar order.
pub const LANES: [&str; 3] = ["modules-left", "modules-center", "modules-right"];

/// A live view over the user's `config.jsonc`, edited through the JSONC editor
/// so comments and formatting survive.
pub struct WaybarConfig {
    doc: JsoncDoc,
    path: PathBuf,
}

pub(crate) fn config_path(paths: &OmarchyPaths) -> PathBuf {
    paths
        .config
        .parent()
        .map(|c| c.join("waybar/config.jsonc"))
        .unwrap_or_else(|| PathBuf::from("waybar/config.jsonc"))
}

/// Resolve the `config.jsonc` Studio should edit (roadmap 0.8.2): a `[targets]`
/// override wins, else a running Waybar's `-c`, else the Omarchy default. Stock
/// setups get the default with zero config.
pub fn resolve_config(paths: &OmarchyPaths) -> crate::modules::targets::Resolved {
    use crate::modules::targets;
    let overrides = targets::Overrides::load(&crate::studio_config_dir().join("config.toml"));
    let instances = targets::waybar_instances();
    targets::resolve_waybar_config(config_path(paths), &overrides, &instances)
}

impl WaybarConfig {
    pub fn load(paths: &OmarchyPaths) -> Result<Self> {
        let path = resolve_config(paths).path;
        let src = std::fs::read_to_string(&path).map_err(|e| StudioError::External {
            cmd: format!("read {}", path.display()),
            detail: e.to_string(),
        })?;
        let doc = JsoncDoc::parse(&src).map_err(|e| StudioError::ParseFailed {
            file: path.clone(),
            line: None,
            hint: e,
        })?;
        Ok(Self { doc, path })
    }

    /// Module ids in a lane, unquoted for display (e.g. `clock`, `custom/x`).
    pub fn lane(&self, lane: &str) -> Vec<String> {
        self.doc
            .array_items(lane)
            .unwrap_or_default()
            .iter()
            .map(|s| s.trim_matches('"').to_string())
            .collect()
    }

    /// All three lanes in bar order.
    pub fn lanes(&self) -> Vec<(&'static str, Vec<String>)> {
        LANES.iter().map(|l| (*l, self.lane(l))).collect()
    }

    pub fn reorder(&mut self, lane: &str, ids: &[String]) -> Result<()> {
        let quoted: Vec<String> = ids.iter().map(|i| format!("\"{i}\"")).collect();
        self.doc.reorder(lane, &quoted).map_err(edit_err)
    }

    pub fn add(&mut self, lane: &str, id: &str) -> Result<()> {
        self.doc
            .add_item(lane, &format!("\"{id}\""))
            .map_err(edit_err)
    }

    pub fn remove(&mut self, lane: &str, id: &str) -> Result<()> {
        self.doc
            .remove_item(lane, &format!("\"{id}\""))
            .map_err(edit_err)
    }

    /// Move a module to `lane`, removing it from whichever lane holds it.
    pub fn move_to(&mut self, id: &str, lane: &str) -> Result<()> {
        for l in LANES {
            self.remove(l, id)?;
        }
        self.add(lane, id)
    }

    /// Set a scalar config value in place (e.g. `height` → `30`). `raw` is the
    /// literal JSON text (numbers bare, strings quoted).
    pub fn set_scalar(&mut self, path: &str, raw: &str) -> Result<()> {
        self.doc.set_scalar(path, raw).map_err(edit_err)
    }

    /// Scaffold a `custom/<name>` module from a plain-language spec: write its
    /// config object at the root and drop the module id into `lane`. Returns the
    /// full module id (`custom/<name>`).
    pub fn add_custom_module(&mut self, spec: &CustomSpec, lane: &str) -> Result<String> {
        let name = spec.slug();
        if name.is_empty() {
            return Err(edit_err("a custom module needs a name".into()));
        }
        let id = format!("custom/{name}");
        let mut entries: Vec<String> = vec![format!("\"exec\": \"{}\"", json_escape(&spec.exec))];
        if let Some(iv) = spec.interval {
            entries.push(format!("\"interval\": {iv}"));
        }
        entries.push(format!("\"format\": \"{}\"", json_escape(&spec.format)));
        if let Some(oc) = spec.on_click.as_deref().filter(|s| !s.trim().is_empty()) {
            entries.push(format!("\"on-click\": \"{}\"", json_escape(oc)));
        }
        let value = format!("{{\n  {}\n}}", entries.join(",\n  "));
        self.doc.insert_member("", &id, &value).map_err(edit_err)?;
        self.add(lane, &id)?;
        Ok(id)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn save(&self) -> Result<()> {
        crate::configfs::atomic_write(&self.path, &self.doc.to_string())
    }

    /// Save and reload Waybar so the change shows. Caller snapshots first.
    /// Reloads an already-running Waybar only — if Waybar isn't the user's
    /// active bar (they run a Quickshell shell or similar) the edit is saved but
    /// no Waybar is spawned on top of it.
    pub fn apply(&self, runner: &dyn crate::cmd::CommandRunner) -> Result<()> {
        self.save()?;
        crate::omarchy::restart_if_running(runner, Component::Waybar)?;
        Ok(())
    }

    /// Apply with a crash watchdog (roadmap 0.4.6): save + restart, then confirm
    /// Waybar stayed up. If it was running before and is gone `settle` after the
    /// restart, the previous config is written back and Waybar restarted again —
    /// so a bad edit can never leave the user bar-less.
    ///
    /// The safety net only arms when Waybar was confirmed running beforehand: if
    /// we can't even check (no `pgrep`, headless session), we apply without
    /// reverting rather than risk clobbering a good change on a false negative.
    /// Plan this config as a pipeline edit, writing nothing.
    pub fn plan(&self) -> crate::engine::FileEdit {
        // `None` for a file that doesn't exist yet — the pipeline's hash guard
        // reads it the same way.
        let on_disk = std::fs::read_to_string(&self.path).ok();
        crate::engine::FileEdit::new(self.path.clone(), on_disk.as_deref(), self.doc.to_string())
    }

    /// Apply through the pipeline, watching that Waybar survives the change.
    ///
    /// This used to hand-roll save → restart → settle → check → restore, which
    /// is precisely what the pipeline does; the one rule it can't express is
    /// Omarchy-specific and preserved here: **never spawn a bar the user isn't
    /// running.** If Waybar wasn't up (they run a Quickshell shell), the edit
    /// is still snapshotted and written, but nothing restarts and there is
    /// nothing to watch — so no reload and no verification go in the plan.
    pub fn apply_watched(
        &self,
        store: &crate::snapshot::SnapshotStore,
        runner: &dyn crate::cmd::CommandRunner,
        settle: std::time::Duration,
    ) -> Result<ApplyOutcome> {
        let was_alive = waybar_alive(runner);
        let (reload, verify) = if was_alive {
            (
                vec![crate::engine::ReloadStep::Restart(Component::Waybar)],
                vec![crate::engine::Verification::ProcessAlive {
                    name: "waybar",
                    grace: settle,
                }],
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let plan = crate::engine::ApplyPlan {
            summary: "waybar config".into(),
            module: "waybar".into(),
            edits: vec![self.plan()],
            reload,
            verify,
            risk: crate::engine::Risk::Restarty,
            trailers: Vec::new(),
        };
        match crate::engine::Pipeline::new(store, runner).apply(&plan, false) {
            Ok(_) if was_alive => Ok(ApplyOutcome::Applied),
            Ok(_) => Ok(ApplyOutcome::AppliedUnwatched),
            // The bar died on the new config: the pipeline restored the old one
            // and re-ran the restart, so Waybar is back on the previous config.
            Err(StudioError::VerifyFailed {
                rolled_back: true, ..
            }) => Ok(ApplyOutcome::Reverted),
            Err(e) => Err(e),
        }
    }
}

/// Result of a watched apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The change was applied and Waybar is still running.
    Applied,
    /// Applied, but Waybar wasn't running beforehand so the watchdog stood down.
    AppliedUnwatched,
    /// The change stopped Waybar; the previous config was restored and the bar
    /// brought back.
    Reverted,
}

/// Whether a process named `waybar` is currently running. A failure to check
/// (missing `pgrep`, etc.) reads as "not running" — callers pair this with a
/// pre-apply check so an unknowable state disarms the watchdog rather than
/// triggering a revert.
fn waybar_alive(runner: &dyn crate::cmd::CommandRunner) -> bool {
    runner
        .run(&crate::omarchy::cmds::process_alive("waybar"))
        .map(|o| o.ok())
        .unwrap_or(false)
}

fn edit_err(e: String) -> StudioError {
    StudioError::External {
        cmd: "edit waybar config".into(),
        detail: e,
    }
}

/// A plain-language description of a script-driven Waybar module, as gathered by
/// the custom-module wizard (roadmap 0.4.5).
#[derive(Debug, Clone, Default)]
pub struct CustomSpec {
    /// Human name (becomes the `custom/<slug>` id).
    pub name: String,
    /// The shell command that produces the module's text.
    pub exec: String,
    /// How often to re-run it, in seconds (None = run once / signal-driven).
    pub interval: Option<u32>,
    /// The display format (defaults to `{}` — the command's raw output).
    pub format: String,
    /// Optional command to run when the module is clicked.
    pub on_click: Option<String>,
}

impl CustomSpec {
    /// The `custom/<slug>` suffix — lowercased, non-alphanumerics to dashes.
    pub fn slug(&self) -> String {
        let mut out = String::new();
        let mut prev_dash = false;
        for ch in self.name.trim().chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
                prev_dash = false;
            } else if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        }
        out.trim_end_matches('-').to_string()
    }
}

/// Escape a string for embedding in a JSON double-quoted literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

// ── style.css geometry (roadmap 0.4.4) ───────────────────────────────────────

use crate::configfs::{CommentStyle, ManagedBlock};

/// A couple of safe geometry knobs written to a managed CSS block at the end of
/// the user's `style.css` — after the theme `@import`, so it wins. We never
/// touch the import or the user's own rules.
#[derive(Default)]
pub struct WaybarStyle {
    pub font_size: Option<u32>,
    pub radius: Option<u32>,
}

pub(crate) fn style_path(paths: &OmarchyPaths) -> PathBuf {
    paths
        .config
        .parent()
        .map(|c| c.join("waybar/style.css"))
        .unwrap_or_else(|| PathBuf::from("waybar/style.css"))
}

/// Resolve the `style.css` Studio should edit (roadmap 0.8.2): override, else a
/// running Waybar's `-s`, else the Omarchy default.
pub fn resolve_style(paths: &OmarchyPaths) -> crate::modules::targets::Resolved {
    use crate::modules::targets;
    let overrides = targets::Overrides::load(&crate::studio_config_dir().join("config.toml"));
    let instances = targets::waybar_instances();
    targets::resolve_waybar_style(style_path(paths), &overrides, &instances)
}

pub(crate) fn style_block() -> ManagedBlock {
    ManagedBlock::new("waybar-style", CommentStyle::CBlock)
}

impl WaybarStyle {
    pub fn load(paths: &OmarchyPaths) -> Self {
        let mut s = Self::default();
        if let Ok(text) = std::fs::read_to_string(resolve_style(paths).path) {
            if let Some(body) = style_block().extract(&text) {
                s.font_size = grab_px(body, "font-size");
                s.radius = grab_px(body, "border-radius");
            }
        }
        s
    }

    fn body(&self) -> String {
        let mut decls = String::new();
        if let Some(fs) = self.font_size {
            decls.push_str(&format!("  font-size: {fs}px;\n"));
        }
        if let Some(r) = self.radius {
            decls.push_str(&format!("  border-radius: {r}px;\n"));
        }
        format!("* {{\n{decls}}}")
    }

    fn is_empty(&self) -> bool {
        self.font_size.is_none() && self.radius.is_none()
    }

    /// Write the managed block into style.css (removed when empty). The block
    /// always lands at the end, after any `@import`. Returns the path.
    pub fn save(&self, paths: &OmarchyPaths) -> Result<PathBuf> {
        let path = resolve_style(paths).path;
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = if self.is_empty() {
            style_block().remove(&existing)
        } else {
            style_block().upsert(&existing, &self.body())
        };
        crate::configfs::atomic_write(&path, &updated)?;
        Ok(path)
    }

    pub fn apply(
        &self,
        paths: &OmarchyPaths,
        runner: &dyn crate::cmd::CommandRunner,
    ) -> Result<PathBuf> {
        let path = self.save(paths)?;
        // Reload a running Waybar only — never spawn one on top of a user's
        // alternative bar (see `WaybarConfig::apply`).
        crate::omarchy::restart_if_running(runner, Component::Waybar)?;
        Ok(path)
    }
}

/// Pull an integer px value for a CSS property out of a block body.
fn grab_px(body: &str, prop: &str) -> Option<u32> {
    let needle = format!("{prop}:");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_unique_and_snippets_are_valid_jsonc() {
        use crate::configfs::jsonc::JsoncDoc;
        let mut ids: Vec<_> = MODULES.iter().map(|m| m.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate module id");

        for m in MODULES {
            if !m.default_config.is_empty() {
                // each snippet must parse as a JSONC object
                let wrapped = format!(
                    "{{ \"{}\": {} }}",
                    m.id.trim_end_matches("/*"),
                    m.default_config
                );
                JsoncDoc::parse(&wrapped)
                    .unwrap_or_else(|e| panic!("bad snippet for {}: {e}", m.id));
            }
        }
    }

    #[test]
    fn resolves_standard_and_custom_ids() {
        assert_eq!(lookup("clock").unwrap().group, Group::Clock);
        assert_eq!(label_for("clock"), "Clock");
        // the user's real modules resolve (custom/* + group/* fallbacks)
        for id in [
            "hyprland/workspaces",
            "battery",
            "network",
            "pulseaudio",
            "tray",
        ] {
            assert!(lookup(id).is_some(), "missing {id}");
        }
        assert!(lookup("custom/voxtype").is_some());
        assert_eq!(label_for("custom/voxtype"), "custom/voxtype");
        assert!(lookup("group/tray-expander").is_some());
        assert!(lookup("nonexistent-core-module").is_none());
    }

    #[test]
    fn deps_are_surfaced() {
        assert_eq!(lookup("temperature").unwrap().needs, &["lm_sensors"]);
        assert!(lookup("clock").unwrap().needs.is_empty());
    }

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-wb-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/waybar")).unwrap();
        OmarchyPaths {
            system: root.join("sys/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    #[test]
    fn config_model_reads_and_edits_lanes() {
        let paths = fake_paths("model");
        std::fs::write(
            config_path(&paths),
            "{\n  // bar\n  \"modules-left\": [\"clock\", \"cpu\"],\n  \"modules-center\": [],\n  \"modules-right\": [\"battery\", \"tray\"]\n}\n",
        )
        .unwrap();

        let mut wb = WaybarConfig::load(&paths).unwrap();
        assert_eq!(wb.lane("modules-left"), vec!["clock", "cpu"]);
        assert_eq!(wb.lanes().len(), 3);

        // reorder, add, remove — all through the comment-preserving editor
        wb.reorder("modules-left", &["cpu".into(), "clock".into()])
            .unwrap();
        assert_eq!(wb.lane("modules-left"), vec!["cpu", "clock"]);
        wb.add("modules-center", "network").unwrap();
        assert_eq!(wb.lane("modules-center"), vec!["network"]);
        wb.remove("modules-right", "battery").unwrap();
        assert_eq!(wb.lane("modules-right"), vec!["tray"]);

        wb.save().unwrap();
        let on_disk = std::fs::read_to_string(config_path(&paths)).unwrap();
        assert!(on_disk.contains("// bar")); // comment preserved through edits
                                             // reloads to the same state
        let wb2 = WaybarConfig::load(&paths).unwrap();
        assert_eq!(wb2.lane("modules-left"), vec!["cpu", "clock"]);
    }

    #[test]
    fn custom_module_wizard_scaffolds_config_and_lane_entry() {
        let paths = fake_paths("custom");
        std::fs::write(
            config_path(&paths),
            "{\n  \"modules-left\": [\"clock\"],\n  \"modules-center\": [],\n  \"modules-right\": []\n}\n",
        )
        .unwrap();

        let mut wb = WaybarConfig::load(&paths).unwrap();
        let spec = CustomSpec {
            name: "My Weather!".into(),
            exec: "curl wttr.in?format=3".into(),
            interval: Some(600),
            format: "{}".into(),
            on_click: Some("xdg-open https://wttr.in".into()),
        };
        let id = wb.add_custom_module(&spec, "modules-right").unwrap();
        assert_eq!(id, "custom/my-weather");
        assert_eq!(wb.lane("modules-right"), vec!["custom/my-weather"]);

        wb.save().unwrap();
        let re = WaybarConfig::load(&paths).unwrap();
        // config object landed and is reachable + valid
        let on_disk = std::fs::read_to_string(config_path(&paths)).unwrap();
        assert!(on_disk.contains("\"custom/my-weather\""));
        assert_eq!(re.lane("modules-right"), vec!["custom/my-weather"]);
        assert!(on_disk.contains("\"interval\": 600"));
        assert!(on_disk.contains("\"on-click\""));
        // the original module survived
        assert_eq!(re.lane("modules-left"), vec!["clock"]);
    }

    #[test]
    fn watchdog_reverts_when_waybar_dies_and_leaves_good_edits_alone() {
        use crate::cmd::{CmdOutput, StubRunner};
        use std::time::Duration;

        let paths = fake_paths("watch");
        let good = "{\n  \"modules-left\": [\"clock\"],\n  \"modules-center\": [],\n  \"modules-right\": []\n}\n";
        std::fs::write(config_path(&paths), good).unwrap();

        let restart = crate::omarchy::cmds::restart(Component::Waybar).display();
        let alive = crate::omarchy::cmds::process_alive("waybar").display();
        let dead = CmdOutput {
            status: 1,
            ..Default::default()
        };
        let up = CmdOutput {
            status: 0,
            ..Default::default()
        };

        // The pipeline snapshots with real git; the stubs cover pgrep/restart.
        let store = crate::snapshot::SnapshotStore::open_or_init(
            paths.config.join("history"),
            Box::new(crate::cmd::RealRunner),
        )
        .unwrap();

        // Case 1: Waybar alive before, alive after → Applied, edit persists.
        let mut wb = WaybarConfig::load(&paths).unwrap();
        wb.add("modules-center", "cpu").unwrap();
        let runner = StubRunner::default()
            .with(&alive, up.clone())
            .with(&restart, up.clone());
        assert_eq!(
            wb.apply_watched(&store, &runner, Duration::ZERO).unwrap(),
            ApplyOutcome::Applied
        );
        assert!(std::fs::read_to_string(config_path(&paths))
            .unwrap()
            .contains("cpu"));

        // Case 2: Waybar alive before, gone after → Reverted, disk back to good.
        let mut wb = WaybarConfig::load(&paths).unwrap();
        let before = std::fs::read_to_string(config_path(&paths)).unwrap();
        wb.add("modules-right", "battery").unwrap();
        // first `pgrep` (before) = up, restart ok, second `pgrep` (after) = dead
        let runner = Recorder::new(vec![up.clone(), up.clone(), dead.clone()]);
        assert_eq!(
            wb.apply_watched(&store, &runner, Duration::ZERO).unwrap(),
            ApplyOutcome::Reverted
        );
        assert_eq!(
            std::fs::read_to_string(config_path(&paths)).unwrap(),
            before,
            "a bar-killing edit must be rolled back"
        );

        // Case 3: Waybar not running beforehand → AppliedUnwatched (no revert).
        let mut wb = WaybarConfig::load(&paths).unwrap();
        wb.add("modules-center", "memory").unwrap();
        let runner = StubRunner::default()
            .with(&alive, dead.clone())
            .with(&restart, up.clone());
        assert_eq!(
            wb.apply_watched(&store, &runner, Duration::ZERO).unwrap(),
            ApplyOutcome::AppliedUnwatched
        );
        assert!(std::fs::read_to_string(config_path(&paths))
            .unwrap()
            .contains("memory"));
    }

    /// A runner that returns scripted outputs in call order (so the same command
    /// — `pgrep` — can answer differently before vs after the restart).
    struct Recorder {
        outs: std::sync::Mutex<std::collections::VecDeque<crate::cmd::CmdOutput>>,
    }
    impl Recorder {
        fn new(outs: Vec<crate::cmd::CmdOutput>) -> Self {
            Self {
                outs: std::sync::Mutex::new(outs.into()),
            }
        }
    }
    impl crate::cmd::CommandRunner for Recorder {
        fn run(&self, _cmd: &crate::cmd::Cmd) -> Result<crate::cmd::CmdOutput> {
            Ok(self.outs.lock().unwrap().pop_front().unwrap_or_default())
        }
    }

    #[test]
    fn slug_and_escape_are_sane() {
        let s = CustomSpec {
            name: "  Foo Bar / Baz  ".into(),
            ..Default::default()
        };
        assert_eq!(s.slug(), "foo-bar-baz");
        assert_eq!(json_escape(r#"say "hi"\ok"#), r#"say \"hi\"\\ok"#);
    }

    #[test]
    fn style_geometry_block_after_import_and_round_trips() {
        let paths = fake_paths("style");
        let css = "@import \"theme.css\";\n\n* {\n  font-size: 12px;\n}\n";
        std::fs::write(style_path(&paths), css).unwrap();

        let mut st = WaybarStyle::load(&paths);
        assert!(st.font_size.is_none()); // nothing managed yet
        st.font_size = Some(14);
        st.radius = Some(6);
        st.save(&paths).unwrap();

        let out = std::fs::read_to_string(style_path(&paths)).unwrap();
        // the @import stays first; our block is appended after the user's rules
        assert!(out.starts_with("@import"));
        let import_pos = out.find("@import").unwrap();
        let block_pos = out.find("omarchy-studio:waybar-style").unwrap();
        assert!(import_pos < block_pos, "managed block must follow @import");
        assert!(out.contains("font-size: 14px"));
        assert!(out.contains("border-radius: 6px"));
        // the user's own rule is untouched
        assert!(out.contains("font-size: 12px"));

        // reload reads the managed values back
        let re = WaybarStyle::load(&paths);
        assert_eq!(re.font_size, Some(14));
        assert_eq!(re.radius, Some(6));

        // clearing removes the block, leaving the original css intact
        let empty = WaybarStyle::default();
        empty.save(&paths).unwrap();
        let cleared = std::fs::read_to_string(style_path(&paths)).unwrap();
        assert!(!cleared.contains("omarchy-studio:waybar-style"));
        assert_eq!(cleared, css);
    }
}

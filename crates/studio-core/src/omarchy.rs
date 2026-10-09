//! Omarchy adapter (spec 02). The ONLY module allowed to know Omarchy paths
//! and command names — CI greps enforce this (spec 02 §6).

use std::path::PathBuf;

use crate::cmd::{find_in_path, Cmd, CommandRunner};

/// Resolved Omarchy locations (spec 02 §1).
#[derive(Debug, Clone)]
pub struct OmarchyPaths {
    /// `~/.local/share/omarchy` — `$OMARCHY_PATH`. READ-ONLY, always.
    pub system: PathBuf,
    /// `~/.config/omarchy`
    pub config: PathBuf,
    /// `~/.local/state/omarchy`
    pub state: PathBuf,
}

impl OmarchyPaths {
    pub fn discover() -> crate::error::Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            crate::StudioError::External {
                cmd: "env".into(),
                detail: "HOME is not set".into(),
            }
        })?;
        Ok(Self {
            system: std::env::var_os("OMARCHY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share/omarchy")),
            config: home.join(".config/omarchy"),
            state: home.join(".local/state/omarchy"),
        })
    }

    /// Is there an Omarchy install here at all? (Gates everything else.)
    pub fn installed(&self) -> bool {
        self.system.join("version").is_file()
    }

    /// Slug of the active theme (`current/theme.name`) — spec 02 §1.
    pub fn current_theme_name(&self) -> crate::error::Result<String> {
        let p = self.config.join("current/theme.name");
        Ok(std::fs::read_to_string(p)?.trim().to_string())
    }

    /// `~/.config/hypr` — the user's own Hyprland config, sourced last and thus
    /// the right place for Studio's keybind/looknfeel overrides. Derived as a
    /// sibling of `config` (`~/.config/omarchy` → `~/.config/hypr`).
    pub fn hypr_config(&self) -> PathBuf {
        self.config
            .parent()
            .map(|p| p.join("hypr"))
            .unwrap_or_else(|| PathBuf::from("hypr"))
    }

    /// `~/.local/share/applications` — where Omarchy drops web-app launchers
    /// (`omarchy-webapp-install`/`-remove` hardcode `$HOME/.local/share`,
    /// ignoring `$XDG_DATA_HOME`, so we match that exactly). `<Name>.desktop`
    /// here with an `Exec=` that runs `omarchy-launch-webapp` is a web app.
    pub fn applications(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join(".local/share/applications")
    }

    /// The active theme's `colors.toml` (`current/theme` symlinks to the live
    /// theme dir) — the single source of truth for the running palette.
    pub fn current_colors(&self) -> PathBuf {
        self.config.join("current/theme/colors.toml")
    }

    /// The live theme directory itself (`current/theme`).
    pub fn current_theme_dir(&self) -> PathBuf {
        self.config.join("current/theme")
    }

    /// Omarchy's built-in templates (`default/themed/*.tpl`). A theme that
    /// doesn't ship one of these files still gets it, rendered from the
    /// palette — which is what makes a *missing* non-templated file a real
    /// breakage rather than a cosmetic gap.
    pub fn default_templates(&self) -> PathBuf {
        self.system.join("default/themed")
    }

    /// The user's own template overrides/additions (`~/.config/omarchy/themed`).
    pub fn user_templates(&self) -> PathBuf {
        self.config.join("themed")
    }

    /// Where themes live: Omarchy's own, then the user's (which overlay by
    /// name — a community install lands in the user dir).
    pub fn theme_dirs(&self) -> [PathBuf; 2] {
        [self.system.join("themes"), self.config.join("themes")]
    }

    /// Omarchy's own version, from the `version` file it ships. Empty when
    /// it can't be read — callers warn, never block, since an unknown
    /// version is exactly the case where guessing hurts.
    pub fn omarchy_version(&self) -> String {
        std::fs::read_to_string(self.system.join("version"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
}

/// Capability probe result (spec 02 §5).
///
/// Probing is cheap (< 20 ms: two fast execs + a few stats), so v0.1 probes on
/// every startup; the on-disk cache keyed by versions arrives with `toml_edit`
/// (roadmap 0.1.5) if probing ever shows up in a profile.
#[derive(Debug, Default, Clone)]
pub struct Capabilities {
    /// Contents of `$OMARCHY_PATH/version`, e.g. `3.8.2`. Empty = not found.
    pub omarchy_version: String,
    /// Parsed from `hyprctl version`, e.g. `0.55.2`. Empty = hyprctl absent
    /// or not in a Hyprland session (e.g. SSH).
    pub hyprland_version: String,
    /// Which config dialect the *running* compositor loaded, from
    /// `hyprctl systeminfo` (`configProvider: lua` on Omarchy 4). Empty when
    /// unknown — an old Hyprland that doesn't report it, or no session.
    /// Authoritative in a way file-existence checks are not: this machine keeps
    /// both `hyprland.conf` and `hyprland.lua` on disk, and only one is live.
    pub config_provider: String,
    /// `hyprctl configerrors` supported — enables post-apply verification. [gate]
    pub has_configerrors: bool,
    /// `default/themed/` template engine present (Omarchy ≥ 3.x theme model).
    pub has_templates: bool,
    /// `bin/omarchy-hook` present — lifecycle hooks usable.
    pub has_hooks: bool,
    /// Stock `TUI.float` window rule present — our floating launch needs no
    /// rule installation (spec 02 §3.3).
    pub tui_float_rule: bool,
    /// `mpvpaper-rs` or `mpvpaper` in PATH — video wallpapers playable.
    pub video_wallpapers: bool,
    /// `$OMARCHY_PATH` git checkout has local modifications — will conflict
    /// on `omarchy-update` (doctor warns; the reference machine's video-
    /// wallpaper patch is the canonical case).
    pub omarchy_dirty: bool,
}

impl Capabilities {
    pub fn probe(paths: &OmarchyPaths, runner: &dyn CommandRunner) -> Self {
        let omarchy_version = paths.omarchy_version();

        let hyprland_version = runner
            .run(&Cmd::new("hyprctl").arg("version"))
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| parse_hyprland_version(&o.stdout))
            .unwrap_or_default();

        let config_provider = runner
            .run(&Cmd::new("hyprctl").arg("systeminfo"))
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| parse_config_provider(&o.stdout))
            .unwrap_or_default();

        let has_configerrors = runner
            .run(&Cmd::new("hyprctl").arg("configerrors"))
            .map(|o| o.ok())
            .unwrap_or(false);

        let omarchy_dirty = runner
            .run(
                &Cmd::new("git")
                    .args(["-C".to_string(), paths.system.display().to_string()])
                    .args(["status", "--porcelain"]),
            )
            .map(|o| o.ok() && !o.stdout.trim().is_empty())
            .unwrap_or(false);

        Self {
            omarchy_version,
            hyprland_version,
            config_provider,
            has_configerrors,
            has_templates: paths.system.join("default/themed").is_dir(),
            has_hooks: paths.system.join("bin/omarchy-hook").is_file(),
            // Omarchy 4 moved this rule from apps/system.conf to apps/system.lua;
            // check both so the probe doesn't report a missing rule that is
            // actually present (and would send us installing a duplicate).
            tui_float_rule: [
                "default/hypr/apps/system.lua",
                "default/hypr/apps/system.conf",
            ]
            .iter()
            .any(|f| {
                std::fs::read_to_string(paths.system.join(f))
                    .map(|s| s.contains("TUI.float"))
                    .unwrap_or(false)
            }),
            video_wallpapers: find_in_path("mpvpaper-rs").is_some()
                || find_in_path("mpvpaper").is_some(),
            omarchy_dirty,
        }
    }
}

impl Capabilities {
    /// Is Hyprland running the Lua config dialect (Omarchy 4 "Quattro")?
    ///
    /// Studio's hyprlang writers target `~/.config/hypr/*.conf`, which a
    /// Lua-mode Hyprland never reads — writing them would report success and
    /// change nothing. Modules branch on this instead of guessing from paths.
    pub fn hypr_lua_mode(&self) -> bool {
        self.config_provider.eq_ignore_ascii_case("lua")
    }
}

/// `hyprctl version` first line: `Hyprland 0.55.2 built from branch …`.
/// Tagged builds may render as `Hyprland v0.55.2 …`; tolerate the `v`.
fn parse_hyprland_version(stdout: &str) -> Option<String> {
    let first = stdout.lines().next()?;
    let word = first.split_whitespace().nth(1)?;
    let version = word.trim_start_matches('v');
    if version.chars().next()?.is_ascii_digit() {
        Some(version.to_string())
    } else {
        None
    }
}

/// Parse the JSON stanzas `hyprctl -j --batch "getoption …"` emits into
/// `colon:key -> Studio's string form`.
///
/// Each stanza names the option and carries exactly one typed field, so the
/// type is discovered rather than assumed. Gap options come back as a `css`
/// quad ("5 5 5 5"); Studio edits them as a single number, so the first
/// component is taken.
pub fn parse_getoptions(stdout: &str) -> std::collections::BTreeMap<String, String> {
    use crate::configfs::jsonc::JsoncDoc;

    let mut out = std::collections::BTreeMap::new();
    for stanza in stdout.split("\n\n") {
        let stanza = stanza.trim();
        if stanza.is_empty() {
            continue;
        }
        let Ok(doc) = JsoncDoc::parse(stanza) else {
            continue;
        };
        let Some(name) = doc.get("option").map(|v| unquote(&v)) else {
            continue;
        };
        let value = if let Some(v) = doc.get("int") {
            v.trim().to_string()
        } else if let Some(v) = doc.get("bool") {
            v.trim().to_string()
        } else if let Some(v) = doc.get("float") {
            // `0.500000` → `0.5`, matching what Kind::validate produces.
            match v.trim().parse::<f64>() {
                Ok(f) => format!("{f}"),
                Err(_) => v.trim().to_string(),
            }
        } else if let Some(v) = doc.get("str") {
            unquote(&v)
        } else if let Some(v) = doc.get("css") {
            unquote(&v)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            continue;
        };
        if !value.is_empty() {
            out.insert(name, value);
        }
    }
    out
}

fn unquote(raw: &str) -> String {
    let t = raw.trim();
    t.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(t)
        .to_string()
}

/// Which config dialect Studio reads and writes for Hyprland.
///
/// Omarchy 4 ("Quattro") switched the compositor to Lua; before it, hyprlang.
/// The two are not interchangeable — a `.conf` file is simply never read by a
/// Lua-mode Hyprland — so every module that writes Hyprland config resolves
/// this first and picks its file, comment style and renderer from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// `key = value` lines in `~/.config/hypr/*.conf` (Omarchy ≤ 3.x).
    Hyprlang,
    /// `hl.config({…})` calls in `~/.config/hypr/*.lua` (Omarchy 4+).
    Lua,
}

impl Dialect {
    /// Ask the running compositor which dialect it loaded. Falls back to
    /// hyprlang when it can't be determined (no session, or a Hyprland too old
    /// to report it) — the conservative choice, since that is what every
    /// Omarchy before 4 used.
    pub fn probe(runner: &dyn CommandRunner) -> Dialect {
        let provider = runner
            .run(&Cmd::new("hyprctl").arg("systeminfo"))
            .ok()
            .filter(|o| o.ok())
            .and_then(|o| parse_config_provider(&o.stdout))
            .unwrap_or_default();
        if provider.eq_ignore_ascii_case("lua") {
            Dialect::Lua
        } else {
            Dialect::Hyprlang
        }
    }

    pub fn is_lua(self) -> bool {
        self == Dialect::Lua
    }

    /// The extension of the user config files this dialect is written to.
    pub fn ext(self) -> &'static str {
        match self {
            Dialect::Hyprlang => "conf",
            Dialect::Lua => "lua",
        }
    }

    /// The comment syntax used to delimit Studio's managed blocks.
    pub fn comment_style(self) -> crate::configfs::CommentStyle {
        match self {
            Dialect::Hyprlang => crate::configfs::CommentStyle::Hash,
            Dialect::Lua => crate::configfs::CommentStyle::DashDash,
        }
    }
}

/// `hyprctl systeminfo` reports the loaded dialect on one line:
/// `configProvider: lua`. Absent on Hyprland versions that predate it.
fn parse_config_provider(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("configProvider:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Components with an `omarchy-restart-*` primitive (spec 02 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Waybar,
    Mako,
    Swayosd,
    Hypridle,
    Walker,
    Terminal,
    Btop,
}

impl Component {
    /// The matching `omarchy-restart-*` bin — always wrapped, never reimplemented.
    pub fn restart_bin(self) -> &'static str {
        match self {
            Component::Waybar => "omarchy-restart-waybar",
            Component::Mako => "omarchy-restart-mako",
            Component::Swayosd => "omarchy-restart-swayosd",
            Component::Hypridle => "omarchy-restart-hypridle",
            Component::Walker => "omarchy-restart-walker",
            Component::Terminal => "omarchy-restart-terminal",
            Component::Btop => "omarchy-restart-btop",
        }
    }

    /// Process name for the post-restart alive check, where one applies.
    /// The binary this component *is*, for a plain presence check.
    ///
    /// Omarchy 4 dropped Waybar, Mako, SwayOSD, hypridle and Walker; the shell
    /// (Quickshell) took over the bar, notifications, the OSD, idle and the
    /// launcher. When the binary is absent, editing its config file writes
    /// something nothing will ever read — so the modules that own these say so
    /// instead.
    pub fn binary(self) -> &'static str {
        match self {
            Component::Waybar => "waybar",
            Component::Mako => "mako",
            Component::Swayosd => "swayosd-server",
            Component::Hypridle => "hypridle",
            Component::Walker => "walker",
            Component::Terminal => "foot",
            Component::Btop => "btop",
        }
    }

    /// Is this component installed at all?
    pub fn present(self) -> bool {
        find_in_path(self.binary()).is_some()
    }

    /// A one-line explanation for a component Omarchy 4 replaced, or `None`
    /// when it is installed and editable as usual.
    pub fn unavailable_reason(self) -> Option<String> {
        if self.present() {
            return None;
        }
        let replacement = match self {
            Component::Waybar => "the Omarchy shell's bar (~/.config/omarchy/shell.json)",
            Component::Mako => "the Omarchy shell's notifications",
            Component::Swayosd => "the Omarchy shell's on-screen display",
            Component::Hypridle => "the Omarchy shell's idle handling",
            Component::Walker => "the Omarchy shell's launcher",
            _ => return Some(format!("{} isn't installed.", self.binary())),
        };
        Some(format!(
            "{} isn't installed — Omarchy 4 replaced it with {replacement}. \
             Studio can't edit it here, and writing its old config file would \
             change nothing.",
            self.binary()
        ))
    }

    pub fn process_name(self) -> Option<&'static str> {
        match self {
            Component::Waybar => Some("waybar"),
            Component::Mako => Some("mako"),
            _ => None,
        }
    }
}

/// Restart `c` to load a config change, but never *spawn* a component the user
/// isn't running. `omarchy-restart-*` primitives kill-and-relaunch, so calling
/// one for a component that's absent would start it fresh — e.g. launching
/// Waybar on top of a user's alternative bar (a Quickshell shell). For any
/// component with a known process name we first check it's alive and skip the
/// restart otherwise; components without one (pure reload primitives) always
/// run. Returns whether a restart was actually issued.
pub fn restart_if_running(runner: &dyn CommandRunner, c: Component) -> crate::error::Result<bool> {
    if let Some(proc) = c.process_name() {
        let alive = runner
            .run(&cmds::process_alive(proc))
            .map(|o| o.ok())
            .unwrap_or(false);
        if !alive {
            return Ok(false);
        }
    }
    let out = runner.run(&cmds::restart(c))?;
    if !out.ok() {
        return Err(crate::error::StudioError::External {
            cmd: c.restart_bin().into(),
            detail: out.stderr.trim().to_string(),
        });
    }
    Ok(true)
}

/// Typed constructors for the reload primitives (spec 02 §2). Keeping the
/// command names here — not in `engine` — preserves the "only this module
/// knows Omarchy commands" rule.
pub mod cmds {
    use super::Component;
    use crate::cmd::Cmd;

    pub fn restart(c: Component) -> Cmd {
        Cmd::new(c.restart_bin())
    }

    pub fn hypr_reload() -> Cmd {
        Cmd::new("hyprctl").arg("reload")
    }

    pub fn hypr_configerrors() -> Cmd {
        Cmd::new("hyprctl").arg("configerrors")
    }

    /// Effective keybinds as JSON — ground truth for the keybinds screen.
    pub fn binds_json() -> Cmd {
        Cmd::new("hyprctl").arg("binds").arg("-j")
    }

    /// Plain-text `hyprctl binds`. Needed because Hyprland 0.56's `-j` writer
    /// emits malformed JSON (keys and values misaligned, unquoted values), so
    /// the text form is the only readable keymap on that version.
    pub fn binds_text() -> Cmd {
        Cmd::new("hyprctl").arg("binds")
    }

    /// Read many options' *effective* values in one exec, as JSON stanzas.
    /// `names` are Hyprland's colon form (`decoration:blur:size`).
    ///
    /// This is how Studio reads current values on Omarchy 4: the values live in
    /// Lua now, and rather than parse a Turing-complete config we ask the
    /// compositor what it actually ended up with. Correct on either dialect.
    pub fn hypr_getoptions(names: &[String]) -> Cmd {
        let script = names
            .iter()
            .map(|n| format!("getoption {n}"))
            .collect::<Vec<_>>()
            .join(" ; ");
        Cmd::new("hyprctl").arg("-j").arg("--batch").arg(script)
    }

    /// Apply a single setting live, without writing any file — the look & feel
    /// preview primitive. `name` is Hyprland's colon form, e.g.
    /// `decoration:blur:size`. Lost on the next reload (that's the point).
    pub fn hypr_keyword(name: &str, value: &str) -> Cmd {
        Cmd::new("hyprctl").arg("keyword").arg(name).arg(value)
    }

    // ── toggles (spec 05 §2) ─────────────────────────────────────────────────

    /// Is a persistent Hyprland flag toggle currently enabled (its state file
    /// present)? Exit 0 = present.
    pub fn hypr_toggle_enabled(flag: &str) -> Cmd {
        Cmd::new("omarchy-hyprland-toggle-enabled").arg(flag)
    }

    pub fn toggle_window_gaps() -> Cmd {
        Cmd::new("omarchy-hyprland-window-gaps-toggle")
    }
    pub fn toggle_square_aspect() -> Cmd {
        Cmd::new("omarchy-hyprland-window-single-square-aspect-toggle")
    }
    pub fn toggle_nightlight() -> Cmd {
        Cmd::new("omarchy-toggle-nightlight")
    }
    pub fn toggle_idle() -> Cmd {
        Cmd::new("omarchy-toggle-idle")
    }
    pub fn toggle_waybar() -> Cmd {
        Cmd::new("omarchy-toggle-waybar")
    }
    pub fn toggle_notification_silencing() -> Cmd {
        Cmd::new("omarchy-toggle-notification-silencing")
    }

    pub fn theme_refresh() -> Cmd {
        Cmd::new("omarchy-theme-refresh")
    }

    /// Idempotent full theme apply — Omarchy's own pipeline, never ours.
    pub fn theme_set(slug: &str) -> Cmd {
        Cmd::new("omarchy-theme-set").arg(slug)
    }

    /// Clone a theme repo into the user themes dir **and apply it** —
    /// `omarchy-theme-install` does both. The generous timeout covers the
    /// git clone plus the full theme-set pipeline it runs at the end.
    pub fn theme_install(url: &str) -> Cmd {
        Cmd::new("omarchy-theme-install")
            .arg(url)
            .timeout(std::time::Duration::from_secs(120))
    }

    /// Re-apply current theme without touching the wallpaper (env guard).
    pub fn theme_set_keep_bg(slug: &str) -> Cmd {
        Cmd::new("omarchy-theme-set")
            .arg(slug)
            .env("OMARCHY_THEME_SKIP_BACKGROUND", "1")
    }

    /// Set the wallpaper — Omarchy's own script handles image/gif/video
    /// dispatch (swaybg/swww/mpvpaper) and the current/background link.
    pub fn theme_bg_set(path: &std::path::Path) -> Cmd {
        Cmd::new("omarchy-theme-bg-set").arg(path.to_string_lossy())
    }

    /// Cycle to the next background for the current theme.
    pub fn theme_bg_next() -> Cmd {
        Cmd::new("omarchy-theme-bg-next")
    }

    pub fn makoctl_reload() -> Cmd {
        Cmd::new("makoctl").arg("reload")
    }

    /// List the notification daemon's currently active modes (one per line).
    pub fn makoctl_mode_list() -> Cmd {
        Cmd::new("makoctl").arg("mode")
    }

    /// Add a mode (e.g. `do-not-disturb`).
    pub fn makoctl_mode_add(mode: &str) -> Cmd {
        Cmd::new("makoctl").arg("mode").arg("-a").arg(mode)
    }

    /// Remove a mode.
    pub fn makoctl_mode_remove(mode: &str) -> Cmd {
        Cmd::new("makoctl").arg("mode").arg("-r").arg(mode)
    }

    /// Flash the SwayOSD popup without changing anything — a `raise 0` no-op
    /// bump, used as the OSD self-test.
    pub fn swayosd_selftest() -> Cmd {
        Cmd::new("swayosd-client")
            .arg("--output-volume")
            .arg("raise")
            .arg("0")
    }

    /// Fire a sample notification (used by the live-test rows).
    pub fn notify_send(urgency: &str, summary: &str, body: &str) -> Cmd {
        Cmd::new("notify-send")
            .arg("-u")
            .arg(urgency)
            .arg(summary)
            .arg(body)
    }

    pub fn process_alive(name: &str) -> Cmd {
        Cmd::new("pgrep").arg("-x").arg(name)
    }

    // ── apps & services (spec 0.8.3) ─────────────────────────────────────────

    /// Remove one or more Omarchy web-app launchers by name (deletes the
    /// `.desktop` + icon and restarts walker). Non-interactive when names are
    /// passed.
    pub fn webapp_remove(names: &[String]) -> Cmd {
        Cmd::new("omarchy-webapp-remove").args(names.iter().cloned())
    }

    // ── Omarchy 4 shell: bar, plugins, idle (roadmap O4.5) ──────────────────

    /// Ask the running `omarchy-shell` to reread `shell.json`. The bar and
    /// menu already hot-reload on save; this is for callers (like Studio) that
    /// want to be sure a write landed rather than relying on the file watcher.
    pub fn shell_reload() -> Cmd {
        Cmd::new("omarchy-shell").arg("shell").arg("reloadConfig")
    }

    /// Fallback when `reloadConfig` isn't wired up in this build — rescans
    /// plugin code, which also re-reads config. Mirrors what
    /// `omarchy-shell-config`'s own `refresh_shell_config` helper does.
    pub fn shell_rescan_plugins() -> Cmd {
        Cmd::new("omarchy-shell")
            .arg("-q")
            .arg("shell")
            .arg("rescanPlugins")
    }

    /// Every discovered shell plugin (bar widgets, services, panels), with
    /// enabled/active state. The `omarchy plugin` group's own catalog — reuse
    /// it rather than re-deriving one from `shell/plugins/**/manifest.json`.
    pub fn plugin_list_json() -> Cmd {
        Cmd::new("omarchy").arg("plugin").arg("list").arg("--json")
    }

    pub fn plugin_enable(id: &str, placement: &[String]) -> Cmd {
        Cmd::new("omarchy")
            .arg("plugin")
            .arg("enable")
            .arg(id)
            .args(placement.iter().cloned())
    }

    pub fn plugin_disable(id: &str) -> Cmd {
        Cmd::new("omarchy").arg("plugin").arg("disable").arg(id)
    }

    /// `omarchy bar move|put` — reorder or place a widget. `placement` is the
    /// already-rendered `--section/--index/--before/--after/…` flag pairs
    /// (see `modules::shell::Placement::to_args`).
    pub fn bar_move(id: &str, placement: &[String]) -> Cmd {
        Cmd::new("omarchy")
            .arg("bar")
            .arg("move")
            .arg(id)
            .args(placement.iter().cloned())
    }

    pub fn bar_put(id: &str, placement: &[String]) -> Cmd {
        Cmd::new("omarchy")
            .arg("bar")
            .arg("put")
            .arg(id)
            .args(placement.iter().cloned())
    }

    /// A per-widget setting, e.g. `omarchy bar set omarchy.clock format HH:mm`.
    pub fn bar_set(id: &str, key: &str, value: &str, as_json: bool, placement: &[String]) -> Cmd {
        let mut c = Cmd::new("omarchy")
            .arg("bar")
            .arg("set")
            .arg(id)
            .arg(key)
            .arg(value);
        if as_json {
            c = c.arg("--json");
        }
        c.args(placement.iter().cloned())
    }

    pub fn bar_position(position: &str) -> Cmd {
        Cmd::new("omarchy").arg("bar").arg("position").arg(position)
    }

    /// `true` / `false` / `toggle`.
    pub fn bar_transparent(value: &str) -> Cmd {
        Cmd::new("omarchy").arg("bar").arg("transparent").arg(value)
    }

    /// Restore the default bar and service widgets — `omarchy bar defaults`.
    pub fn bar_defaults() -> Cmd {
        Cmd::new("omarchy").arg("bar").arg("defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::StubRunner;

    /// Unique throwaway dir; std-only stand-in for tempfile (the scaffold
    /// stays dependency-free, spec 01 §6).
    fn scratch_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-test-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_omarchy(tag: &str) -> OmarchyPaths {
        let root = scratch_dir(tag);
        let system = root.join("share/omarchy");
        std::fs::create_dir_all(system.join("default/themed")).unwrap();
        std::fs::create_dir_all(system.join("default/hypr/apps")).unwrap();
        std::fs::create_dir_all(system.join("bin")).unwrap();
        std::fs::write(system.join("version"), "3.8.2\n").unwrap();
        std::fs::write(system.join("bin/omarchy-hook"), "#!/bin/bash\n").unwrap();
        std::fs::write(
            system.join("default/hypr/apps/system.conf"),
            "windowrule = tag +floating-window, match:class (org.omarchy.btop|TUI.float)\n",
        )
        .unwrap();
        OmarchyPaths {
            system,
            config: root.join("config/omarchy"),
            state: root.join("state/omarchy"),
        }
    }

    fn git_status_display(paths: &OmarchyPaths) -> String {
        format!("git -C {} status --porcelain", paths.system.display())
    }

    #[test]
    fn probes_a_healthy_install() {
        let paths = fake_omarchy("healthy");
        let stub = StubRunner::default()
            .with_ok(
                "hyprctl version",
                "Hyprland 0.55.2 built from branch v0.55.2 at commit abc\n",
            )
            .with_ok("hyprctl configerrors", "no errors\n")
            .with_ok(git_status_display(&paths), "");

        let caps = Capabilities::probe(&paths, &stub);
        assert_eq!(caps.omarchy_version, "3.8.2");
        assert_eq!(caps.hyprland_version, "0.55.2");
        assert!(caps.has_configerrors);
        assert!(caps.has_templates);
        assert!(caps.has_hooks);
        assert!(caps.tui_float_rule);
        assert!(!caps.omarchy_dirty);
        // No systeminfo scripted → unknown provider, and we must not guess Lua.
        assert_eq!(caps.config_provider, "");
        assert!(!caps.hypr_lua_mode());
    }

    /// An Omarchy 4 install: Lua defaults, and a compositor reporting the Lua
    /// config provider.
    fn fake_omarchy_quattro(tag: &str) -> OmarchyPaths {
        let paths = fake_omarchy(tag);
        std::fs::remove_file(paths.system.join("default/hypr/apps/system.conf")).unwrap();
        std::fs::write(paths.system.join("version"), "4.0.0\n").unwrap();
        std::fs::write(
            paths.system.join("default/hypr/apps/system.lua"),
            "o.window(\"(org.omarchy.terminal|TUI.float)\", { float = true })\n",
        )
        .unwrap();
        paths
    }

    #[test]
    fn detects_lua_mode_and_finds_the_float_rule_in_its_new_home() {
        let paths = fake_omarchy_quattro("quattro");
        let stub = StubRunner::default()
            .with_ok(
                "hyprctl version",
                "Hyprland 0.56.2 built from branch v0.56.2 at commit abc\n",
            )
            .with_ok(
                "hyprctl systeminfo",
                "Hyprland 0.56.2\nconfigProvider: lua\n\nLibraries:\n",
            )
            .with_ok("hyprctl configerrors", "")
            .with_ok(git_status_display(&paths), "");

        let caps = Capabilities::probe(&paths, &stub);
        assert_eq!(caps.config_provider, "lua");
        assert!(caps.hypr_lua_mode());
        assert!(
            caps.tui_float_rule,
            "the rule lives in apps/system.lua on Omarchy 4"
        );
    }

    /// Verbatim `hyprctl -j --batch "getoption …"` output from a Hyprland
    /// 0.56.2 / Omarchy 4 session — including the css-quad form gaps use and
    /// an option the user hasn't set.
    const REAL_GETOPTIONS: &str = r#"{"option": "general:gaps_in", "css": "5 5 5 5", "set": true }


{"option": "decoration:rounding", "int": 0, "set": true }


{"option": "general:layout", "str": "dwindle", "set": true }


{"option": "decoration:dim_strength", "float": 0.500000, "set": false }


{"option": "general:resize_on_border", "bool": false, "set": true }
"#;

    #[test]
    fn parses_real_batch_getoption_output() {
        let got = parse_getoptions(REAL_GETOPTIONS);
        assert_eq!(got.get("general:gaps_in").map(String::as_str), Some("5"));
        assert_eq!(
            got.get("decoration:rounding").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            got.get("general:layout").map(String::as_str),
            Some("dwindle")
        );
        // 0.500000 normalises to the form Kind::validate would produce.
        assert_eq!(
            got.get("decoration:dim_strength").map(String::as_str),
            Some("0.5")
        );
        assert_eq!(
            got.get("general:resize_on_border").map(String::as_str),
            Some("false")
        );
        assert_eq!(got.len(), 5);
    }

    #[test]
    fn getoption_parser_skips_junk_without_panicking() {
        assert!(parse_getoptions("").is_empty());
        assert!(parse_getoptions("not json at all").is_empty());
        // A stanza with no recognised typed field contributes nothing.
        assert!(parse_getoptions(r#"{"option": "x:y", "set": false }"#).is_empty());
    }

    #[test]
    fn builds_a_single_batch_command_for_many_options() {
        let cmd = cmds::hypr_getoptions(&[
            "general:gaps_in".to_string(),
            "decoration:rounding".to_string(),
        ]);
        assert_eq!(
            cmd.display(),
            "hyprctl -j --batch getoption general:gaps_in ; getoption decoration:rounding"
        );
    }

    /// A component Omarchy 4 dropped must explain itself rather than let a
    /// module write a config file nothing reads.
    #[test]
    fn a_replaced_component_names_what_took_over() {
        // `present()` reads the real PATH, so assert on the message shape for
        // whichever side this machine is on.
        for (c, expect) in [
            (Component::Waybar, "shell.json"),
            (Component::Mako, "notifications"),
            (Component::Swayosd, "on-screen display"),
        ] {
            match c.unavailable_reason() {
                Some(msg) => {
                    assert!(msg.contains(c.binary()), "names the binary: {msg}");
                    assert!(msg.contains(expect), "names the replacement: {msg}");
                }
                None => assert!(c.present(), "no reason given, so it must be installed"),
            }
        }
    }

    #[test]
    fn hyprlang_provider_is_not_lua_mode() {
        assert_eq!(
            parse_config_provider("Hyprland 0.55\nconfigProvider: hyprlang\n").as_deref(),
            Some("hyprlang")
        );
        let caps = Capabilities {
            config_provider: "hyprlang".into(),
            ..Default::default()
        };
        assert!(!caps.hypr_lua_mode());
        // A Hyprland too old to report the line leaves it empty.
        assert_eq!(parse_config_provider("Hyprland 0.44\n"), None);
    }

    #[test]
    fn flags_dirty_checkout_and_survives_missing_hyprctl() {
        let paths = fake_omarchy("dirty");
        // hyprctl unscripted → StubRunner errors → probe degrades gracefully.
        let stub = StubRunner::default()
            .with_ok(git_status_display(&paths), " M bin/omarchy-theme-bg-next\n");

        let caps = Capabilities::probe(&paths, &stub);
        assert_eq!(caps.hyprland_version, "");
        assert!(!caps.has_configerrors);
        assert!(caps.omarchy_dirty);
    }

    #[test]
    fn absent_install_probes_empty_not_panicky() {
        let root = scratch_dir("absent");
        let paths = OmarchyPaths {
            system: root.join("nope"),
            config: root.join("config"),
            state: root.join("state"),
        };
        let stub = StubRunner::default();
        let caps = Capabilities::probe(&paths, &stub);
        assert!(!paths.installed());
        assert_eq!(caps.omarchy_version, "");
        assert!(!caps.has_templates && !caps.has_hooks && !caps.tui_float_rule);
    }

    #[test]
    fn parses_hyprland_version_variants() {
        assert_eq!(
            parse_hyprland_version("Hyprland 0.55.2 built from branch\nDate: x").as_deref(),
            Some("0.55.2")
        );
        assert_eq!(
            parse_hyprland_version("Hyprland v0.55.2 built…").as_deref(),
            Some("0.55.2")
        );
        assert_eq!(parse_hyprland_version("garbage"), None);
        assert_eq!(parse_hyprland_version(""), None);
    }

    #[test]
    fn reads_current_theme_name() {
        let paths = fake_omarchy("theme");
        std::fs::create_dir_all(paths.config.join("current")).unwrap();
        std::fs::write(paths.config.join("current/theme.name"), "osaka-jade\n").unwrap();
        assert_eq!(paths.current_theme_name().unwrap(), "osaka-jade");
    }

    #[test]
    fn restart_if_running_never_spawns_an_absent_bar() {
        use crate::cmd::CmdOutput;
        let alive = cmds::process_alive("waybar").display();
        let restart = cmds::restart(Component::Waybar).display();

        // Waybar not running (a Quickshell user): no restart is issued, so
        // nothing is spawned on top of their real bar.
        let runner = StubRunner::default().with(
            &alive,
            CmdOutput {
                status: 1,
                ..Default::default()
            },
        );
        assert!(!restart_if_running(&runner, Component::Waybar).unwrap());
        assert!(
            !runner.calls().contains(&restart),
            "must not restart/spawn Waybar when it isn't running"
        );

        // Waybar running: the reload goes through.
        let runner = StubRunner::default()
            .with(&alive, CmdOutput::default())
            .with(&restart, CmdOutput::default());
        assert!(restart_if_running(&runner, Component::Waybar).unwrap());
        assert!(runner.calls().contains(&restart));

        // A pure reload primitive (no process to check) always runs.
        let walker = cmds::restart(Component::Walker).display();
        let runner = StubRunner::default().with(&walker, CmdOutput::default());
        assert!(restart_if_running(&runner, Component::Walker).unwrap());
    }
}

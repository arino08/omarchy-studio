//! Nice Launcher — the Quickshell app-search overlay (github arino08/nova).
//!
//! Nice Launcher is spawned per-invocation from a keybind and reads its whole
//! config at launch from `~/.config/nova/nova.json`, so "apply" here is just an
//! atomic save — there is no daemon to restart and the next launch picks it up.
//! Theme-sync is free: Nice Launcher reads the active Omarchy `colors.toml`.
//!
//! Studio owns three things:
//!   * the JSON config (visual mode, providers, backdrop, animation toggles) —
//!     unknown keys round-trip untouched so a newer version never loses fields;
//!   * an optional launch keybind, written as a `bindd` line through the
//!     keybinds override block (same managed block, same undo story);
//!   * install + launch detection — `nice-launcher` on PATH via XDG install.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cmd::CommandRunner;
use crate::configfs::atomic_write;
use crate::error::{Result, StudioError};
use crate::modules::keybinds::{self, ConfigBind};
use crate::omarchy::OmarchyPaths;

/// The four visual modes, in Nice Launcher's own cycle order.
pub const MODES: [&str; 4] = ["constellation", "spotlight", "orbital", "grid"];

/// Elephant providers Nice Launcher's default config searches. `symbols`/
/// `unicode` exist but are excluded from the defaults (they bury real hits).
pub const KNOWN_PROVIDERS: [&str; 10] = [
    "desktopapplications",
    "calc",
    "files",
    "clipboard",
    "websearch",
    "menus",
    "runner",
    "todo",
    "symbols",
    "unicode",
];

/// Marker description on the managed keybind line (how we find ours again).
pub const BIND_DESC: &str = "Nice Launcher";

/// GitHub repo for the Nice Launcher project.
pub const REPO_URL: &str = "https://github.com/arino08/nova.git";

/// Binary name installed to `~/.local/bin/`.
const BIN_NAME: &str = "nice-launcher";

/// Data directory name under `~/.local/share/`.
const DATA_DIR_NAME: &str = "nice-launcher";

fn config_json(paths: &OmarchyPaths) -> PathBuf {
    paths
        .config
        .parent()
        .map(|c| c.join("nova/nova.json"))
        .unwrap_or_else(|| PathBuf::from("nova/nova.json"))
}

/// The `anim` block — every toggle Nova's modes read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnimCfg {
    pub stagger: bool,
    pub stagger_ms: i64,
    pub living_background: bool,
    pub spring: bool,
    pub micro: bool,
    /// Keys a newer Nova knows and this Studio doesn't — preserved verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for AnimCfg {
    fn default() -> Self {
        Self {
            stagger: true,
            stagger_ms: 26,
            living_background: true,
            spring: true,
            micro: true,
            extra: serde_json::Map::new(),
        }
    }
}

/// `nova.json`, defaults matching Nice Launcher's bundled `config/nova.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NovaConfig {
    pub mode: String,
    pub providers: Vec<String>,
    pub limit: i64,
    pub backdrop_opacity: f64,
    pub blur: bool,
    pub accent_glow: bool,
    pub theme: String,
    pub anim: AnimCfg,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for NovaConfig {
    fn default() -> Self {
        Self {
            mode: "constellation".into(),
            providers: [
                "desktopapplications",
                "calc",
                "files",
                "clipboard",
                "websearch",
                "menus",
                "runner",
                "todo",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            limit: 40,
            backdrop_opacity: 0.985,
            blur: true,
            accent_glow: true,
            theme: "auto".into(),
            anim: AnimCfg::default(),
            extra: serde_json::Map::new(),
        }
    }
}

/// The Nice Launcher config file, loaded (or defaulted) and editable in place.
#[derive(Debug, Clone, PartialEq)]
pub struct Nova {
    path: PathBuf,
    /// Whether nova.json existed on load (absent file → defaults, no error).
    pub existed: bool,
    pub cfg: NovaConfig,
}

impl Nova {
    pub fn load(paths: &OmarchyPaths) -> Self {
        let path = config_json(paths);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let existed = !text.is_empty();
        let cfg = serde_json::from_str(&text).unwrap_or_default();
        Self { path, existed, cfg }
    }

    pub fn config_path(&self) -> &Path {
        &self.path
    }

    /// Write nova.json (pretty, trailing newline). Takes effect on Nice
    /// Launcher's next launch — nothing to restart. Caller snapshots first.
    pub fn save(&self) -> Result<()> {
        let json =
            serde_json::to_string_pretty(&self.cfg).map_err(|e| StudioError::ParseFailed {
                file: self.path.clone(),
                line: None,
                hint: e.to_string(),
            })?;
        atomic_write(&self.path, &format!("{json}\n"))
    }
}

/// The command that launches Nice Launcher. Looks for `nice-launcher` on
/// PATH (the XDG install puts it at `~/.local/bin/nice-launcher`). Returns
/// `None` when Nice Launcher isn't installed.
pub fn launch_command(runner: &dyn CommandRunner) -> Option<String> {
    // Check PATH for the installed binary.
    let cmd = crate::cmd::Cmd::new("which").arg(BIN_NAME);
    if let Ok(out) = runner.run(&cmd) {
        let path = out.stdout.trim().to_string();
        if out.ok() && !path.is_empty() {
            return Some(path);
        }
    }
    None
}

/// Install directory for the QML application data.
fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join(DATA_DIR_NAME)
}

/// Install path for the launch script.
fn bin_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".local/bin")
        .join(BIN_NAME)
}

/// Whether Nice Launcher is installed (the launch script exists on disk).
pub fn is_installed() -> bool {
    bin_path().is_file()
}

/// Install Nice Launcher: clone the repo to a temp dir, copy QML files +
/// assets + config to `~/.local/share/nice-launcher/`, write a launch script
/// to `~/.local/bin/nice-launcher`. Returns the install path on success.
pub fn install(runner: &dyn CommandRunner) -> Result<PathBuf> {
    let data = data_dir();
    let bin = bin_path();

    // Clone into a temp directory.
    let tmp = std::env::temp_dir().join(format!("nice-launcher-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let clone = crate::cmd::Cmd::new("git")
        .arg("clone")
        .arg("--depth=1")
        .arg(REPO_URL)
        .arg(tmp.display().to_string());
    let out = runner.run(&clone).map_err(|_| StudioError::External {
        cmd: "git clone".into(),
        detail: "failed to clone the Nice Launcher repository — is git installed?".into(),
    })?;
    if !out.ok() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(StudioError::External {
            cmd: "git clone".into(),
            detail: out.stderr.trim().to_string(),
        });
    }

    // Create destination dirs.
    std::fs::create_dir_all(&data)?;
    if let Some(parent) = bin.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Copy application files (shell.qml, qml/, config/, assets/).
    for entry in ["shell.qml", "qml", "config", "assets"] {
        let src = tmp.join(entry);
        if src.exists() {
            copy_recursive(&src, &data.join(entry))?;
        }
    }

    // Write the launch script.
    let script = format!(
        "#!/usr/bin/env bash\n\
         # Nice Launcher — launch script (installed by omarchy-studio)\n\
         set -euo pipefail\n\
         exec qs -n -p {:?}\n",
        data
    );
    std::fs::write(&bin, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    }

    // Clean up temp dir.
    let _ = std::fs::remove_dir_all(&tmp);

    Ok(bin)
}

/// Remove the Nice Launcher installation.
pub fn uninstall() -> Result<bool> {
    let data = data_dir();
    let bin = bin_path();
    let existed = bin.is_file() || data.is_dir();
    if bin.is_file() {
        std::fs::remove_file(&bin)?;
    }
    if data.is_dir() {
        std::fs::remove_dir_all(&data)?;
    }
    Ok(existed)
}

/// Recursively copy a file or directory tree.
fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name();
            // Skip hidden dirs like .git
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            copy_recursive(&entry.path(), &dst.join(name))?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Studio's Nice Launcher bind from the override block, if installed.
pub fn keybind(paths: &OmarchyPaths) -> Option<ConfigBind> {
    keybinds::find_marked(paths, BIND_DESC)
}

/// Bind `mods+key` to launch Nice Launcher (replacing any previous bind),
/// through the shared keybinds override block — sourced last, so it wins over
/// the Omarchy default on the same chord (e.g. SUPER+SPACE's Walker). Reloads
/// Hyprland through the apply pipeline, which snapshots and rolls back.
pub fn install_keybind(
    paths: &OmarchyPaths,
    mods: &str,
    key: &str,
    exec: &str,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<ConfigBind> {
    keybinds::install_marked(paths, BIND_DESC, mods, key, exec, store, runner)
}

/// Remove the Nice Launcher bind from the override block (other overrides
/// survive). Returns false when none was installed. The pipeline snapshots.
pub fn remove_keybind(
    paths: &OmarchyPaths,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<bool> {
    keybinds::remove_marked(paths, BIND_DESC, store, runner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::StubRunner;
    use crate::modules::keybinds::Override;

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-nova-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/omarchy")).unwrap();
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("sys/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    #[test]
    fn missing_file_loads_bundled_defaults() {
        let paths = fake_paths("defaults");
        let nova = Nova::load(&paths);
        assert!(!nova.existed);
        assert_eq!(nova.cfg.mode, "constellation");
        assert_eq!(nova.cfg.limit, 40);
        assert!(nova.cfg.anim.spring);
        assert_eq!(nova.cfg.providers.len(), 8);
    }

    #[test]
    fn save_round_trips_and_preserves_unknown_keys() {
        let paths = fake_paths("roundtrip");
        std::fs::create_dir_all(config_json(&paths).parent().unwrap()).unwrap();
        std::fs::write(
            config_json(&paths),
            r#"{ "mode": "grid", "futureKnob": 7, "anim": { "spring": false, "wobble": true } }"#,
        )
        .unwrap();

        let mut nova = Nova::load(&paths);
        assert!(nova.existed);
        assert_eq!(nova.cfg.mode, "grid");
        assert!(!nova.cfg.anim.spring);
        // unspecified fields fall back to defaults
        assert_eq!(nova.cfg.limit, 40);

        nova.cfg.mode = "orbital".into();
        nova.cfg.backdrop_opacity = 0.9;
        nova.save().unwrap();

        let text = std::fs::read_to_string(config_json(&paths)).unwrap();
        assert!(text.contains("\"orbital\""));
        // unknown keys survive the rewrite
        assert!(text.contains("futureKnob"));
        assert!(text.contains("wobble"));
        // camelCase field names on disk (what Nova's QML reads)
        assert!(text.contains("backdropOpacity"));
        assert!(text.contains("livingBackground"));

        let re = Nova::load(&paths);
        assert_eq!(re.cfg.mode, "orbital");
        assert_eq!(re.cfg.backdrop_opacity, 0.9);
    }

    #[test]
    fn save_creates_the_config_dir_first_run() {
        let paths = fake_paths("firstrun");
        let nova = Nova::load(&paths);
        assert!(!nova.existed);
        nova.save().unwrap();
        assert!(config_json(&paths).is_file());
    }

    #[test]
    fn keybind_installs_updates_and_removes() {
        let paths = fake_paths("bind");
        let runner = StubRunner::default().with_ok("hyprctl reload", "");
        // The pipeline snapshots with real git; the stub covers hyprctl.
        let store = crate::snapshot::SnapshotStore::open_or_init(
            paths.config.join("history"),
            Box::new(crate::cmd::RealRunner),
        )
        .unwrap();

        assert!(keybind(&paths).is_none());
        let bind = install_keybind(&paths, "SUPER", "SPACE", "/x/nova", &store, &runner).unwrap();
        assert_eq!(
            bind.render_line(),
            "bindd = SUPER, SPACE, Nice Launcher, exec, /x/nova"
        );

        let found = keybind(&paths).expect("bind installed");
        assert_eq!(found.arg, "/x/nova");

        // reinstall with a new chord replaces, not duplicates
        install_keybind(&paths, "SUPER SHIFT", "N", "/x/nova", &store, &runner).unwrap();
        let binds: Vec<_> = keybinds::read_overrides(&paths);
        let nova_binds = binds
            .iter()
            .filter(
                |o| matches!(o, Override::Set(cb) if cb.description.as_deref() == Some(BIND_DESC)),
            )
            .count();
        assert_eq!(nova_binds, 1);
        assert_eq!(keybind(&paths).unwrap().key, "N");

        assert!(remove_keybind(&paths, &store, &runner).unwrap());
        assert!(keybind(&paths).is_none());
        assert!(!remove_keybind(&paths, &store, &runner).unwrap());
    }
}

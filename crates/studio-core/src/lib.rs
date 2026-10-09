//! studio-core — engine for Omarchy Studio.
//!
//! Frontends (TUI/CLI, later GUI) live in the `omarchy-studio` crate; this
//! crate never prints, never draws, never prompts (spec 01 §1).
//!
//! Module map (each corresponds to a build spec in `docs/specs/`):
//! - [`cmd`]      — external command runner: timeout/capture/dry-run/stub (spec 02 §2)
//! - [`omarchy`]  — adapter: paths, command wrappers, capability probe (spec 02)
//! - [`configfs`] — dialect parsers/writers, managed blocks, atomic writes (spec 03)
//! - [`engine`]   — settings schema, apply pipeline, verification (spec 01 §2–3)
//! - [`snapshot`] — git-backed history, undo/restore (spec 03 §6)
//! - [`deps`]     — dependency & tool registry, guided-install guidance (spec 06)
//! - [`integration`] — omarchy menu install/uninstall (spec 02 §3)
//! - [`hooks`]    — theme-set/post-update hooks, update-survival flows (spec 02 §4, §6)
//! - [`manifest`] — registry of artifacts Studio installed (spec 02 §6.2)
//! - [`modules`]  — domain logic per PRD module (specs 04–05)

pub mod cmd;
pub mod configfs;
pub mod deps;
pub mod engine;
pub mod error;
pub mod hooks;
pub mod integration;
pub mod manifest;
pub mod modules;
pub mod omarchy;
pub mod snapshot;

pub use error::StudioError;

/// Studio's own version, single source of truth for `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Omarchy versions this build is tested against (spec 10 §2).
///
/// Two majors, because Studio drives both Hyprland config dialects: hyprlang
/// (`~/.config/hypr/*.conf`) on 3.x and Lua (`*.lua`) on 4.x "Quattro". What is
/// *not* ported to 4 is the bar/notification/OSD layer — Waybar, Mako and
/// SwayOSD are gone there, replaced by the Quickshell shell — so those screens
/// report unavailable rather than writing config nothing reads.
pub const TESTED_OMARCHY: &[&str] = &["3.8", "4.0"];

/// The tested versions as a human list, e.g. `3.8 or 4.0`.
pub fn tested_omarchy() -> String {
    match TESTED_OMARCHY {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// How the installed Omarchy relates to the one this build was tested on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionFit {
    /// Same major.minor as one of [`TESTED_OMARCHY`].
    Tested,
    /// A tested major, later minor — expected to work; paths rarely move.
    Newer,
    /// A major Studio has never seen. Omarchy 4 already moved the config
    /// dialect and collapsed Waybar/Mako/Swayosd/Walker into omarchy-shell;
    /// a further major is where assumptions break again.
    Major,
    /// No version file to read.
    Unknown,
}

impl VersionFit {
    /// The one line a frontend should show. `None` when all is well —
    /// silence is the right output for a supported version.
    pub fn warning(&self) -> Option<String> {
        match self {
            VersionFit::Tested | VersionFit::Newer => None,
            VersionFit::Major => Some(format!(
                "this build is tested against Omarchy {} — on a different major version \
                 some paths and commands may have moved. Studio still snapshots every \
                 change, so anything it does here stays undoable.",
                tested_omarchy()
            )),
            VersionFit::Unknown => Some(
                "couldn't read Omarchy's version file — proceeding as if it were \
                 supported; changes stay snapshotted either way."
                    .into(),
            ),
        }
    }
}

/// Compare an installed Omarchy version against [`TESTED_OMARCHY`]. Warns,
/// never blocks: refusing to run on an untested version would make every
/// Omarchy release a brick, and the snapshot store is the real safety net.
pub fn version_fit(installed: &str) -> VersionFit {
    let part = |s: &str, i: usize| -> Option<u32> { s.split('.').nth(i)?.trim().parse().ok() };
    let Some(major) = part(installed, 0) else {
        return VersionFit::Unknown;
    };
    // Best fit across the tested versions: an exact major.minor wins, a tested
    // major with a later minor is next, and no matching major is a warning.
    let mut fit = VersionFit::Major;
    for tested in TESTED_OMARCHY {
        let (Some(want_major), Some(want_minor)) = (part(tested, 0), part(tested, 1)) else {
            continue;
        };
        if major != want_major {
            continue;
        }
        match part(installed, 1) {
            Some(m) if m > want_minor => {
                if fit == VersionFit::Major {
                    fit = VersionFit::Newer;
                }
            }
            _ => return VersionFit::Tested,
        }
    }
    fit
}

/// Expand a leading `~` against `$HOME`. Anything else is returned as-is, so
/// absolute and relative paths pass through untouched.
pub fn expand_tilde(raw: impl AsRef<str>) -> std::path::PathBuf {
    let raw = raw.as_ref();
    if raw == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home);
        }
    } else if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(raw)
}

fn xdg_dir(var: &str, fallback: &str) -> std::path::PathBuf {
    std::env::var_os(var)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback)
        })
        .join("omarchy-studio")
}

/// Studio's own settings (spec 01 §7): `$XDG_CONFIG_HOME/omarchy-studio`,
/// defaulting to `~/.config/omarchy-studio` — holds `config.toml`.
pub fn studio_config_dir() -> std::path::PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// Studio's cache root (spec 01 §7): `$XDG_CACHE_HOME/omarchy-studio`,
/// defaulting to `~/.cache/omarchy-studio` — wallhaven thumbnails live here.
pub fn studio_cache_dir() -> std::path::PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// Studio's own state root (spec 01 §7): `$XDG_STATE_HOME/omarchy-studio`,
/// defaulting to `~/.local/state/omarchy-studio`.
pub fn studio_state_dir() -> std::path::PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tested_versions_read_as_a_list() {
        assert_eq!(tested_omarchy(), "3.8 or 4.0");
    }

    #[test]
    fn version_fit_warns_on_an_unknown_major_not_a_new_minor() {
        assert_eq!(version_fit("3.8.3"), VersionFit::Tested);
        assert_eq!(version_fit("3.8"), VersionFit::Tested);
        // A later minor is expected to work — no noise for it.
        assert_eq!(version_fit("3.9.0"), VersionFit::Newer);
        assert!(version_fit("3.9.0").warning().is_none());
        // Omarchy 4 is the one that rebuilds the shell.
        // Omarchy 4 is a tested major now — Studio writes its Lua config.
        assert_eq!(version_fit("4.0.0"), VersionFit::Tested);
        assert!(version_fit("4.0.0").warning().is_none());
        assert_eq!(version_fit("4.0.0.alpha"), VersionFit::Tested);
        assert_eq!(version_fit("4.1.0"), VersionFit::Newer);
        assert!(version_fit("4.1.0").warning().is_none());

        // A major nobody has tested still warns rather than blocks.
        assert_eq!(version_fit("5.0.0"), VersionFit::Major);
        assert!(version_fit("5.0.0").warning().is_some());
        assert_eq!(version_fit("2.9.0"), VersionFit::Major);
        // An unreadable version file must not look supported.
        assert_eq!(version_fit(""), VersionFit::Unknown);
        assert_eq!(version_fit("what"), VersionFit::Unknown);
        assert!(version_fit("").warning().is_some());
    }
}

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
pub const TESTED_OMARCHY: &str = "3.8";

/// How the installed Omarchy relates to the one this build was tested on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionFit {
    /// Same major.minor as [`TESTED_OMARCHY`].
    Tested,
    /// Same major, later minor — expected to work; paths rarely move.
    Newer,
    /// A different major. Omarchy 4 rebuilds the shell (Waybar, Mako,
    /// Swayosd and Walker collapse into omarchy-shell), so this is where
    /// Studio's assumptions actually break.
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
                "this build is tested against Omarchy {TESTED_OMARCHY} — on a different \
                 major version some paths and commands may have moved. Studio still \
                 snapshots every change, so anything it does here stays undoable."
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
    let (Some(major), Some(want_major)) = (part(installed, 0), part(TESTED_OMARCHY, 0)) else {
        return VersionFit::Unknown;
    };
    if major != want_major {
        return VersionFit::Major;
    }
    match (part(installed, 1), part(TESTED_OMARCHY, 1)) {
        (Some(m), Some(w)) if m > w => VersionFit::Newer,
        _ => VersionFit::Tested,
    }
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
    fn version_fit_warns_on_a_new_major_not_a_new_minor() {
        assert_eq!(version_fit("3.8.3"), VersionFit::Tested);
        assert_eq!(version_fit("3.8"), VersionFit::Tested);
        // A later minor is expected to work — no noise for it.
        assert_eq!(version_fit("3.9.0"), VersionFit::Newer);
        assert!(version_fit("3.9.0").warning().is_none());
        // Omarchy 4 is the one that rebuilds the shell.
        assert_eq!(version_fit("4.0.0"), VersionFit::Major);
        assert!(version_fit("4.0.0").warning().is_some());
        // An unreadable version file must not look supported.
        assert_eq!(version_fit(""), VersionFit::Unknown);
        assert_eq!(version_fit("what"), VersionFit::Unknown);
        assert!(version_fit("").warning().is_some());
    }
}

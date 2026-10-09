//! Omarchy menu integration (spec 02 §3, roadmap 0.1.12).
//!
//! Studio adds itself to the Omarchy menu by declaring one row in
//! `~/.config/omarchy/extensions/omarchy-menu.jsonc`, the JSONC file the
//! Quickshell menu plugin merges over its own defaults at startup. The row id
//! is dotted, so `style.studio` lands in the Style submenu
//! (Super+Alt+Space → Style → Studio) and is reachable by search and by
//! `omarchy menu summon studio`.
//!
//! Omarchy 4 ("Quattro") replaced the bash `omarchy-menu` with that plugin.
//! Studio used to override the `show_style_menu` shell function in
//! `extensions/menu.sh`; nothing sources that file any more, which is why the
//! entry vanished after the update. Install now writes the JSONC row *and*
//! clears the stale shell block, so upgrading users get the entry back without
//! having to find the dead file themselves.
//!
//! Declaring a row is additive rather than an override, so unlike the old shell
//! block it does not freeze the rest of the Style menu at the shape it had when
//! Studio was installed.
//!
//! The launcher is `omarchy-launch-floating-terminal-with-presentation`, which
//! runs under the `org.omarchy.terminal` app-id — already matched by Omarchy's
//! stock float rule, so no window rule needs installing (spec 02 §3.3).

use std::path::PathBuf;

use crate::configfs::jsonc::JsoncDoc;
use crate::configfs::{atomic_write, CommentStyle, ManagedBlock};
use crate::error::{Result, StudioError};
use crate::omarchy::OmarchyPaths;

const SECTION: &str = "menu";

/// Dotted menu id: the parent (`style`) is inferred from the prefix.
const MENU_ID: &str = "style.studio";

/// The row Studio declares. One line, matching the shipped file's own style.
const MENU_ROW: &str = r#"{"icon":"󰏘","label":"Studio","aliases":["studio","omarchy-studio"],"description":"Rice your Omarchy desktop — themes, keybinds, bar, monitors","action":"omarchy-launch-floating-terminal-with-presentation omarchy-studio"}"#;

/// The JSONC object an absent/blank extension file starts from. Mirrors the
/// stub Omarchy ships so a hand-edited file and a Studio-created one look alike.
const EMPTY_DOC: &str = "{\n}\n";

fn menu_path(paths: &OmarchyPaths) -> PathBuf {
    paths.config.join("extensions/omarchy-menu.jsonc")
}

/// Pre-Quattro location: a bash file `omarchy-menu` used to source. Dead in
/// Omarchy 4, but left on disk by the update, so Studio cleans up after itself.
fn legacy_menu_path(paths: &OmarchyPaths) -> PathBuf {
    paths.config.join("extensions/menu.sh")
}

fn legacy_block() -> ManagedBlock {
    ManagedBlock::new(SECTION, CommentStyle::Hash)
}

/// Is the Studio menu row currently declared?
pub fn is_installed(paths: &OmarchyPaths) -> bool {
    let Ok(raw) = std::fs::read_to_string(menu_path(paths)) else {
        return false;
    };
    JsoncDoc::parse(&raw)
        .map(|doc| doc.has_member("", MENU_ID))
        .unwrap_or(false)
}

/// Every file `install_menu`/`uninstall_menu` may write, so callers snapshot the
/// legacy file too and its cleanup stays undoable.
pub fn managed_files(paths: &OmarchyPaths) -> Vec<PathBuf> {
    let mut files = vec![menu_path(paths)];
    let legacy = legacy_menu_path(paths);
    if legacy.is_file() {
        files.push(legacy);
    }
    files
}

/// Drop the pre-Quattro `menu.sh` override if it is still lying around.
/// Removes the file outright when Studio's block was all it held.
fn clear_legacy_block(paths: &OmarchyPaths) -> Result<()> {
    let path = legacy_menu_path(paths);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    if !legacy_block().contains(&existing) {
        return Ok(());
    }
    let updated = legacy_block().remove(&existing);
    if updated.trim().is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        atomic_write(&path, &updated)?;
    }
    Ok(())
}

/// Install (or refresh) the Studio menu row, and clear the dead pre-Quattro
/// shell override. Idempotent. Returns the file that was written.
pub fn install_menu(paths: &OmarchyPaths) -> Result<PathBuf> {
    let path = menu_path(paths);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let source = if existing.trim().is_empty() {
        EMPTY_DOC.to_string()
    } else {
        existing
    };

    let mut doc = JsoncDoc::parse(&source).map_err(|e| StudioError::ParseFailed {
        file: path.clone(),
        line: None,
        hint: format!(
            "the menu extension file isn't valid JSONC ({e}) — fix or delete it, then re-run"
        ),
    })?;
    doc.insert_member("", MENU_ID, MENU_ROW)
        .map_err(|e| StudioError::ParseFailed {
            file: path.clone(),
            line: None,
            hint: format!("could not add the {MENU_ID} row ({e})"),
        })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(&path, &doc.to_string())?;
    clear_legacy_block(paths)?;
    Ok(path)
}

/// Remove the Studio menu row, leaving the rest of the file — the user's own
/// rows and the shipped help comments — intact. Also clears the legacy shell
/// block. Returns the file path if it existed.
pub fn uninstall_menu(paths: &OmarchyPaths) -> Result<Option<PathBuf>> {
    clear_legacy_block(paths)?;
    let path = menu_path(paths);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let mut doc = JsoncDoc::parse(&existing).map_err(|e| StudioError::ParseFailed {
        file: path.clone(),
        line: None,
        hint: format!(
            "the menu extension file isn't valid JSONC ({e}) — fix it by hand, then re-run"
        ),
    })?;
    doc.remove_member("", MENU_ID)
        .map_err(|e| StudioError::ParseFailed {
            file: path.clone(),
            line: None,
            hint: format!("could not remove the {MENU_ID} row ({e})"),
        })?;
    atomic_write(&path, &doc.to_string())?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comment-only stub Omarchy 4 ships as the user's extension file.
    const SHIPPED_STUB: &str = r#"{
  // Extend the Quickshell Omarchy menu with JSONC.
  //
  // Example: replace the default About action by reusing the same id.
  // "about": {"icon":"","label":"About","action":"omarchy-launch-or-focus-tui"},
}
"#;

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-integ-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("config/omarchy/extensions")).unwrap();
        OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join("config/omarchy"),
            state: root.join("state/omarchy"),
        }
    }

    fn read_menu(paths: &OmarchyPaths) -> String {
        std::fs::read_to_string(menu_path(paths)).unwrap()
    }

    #[test]
    fn install_is_idempotent_and_uninstall_is_exact() {
        let paths = fake_paths("menu");
        assert!(!is_installed(&paths));

        install_menu(&paths).unwrap();
        assert!(is_installed(&paths));
        let after_first = read_menu(&paths);

        // second install must not duplicate or drift
        install_menu(&paths).unwrap();
        let after_second = read_menu(&paths);
        assert_eq!(after_first, after_second);
        assert_eq!(after_second.matches(MENU_ID).count(), 1);

        uninstall_menu(&paths).unwrap();
        assert!(!is_installed(&paths));
    }

    #[test]
    fn installs_into_the_shipped_comment_only_stub() {
        let paths = fake_paths("stub");
        std::fs::write(menu_path(&paths), SHIPPED_STUB).unwrap();

        install_menu(&paths).unwrap();
        let out = read_menu(&paths);

        // The row is live, the help comments survived, and it still parses.
        assert!(is_installed(&paths));
        assert!(out.contains("Extend the Quickshell Omarchy menu"));
        let doc = JsoncDoc::parse(&out).expect("still valid JSONC");
        assert!(doc.has_member("", MENU_ID));

        uninstall_menu(&paths).unwrap();
        let back = read_menu(&paths);
        assert!(back.contains("Extend the Quickshell Omarchy menu"));
        assert!(!back.contains(MENU_ID));
        JsoncDoc::parse(&back).expect("valid JSONC after removal");
    }

    #[test]
    fn preserves_a_users_own_rows() {
        let paths = fake_paths("coexist");
        let user =
            "{\n  \"style.cursor\": {\"icon\":\"C\",\"label\":\"Cursor\",\"action\":\"pick\"}\n}\n";
        std::fs::write(menu_path(&paths), user).unwrap();

        install_menu(&paths).unwrap();
        let both = read_menu(&paths);
        assert!(both.contains("style.cursor"));
        assert!(both.contains(MENU_ID));
        JsoncDoc::parse(&both).expect("valid JSONC with both rows");

        uninstall_menu(&paths).unwrap();
        let back = read_menu(&paths);
        assert!(back.contains("style.cursor"));
        assert!(!back.contains(MENU_ID));
        JsoncDoc::parse(&back).expect("valid JSONC after removal");
    }

    #[test]
    fn install_clears_the_dead_pre_quattro_shell_block() {
        let paths = fake_paths("legacy");
        let legacy = legacy_menu_path(&paths);
        let user_override = "show_system_menu() {\n  omarchy-system-lock\n}\n";
        let stale = legacy_block().upsert(user_override, "show_style_menu() {\n  :\n}");
        std::fs::write(&legacy, &stale).unwrap();

        install_menu(&paths).unwrap();

        assert!(is_installed(&paths), "row lands in the JSONC file");
        let left = std::fs::read_to_string(&legacy).unwrap();
        assert!(!legacy_block().contains(&left), "stale block is gone");
        assert_eq!(left, user_override, "the user's own override survives");
    }

    #[test]
    fn legacy_file_is_removed_when_studio_was_its_only_content() {
        let paths = fake_paths("legacy-only");
        let legacy = legacy_menu_path(&paths);
        std::fs::write(
            &legacy,
            legacy_block().upsert("", "show_style_menu() {\n  :\n}"),
        )
        .unwrap();

        install_menu(&paths).unwrap();
        assert!(!legacy.exists(), "nothing left worth keeping");
    }
}

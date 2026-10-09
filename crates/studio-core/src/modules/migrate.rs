//! Cleaning up after the Omarchy 3 → 4 config move.
//!
//! Omarchy 4 ("Quattro") switched Hyprland from hyprlang to Lua. Studio now
//! writes `~/.config/hypr/*.lua`, but a machine that was *upgraded* still has
//! the `.conf` files, and Studio's managed blocks are still sitting in them.
//!
//! Those blocks are inert — a Lua-mode Hyprland never reads the file — but they
//! are not harmless: they say Studio is managing settings it no longer manages,
//! they show up as drift in `doctor`, and anyone reading the file would
//! reasonably believe the values in it are live. This removes them.
//!
//! Only Studio's own marked blocks are touched. Everything else in those files
//! is the user's (or Omarchy's) and is left exactly as it is — and because the
//! change goes through the apply pipeline, it is snapshotted and undoable like
//! any other.

use std::path::{Path, PathBuf};

use crate::configfs::{CommentStyle, ManagedBlock};
use crate::error::Result;
use crate::omarchy::{Dialect, OmarchyPaths};

/// The pre-Quattro user files Studio used to write. `scrolloverview.conf` is
/// the one file Studio created outright; the rest are Omarchy's own user
/// config, so they are edited, never deleted.
const LEGACY_FILES: &[&str] = &[
    "hyprland.conf",
    "monitors.conf",
    "input.conf",
    "bindings.conf",
    "looknfeel.conf",
    "autostart.conf",
    "scrolloverview.conf",
];

/// A file Studio fully owned — safe to remove once its block is gone.
const STUDIO_OWNED: &[&str] = &["scrolloverview.conf"];

/// One legacy file still carrying Studio blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub file: PathBuf,
    /// The managed-block section names found in it, in file order.
    pub sections: Vec<String>,
}

/// The marker Studio opens a managed block with, e.g.
/// `# >>> omarchy-studio:monitors — generated; …`.
fn section_names(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let Some(rest) = line.split_once(">>> omarchy-studio:") else {
            continue;
        };
        let name: String = rest
            .1
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Legacy `.conf` files that still carry Studio blocks.
///
/// Empty on hyprlang, where those files are the live config and removing
/// Studio's blocks would throw away the user's settings.
pub fn stale(paths: &OmarchyPaths, dialect: Dialect) -> Vec<Stale> {
    if !dialect.is_lua() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for name in LEGACY_FILES {
        let file = paths.hypr_config().join(name);
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let sections = section_names(&content);
        if !sections.is_empty() {
            out.push(Stale { file, sections });
        }
    }
    out
}

/// Is a file one Studio created outright, rather than one it contributed to?
fn studio_owned(file: &Path) -> bool {
    file.file_name()
        .and_then(|n| n.to_str())
        .map(|n| STUDIO_OWNED.contains(&n))
        .unwrap_or(false)
}

/// Plan the cleanup as pipeline edits, writing nothing.
///
/// A shared file keeps everything outside the markers. A file Studio fully
/// owned is planned as empty; [`apply`] deletes it afterwards, since the
/// pipeline only ever writes and would leave a 0-byte file behind.
pub fn plan(paths: &OmarchyPaths, dialect: Dialect) -> Vec<crate::engine::FileEdit> {
    let mut edits = Vec::new();
    for item in stale(paths, dialect) {
        let Ok(before) = std::fs::read_to_string(&item.file) else {
            continue;
        };
        let mut after = before.clone();
        for section in &item.sections {
            after = ManagedBlock::new(section.clone(), CommentStyle::Hash).remove(&after);
        }
        if studio_owned(&item.file) && after.trim().is_empty() {
            after = String::new();
        }
        if after != before {
            edits.push(crate::engine::FileEdit::new(
                item.file,
                Some(before.as_str()),
                after,
            ));
        }
    }
    edits
}

/// Remove Studio's blocks from the dead `.conf` files, through the apply
/// pipeline so the whole thing is snapshotted and undoable. Returns the files
/// changed. No reload: these files aren't read, so nothing can change live.
pub fn apply(
    paths: &OmarchyPaths,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<Vec<PathBuf>> {
    let dialect = Dialect::probe(runner);
    let edits = plan(paths, dialect);
    if edits.is_empty() {
        return Ok(Vec::new());
    }
    let files: Vec<PathBuf> = edits.iter().map(|e| e.file.clone()).collect();
    // Files Studio owned outright, now emptied — deleted after the pipeline has
    // snapshotted them, so undo can still bring the content back.
    let emptied: Vec<PathBuf> = edits
        .iter()
        .filter(|e| e.new_content.is_empty() && studio_owned(&e.file))
        .map(|e| e.file.clone())
        .collect();
    let plan = crate::engine::ApplyPlan {
        summary: "remove Studio's blocks from the pre-Omarchy-4 config files".into(),
        module: "migrate".into(),
        edits,
        reload: Vec::new(),
        verify: Vec::new(),
        risk: crate::engine::Risk::Safe,
        trailers: Vec::new(),
    };
    crate::engine::Pipeline::new(store, runner).apply(&plan, false)?;
    for f in emptied {
        let _ = std::fs::remove_file(f);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-migrate-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    fn block(section: &str, body: &str) -> String {
        ManagedBlock::new(section, CommentStyle::Hash).upsert("", body)
    }

    #[test]
    fn finds_every_studio_block_in_the_legacy_files() {
        let p = paths("find");
        std::fs::write(
            p.hypr_config().join("looknfeel.conf"),
            format!(
                "# mine\n{}\n{}\n",
                block("looknfeel", "general {\n    gaps_in = 8\n}"),
                block("animations", "animations {\n    enabled = no\n}")
            ),
        )
        .unwrap();
        std::fs::write(
            p.hypr_config().join("monitors.conf"),
            block("monitors", "monitor = eDP-1, preferred, auto, 1"),
        )
        .unwrap();

        let found = stale(&p, Dialect::Lua);
        assert_eq!(found.len(), 2);
        let lnf = found
            .iter()
            .find(|s| s.file.ends_with("looknfeel.conf"))
            .unwrap();
        assert_eq!(lnf.sections, vec!["looknfeel", "animations"]);
    }

    /// On hyprlang those files *are* the live config — stripping Studio's
    /// blocks there would delete the user's settings.
    #[test]
    fn does_nothing_on_hyprlang() {
        let p = paths("hyprlang");
        std::fs::write(
            p.hypr_config().join("monitors.conf"),
            block("monitors", "monitor = eDP-1, preferred, auto, 1"),
        )
        .unwrap();
        assert!(stale(&p, Dialect::Hyprlang).is_empty());
        assert!(plan(&p, Dialect::Hyprlang).is_empty());
    }

    #[test]
    fn keeps_everything_outside_the_markers() {
        let p = paths("keep");
        let file = p.hypr_config().join("looknfeel.conf");
        let users = "# my own settings\ngeneral {\n    border_size = 3\n}\n";
        std::fs::write(
            &file,
            format!(
                "{users}{}\n",
                block("looknfeel", "general {\n    gaps_in = 8\n}")
            ),
        )
        .unwrap();

        let edits = plan(&p, Dialect::Lua);
        assert_eq!(edits.len(), 1);
        // Block removal can leave the blank line the block sat on; what matters
        // is that the user's own lines survive and no marker remains.
        assert_eq!(edits[0].new_content.trim_end(), users.trim_end());
        assert!(!edits[0].new_content.contains("omarchy-studio"));
        assert!(edits[0].new_content.contains("border_size = 3"));
    }

    #[test]
    fn empties_a_file_studio_created_outright() {
        let p = paths("owned");
        std::fs::write(
            p.hypr_config().join("scrolloverview.conf"),
            format!("{}\n", block("scrolloverview", "plugin {\n  x = 1\n}")),
        )
        .unwrap();
        let edits = plan(&p, Dialect::Lua);
        assert_eq!(edits.len(), 1);
        assert!(
            edits[0].new_content.is_empty(),
            "a file only Studio ever wrote goes away entirely"
        );
    }

    /// The pipeline only writes, so a Studio-owned file planned as empty has to
    /// be deleted by `apply` or it lingers as a 0-byte file.
    #[test]
    fn an_owned_file_is_planned_empty_so_apply_can_delete_it() {
        let p = paths("delete");
        let owned = p.hypr_config().join("scrolloverview.conf");
        std::fs::write(
            &owned,
            format!("{}\n", block("scrolloverview", "plugin {\n  x = 1\n}")),
        )
        .unwrap();
        let edits = plan(&p, Dialect::Lua);
        let edit = edits.iter().find(|e| e.file == owned).unwrap();
        assert!(edit.new_content.is_empty());
        assert!(studio_owned(&edit.file));
        // A shared file is never a delete candidate, even if it ends up empty.
        assert!(!studio_owned(&p.hypr_config().join("looknfeel.conf")));
    }

    #[test]
    fn is_a_no_op_once_there_is_nothing_left_to_clean() {
        let p = paths("noop");
        std::fs::write(p.hypr_config().join("looknfeel.conf"), "# just mine\n").unwrap();
        assert!(stale(&p, Dialect::Lua).is_empty());
        assert!(plan(&p, Dialect::Lua).is_empty());
    }

    #[test]
    fn reads_section_names_off_the_real_marker_format() {
        let rendered = block("scrolloverview-source", "source = /x/y.conf");
        assert_eq!(section_names(&rendered), vec!["scrolloverview-source"]);
        assert!(section_names("# nothing to see").is_empty());
    }
}

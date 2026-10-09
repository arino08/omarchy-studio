//! Config parsers/writers (spec 03). One submodule per dialect as they land:
//! `hyprlang` (0.2.1), `jsonc` (0.4.1), `managed` blocks.
//!
//! Invariant for every full parser: `to_string(parse(s)) == s` byte-identical
//! for the entire golden corpus (spec 09 §2). Unknown content is preserved,
//! never normalized, never dropped.

pub mod hyprlang;
pub mod jsonc;
pub mod lua;

use std::path::Path;

/// Where an effective value came from — Hyprland's source layering (spec 03 §2).
/// Studio only ever WRITES to the `User` layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// `$OMARCHY_PATH/default/hypr/*` — read-only Omarchy defaults.
    Default,
    /// `~/.config/omarchy/current/theme/hyprland.conf`.
    Theme,
    /// `~/.config/hypr/*.conf` — sourced last, wins.
    User,
    /// `~/.local/state/omarchy/toggles/hypr/*.conf`.
    Toggle,
    /// Present at runtime but not found in any sourced file (e.g. plugins).
    Runtime,
}

/// Provenance of a parsed value/bind (spec 03 §2).
#[derive(Debug, Clone)]
pub struct SourceRef {
    pub file: std::path::PathBuf,
    pub line: usize,
    pub layer: Layer,
}

/// Atomic file write (spec 03 §5): tmp sibling + fsync + rename, preserving
/// an existing file's permissions. Parent directories are created.
///
/// Symlinks are never written *through*: if `path` is a symlink this errors —
/// callers must target the real user-owned source file (the mako-config-is-a-
/// symlink case, spec 05 §5).
pub fn atomic_write(path: &Path, content: &str) -> crate::error::Result<()> {
    use std::io::Write;

    if path.is_symlink() {
        return Err(crate::StudioError::External {
            cmd: format!("write {}", path.display()),
            detail: "refusing to write through a symlink — edit the real source file".into(),
        });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mode = std::fs::metadata(path).ok().map(|m| {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode()
    });

    let tmp = path.with_extension("tmp.omarchy-studio");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Comment syntax for a config dialect, used to delimit managed blocks.
#[derive(Debug, Clone, Copy)]
pub enum CommentStyle {
    /// `# …` — bash, ini, toml, hyprlang.
    Hash,
    /// `/* … */` — CSS.
    CBlock,
    /// `// …` — jsonc.
    DblSlash,
    /// `-- …` — Lua (Omarchy 4 Hyprland config).
    DashDash,
}

impl CommentStyle {
    fn line(self, inner: &str) -> String {
        match self {
            CommentStyle::Hash => format!("# {inner}"),
            CommentStyle::CBlock => format!("/* {inner} */"),
            CommentStyle::DblSlash => format!("// {inner}"),
            CommentStyle::DashDash => format!("-- {inner}"),
        }
    }
}

/// A Studio-owned region inside a shared config file (spec 03 §4). Everything
/// between the markers is regenerated wholesale on each write; everything
/// outside is opaque and untouched. This is how Studio contributes to files
/// it does not own (waybar CSS, swayosd CSS, the omarchy menu extension).
pub struct ManagedBlock {
    pub section: String,
    pub style: CommentStyle,
}

impl ManagedBlock {
    pub fn new(section: impl Into<String>, style: CommentStyle) -> Self {
        Self {
            section: section.into(),
            style,
        }
    }

    fn open(&self) -> String {
        self.style.line(&format!(
            ">>> omarchy-studio:{} — generated; edit outside this block only >>>",
            self.section
        ))
    }

    fn close(&self) -> String {
        self.style
            .line(&format!("<<< omarchy-studio:{} <<<", self.section))
    }

    /// The full block text (markers + body), without surrounding blank lines.
    pub fn render(&self, body: &str) -> String {
        format!(
            "{}\n{}\n{}",
            self.open(),
            body.trim_end_matches('\n'),
            self.close()
        )
    }

    /// True if this section's block is already present.
    pub fn contains(&self, content: &str) -> bool {
        self.locate(content).is_some()
    }

    /// Byte range of the block (open marker line start .. close marker line
    /// end), matching markers by trimmed line equality.
    fn locate(&self, content: &str) -> Option<(usize, usize)> {
        let (open, close) = (self.open(), self.close());
        let mut start = None;
        let mut offset = 0;
        for line in content.split_inclusive('\n') {
            let trimmed = line.trim_end_matches(['\n', ' ', '\t']);
            if start.is_none() && trimmed == open {
                start = Some(offset);
            } else if start.is_some() && trimmed == close {
                return Some((start.unwrap(), offset + line.trim_end_matches('\n').len()));
            }
            offset += line.len();
        }
        None
    }

    /// The body between the markers (exclusive), or None if the block is
    /// absent. Lets a caller read back what Studio previously wrote.
    pub fn extract<'a>(&self, content: &'a str) -> Option<&'a str> {
        let (s, e) = self.locate(content)?;
        let inner = &content[s..e];
        // strip the first line (open marker) and last line (close marker)
        let after_open = inner.split_once('\n').map(|(_, r)| r).unwrap_or("");
        let body = after_open
            .rsplit_once('\n')
            .map(|(b, _)| b)
            .unwrap_or(after_open);
        Some(body)
    }

    /// Insert or replace the block, returning the new file content. Replaces in
    /// place if present; otherwise appends at end (blocks own the end of the
    /// file so later definitions win, spec 03 §4).
    pub fn upsert(&self, content: &str, body: &str) -> String {
        let block = self.render(body);
        match self.locate(content) {
            Some((s, e)) => format!("{}{}{}", &content[..s], block, &content[e..]),
            None => {
                if content.is_empty() {
                    format!("{block}\n")
                } else {
                    let sep = if content.ends_with("\n\n") {
                        ""
                    } else if content.ends_with('\n') {
                        "\n"
                    } else {
                        "\n\n"
                    };
                    format!("{content}{sep}{block}\n")
                }
            }
        }
    }

    /// Remove the block and the newline it leaves behind. Returns the content
    /// unchanged if the block is absent.
    pub fn remove(&self, content: &str) -> String {
        match self.locate(content) {
            Some((s, e)) => {
                let mut end = e;
                if content[end..].starts_with('\n') {
                    end += 1;
                }
                let before = &content[..s];
                let trimmed_before = before.trim_end_matches('\n');
                let kept = if trimmed_before.is_empty() { "" } else { "\n" };
                format!("{trimmed_before}{kept}{}", &content[end..])
            }
            None => content.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-configfs-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = scratch("aw");
        let f = dir.join("nested/new.conf");
        atomic_write(&f, "one").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one");
        atomic_write(&f, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        // no tmp litter
        assert!(!f.with_extension("tmp.omarchy-studio").exists());
    }

    #[test]
    fn atomic_write_preserves_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let f = dir.join("script.sh");
        std::fs::write(&f, "#!/bin/bash\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        atomic_write(&f, "#!/bin/bash\necho hi\n").unwrap();
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn atomic_write_refuses_symlinks() {
        let dir = scratch("sym");
        let real = dir.join("real.ini");
        std::fs::write(&real, "x").unwrap();
        let link = dir.join("link.ini");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = atomic_write(&link, "y").unwrap_err();
        match err {
            crate::StudioError::External { detail, .. } => {
                assert!(detail.contains("symlink"))
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "x");
    }

    #[test]
    fn managed_block_upsert_insert_and_replace() {
        let mb = ManagedBlock::new("menu", CommentStyle::Hash);
        let base = "existing line
";
        let once = mb.upsert(base, "body v1");
        assert!(once.starts_with(
            "existing line
"
        ));
        assert!(mb.contains(&once));
        assert!(once.contains("body v1"));
        // upsert again replaces in place — idempotent, single block.
        let twice = mb.upsert(&once, "body v2");
        assert_eq!(twice.matches("omarchy-studio:menu").count(), 2); // open + close only
        assert!(twice.contains("existing line"));
        assert!(twice.contains("body v2"));
        assert!(!twice.contains("body v1"));
    }

    #[test]
    fn managed_block_remove_is_clean_and_reversible() {
        let mb = ManagedBlock::new("geometry", CommentStyle::CBlock);
        let base = "@import \"theme.css\";\n";
        let with = mb.upsert(base, "* { margin: 0 }");
        assert!(with.contains("/* >>> omarchy-studio:geometry"));
        let without = mb.remove(&with);
        assert_eq!(without, base, "remove should restore the original bytes");
        // removing an absent block is a no-op.
        assert_eq!(mb.remove(base), base);
    }

    #[test]
    fn managed_block_leaves_foreign_content_untouched() {
        let mb = ManagedBlock::new("menu", CommentStyle::Hash);
        let foreign = "show_system_menu() {\n  :\n}\n";
        let with = mb.upsert(foreign, "show_style_menu() { :; }");
        // foreign function survives verbatim
        assert!(with.contains("show_system_menu() {\n  :\n}"));
        let back = mb.remove(&with);
        assert_eq!(back, foreign);
    }
}

//! Animations presets (spec 05 §3, roadmap 0.3.6).
//!
//! Rather than expose Hyprland's bezier/animation grammar, we offer a handful
//! of hand-tuned *feels* — Off, Fast, Smooth, Minimal, Bouncy — and the stock
//! Omarchy default. Choosing one writes a managed `animations { … }` block into
//! the user's looknfeel.conf (sourced last, so it wins) and reloads Hyprland.
//! The bezier canvas editor is a separate, feature-flagged concern (0.3.6b).

use std::path::PathBuf;

use crate::configfs::{CommentStyle, ManagedBlock};
use crate::error::Result;
use crate::omarchy::OmarchyPaths;

/// A named animation feel. `body` is the full `animations { … }` block, or
/// empty for "Default" (which removes our override entirely).
#[derive(Debug, Clone, Copy)]
pub struct AnimPreset {
    pub name: &'static str,
    pub blurb: &'static str,
    pub body: &'static str,
}

pub const PRESETS: &[AnimPreset] = &[
    AnimPreset {
        name: "Default",
        blurb: "Omarchy's stock animations",
        body: "",
    },
    AnimPreset {
        name: "Off",
        blurb: "No animations at all — instant everything",
        body: "animations {\n    enabled = no\n}",
    },
    AnimPreset {
        name: "Fast",
        blurb: "Snappy, quick motion",
        body: "animations {\n    enabled = yes\n    bezier = snappy, 0.05, 0.9, 0.1, 1\n    \
            animation = windows, 1, 2, snappy, popin 90%\n    animation = fade, 1, 2, snappy\n    \
            animation = workspaces, 1, 2, snappy\n    animation = layers, 1, 2, snappy\n    \
            animation = border, 1, 2, snappy\n}",
    },
    AnimPreset {
        name: "Smooth",
        blurb: "Slower, flowing motion",
        body: "animations {\n    enabled = yes\n    bezier = smooth, 0.25, 1, 0.3, 1\n    \
            animation = windows, 1, 6, smooth\n    animation = fade, 1, 6, smooth\n    \
            animation = workspaces, 1, 6, smooth\n    animation = layers, 1, 6, smooth\n    \
            animation = border, 1, 6, smooth\n}",
    },
    AnimPreset {
        name: "Minimal",
        blurb: "Fades only — no sliding or scaling",
        body: "animations {\n    enabled = yes\n    animation = windows, 0, 0, default\n    \
            animation = workspaces, 0, 0, default\n    animation = fade, 1, 4, default\n    \
            animation = layers, 1, 4, default\n}",
    },
    AnimPreset {
        name: "Bouncy",
        blurb: "Playful overshoot on windows",
        body: "animations {\n    enabled = yes\n    bezier = bounce, 0.3, 1.4, 0.5, 1\n    \
            animation = windows, 1, 4, bounce, popin 80%\n    animation = workspaces, 1, 4, bounce\n    \
            animation = fade, 1, 3, bounce\n}",
    },
];

pub fn preset(name: &str) -> Option<&'static AnimPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

fn block() -> ManagedBlock {
    ManagedBlock::new("animations", CommentStyle::Hash)
}

fn user_path(paths: &OmarchyPaths) -> PathBuf {
    paths.hypr_config().join("looknfeel.conf")
}

/// The preset whose body currently occupies the managed block, if recognizable.
/// Absent block → "Default".
pub fn current(paths: &OmarchyPaths) -> &'static str {
    let Ok(text) = std::fs::read_to_string(user_path(paths)) else {
        return "Default";
    };
    match block().extract(&text) {
        None => "Default",
        Some(body) => {
            // drop our leading "# Managed by …" header before matching
            let core = body
                .lines()
                .skip_while(|l| l.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed = core.trim();
            PRESETS
                .iter()
                .find(|p| p.body.trim() == trimmed)
                .map(|p| p.name)
                .unwrap_or("Custom")
        }
    }
}

/// Write a preset into the managed block (empty body removes it) and reload.
/// Caller snapshots first for undo.
pub fn apply(
    paths: &OmarchyPaths,
    preset: &AnimPreset,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<PathBuf> {
    let path = user_path(paths);
    // `None` for a file that doesn't exist yet — the pipeline's hash guard
    // reads it the same way, and treating absent as empty makes it reject.
    let on_disk = std::fs::read_to_string(&path).ok();
    let existing = on_disk.clone().unwrap_or_default();
    let updated = if preset.body.trim().is_empty() {
        block().remove(&existing)
    } else {
        let body = format!(
            "# Managed by Omarchy Studio — animation feel: {}.\n{}",
            preset.name, preset.body
        );
        block().upsert(&existing, &body)
    };
    if updated == existing {
        return Ok(path); // already this preset — nothing to snapshot or reload
    }
    let plan = crate::engine::ApplyPlan {
        summary: format!("animations: {}", preset.name),
        module: "animations".into(),
        edits: vec![crate::engine::FileEdit::new(
            path.clone(),
            on_disk.as_deref(),
            updated,
        )],
        reload: vec![crate::engine::ReloadStep::HyprReload],
        verify: crate::engine::hypr_verification(runner),
        risk: crate::engine::Risk::Safe,
        trailers: Vec::new(),
    };
    crate::engine::Pipeline::new(store, runner).apply(&plan, false)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::StubRunner;

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-anim-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("sys/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    #[test]
    fn presets_bodies_are_valid_hyprlang_animation_blocks() {
        use crate::configfs::hyprlang::HyprDoc;
        for p in PRESETS {
            if p.body.is_empty() {
                continue;
            }
            // must parse and round-trip; "Off" disables, others enable
            let doc = HyprDoc::parse(&format!("{}\n", p.body));
            assert_eq!(
                HyprDoc::parse(&format!("{}\n", p.body)).to_string(),
                format!("{}\n", p.body)
            );
            let enabled = doc.get("animations.enabled");
            if p.name == "Off" {
                assert_eq!(enabled.as_deref(), Some("no"));
            } else {
                assert_eq!(enabled.as_deref(), Some("yes"), "{} should enable", p.name);
            }
        }
    }

    #[test]
    fn apply_and_current_round_trip() {
        let paths = fake_paths("apply");
        std::fs::write(
            user_path(&paths),
            "# user\ngeneral {\n    border_size = 2\n}\n",
        )
        .unwrap();
        let runner = StubRunner::default().with_ok("hyprctl reload", "ok");
        // The pipeline snapshots with real git; the stub covers hyprctl.
        let store = crate::snapshot::SnapshotStore::open_or_init(
            paths.config.join("history"),
            Box::new(crate::cmd::RealRunner),
        )
        .unwrap();

        assert_eq!(current(&paths), "Default");
        apply(&paths, preset("Fast").unwrap(), &store, &runner).unwrap();
        assert_eq!(current(&paths), "Fast");
        let on_disk = std::fs::read_to_string(user_path(&paths)).unwrap();
        assert!(on_disk.contains("border_size = 2")); // user's own untouched
        assert!(on_disk.contains("bezier = snappy"));

        // switching preset replaces cleanly
        apply(&paths, preset("Off").unwrap(), &store, &runner).unwrap();
        assert_eq!(current(&paths), "Off");
        // Default removes the block, restoring the file
        apply(&paths, preset("Default").unwrap(), &store, &runner).unwrap();
        assert_eq!(current(&paths), "Default");
        let after = std::fs::read_to_string(user_path(&paths)).unwrap();
        assert!(!after.contains("omarchy-studio:animations"));
    }
}

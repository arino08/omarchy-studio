//! Animations presets (spec 05 §3, roadmap 0.3.6).
//!
//! Rather than expose Hyprland's bezier/animation grammar, we offer a handful
//! of hand-tuned *feels* — Off, Fast, Smooth, Minimal, Bouncy — and the stock
//! Omarchy default. Choosing one writes a managed block into the user's
//! look & feel file (loaded last, so it wins) and reloads Hyprland.
//! The bezier canvas editor is a separate, feature-flagged concern (0.3.6b).
//!
//! Each preset is described *structurally* rather than as pre-baked config
//! text, so the same definition renders to hyprlang (Omarchy ≤ 3) or to Lua
//! (`hl.curve` / `hl.animation`, Omarchy 4) with no second copy to keep in sync.

use std::path::PathBuf;

use crate::configfs::lua;
use crate::configfs::ManagedBlock;
use crate::error::Result;
use crate::omarchy::{Dialect, OmarchyPaths};

/// A cubic bezier curve the preset defines and its animations refer to.
#[derive(Debug, Clone, Copy)]
pub struct Curve {
    pub name: &'static str,
    /// The two control points, `[x1, y1, x2, y2]`.
    pub points: [f64; 4],
}

/// One animation leaf's setting.
#[derive(Debug, Clone, Copy)]
pub struct Anim {
    /// Hyprland's animation name: `windows`, `fade`, `workspaces`, …
    pub leaf: &'static str,
    pub enabled: bool,
    pub speed: f64,
    /// Curve name — the preset's own, or a stock one like `default`.
    pub curve: &'static str,
    pub style: Option<&'static str>,
}

/// A named animation feel. An empty [`AnimPreset::anims`] with no explicit
/// `enabled` means "Default": remove Studio's override entirely.
#[derive(Debug, Clone, Copy)]
pub struct AnimPreset {
    pub name: &'static str,
    pub blurb: &'static str,
    /// `Some(false)` turns animations off wholesale; `Some(true)` enables them
    /// alongside `anims`; `None` writes nothing (the stock default).
    pub enabled: Option<bool>,
    pub curve: Option<Curve>,
    pub anims: &'static [Anim],
}

const fn anim(leaf: &'static str, speed: f64, curve: &'static str) -> Anim {
    Anim {
        leaf,
        enabled: true,
        speed,
        curve,
        style: None,
    }
}

const fn anim_styled(
    leaf: &'static str,
    speed: f64,
    curve: &'static str,
    style: &'static str,
) -> Anim {
    Anim {
        leaf,
        enabled: true,
        speed,
        curve,
        style: Some(style),
    }
}

const fn anim_off(leaf: &'static str) -> Anim {
    Anim {
        leaf,
        enabled: false,
        speed: 0.0,
        curve: "default",
        style: None,
    }
}

const SNAPPY: Curve = Curve {
    name: "snappy",
    points: [0.05, 0.9, 0.1, 1.0],
};
const SMOOTH: Curve = Curve {
    name: "smooth",
    points: [0.25, 1.0, 0.3, 1.0],
};
const BOUNCE: Curve = Curve {
    name: "bounce",
    points: [0.3, 1.4, 0.5, 1.0],
};

pub const PRESETS: &[AnimPreset] = &[
    AnimPreset {
        name: "Default",
        blurb: "Omarchy's stock animations",
        enabled: None,
        curve: None,
        anims: &[],
    },
    AnimPreset {
        name: "Off",
        blurb: "No animations at all — instant everything",
        enabled: Some(false),
        curve: None,
        anims: &[],
    },
    AnimPreset {
        name: "Fast",
        blurb: "Snappy, quick motion",
        enabled: Some(true),
        curve: Some(SNAPPY),
        anims: &[
            anim_styled("windows", 2.0, "snappy", "popin 90%"),
            anim("fade", 2.0, "snappy"),
            anim("workspaces", 2.0, "snappy"),
            anim("layers", 2.0, "snappy"),
            anim("border", 2.0, "snappy"),
        ],
    },
    AnimPreset {
        name: "Smooth",
        blurb: "Slower, flowing motion",
        enabled: Some(true),
        curve: Some(SMOOTH),
        anims: &[
            anim("windows", 6.0, "smooth"),
            anim("fade", 6.0, "smooth"),
            anim("workspaces", 6.0, "smooth"),
            anim("layers", 6.0, "smooth"),
            anim("border", 6.0, "smooth"),
        ],
    },
    AnimPreset {
        name: "Minimal",
        blurb: "Fades only — no sliding or scaling",
        enabled: Some(true),
        curve: None,
        anims: &[
            anim_off("windows"),
            anim_off("workspaces"),
            anim("fade", 4.0, "default"),
            anim("layers", 4.0, "default"),
        ],
    },
    AnimPreset {
        name: "Bouncy",
        blurb: "Playful overshoot on windows",
        enabled: Some(true),
        curve: Some(BOUNCE),
        anims: &[
            anim_styled("windows", 4.0, "bounce", "popin 80%"),
            anim("workspaces", 4.0, "bounce"),
            anim("fade", 3.0, "bounce"),
        ],
    },
];

/// Trim a float to the shortest form that reads the same (`2`, `0.05`).
fn num(v: f64) -> String {
    let s = format!("{v}");
    s.trim_end_matches('.').to_string()
}

impl AnimPreset {
    /// Does this preset write anything at all?
    pub fn is_default(&self) -> bool {
        self.enabled.is_none()
    }

    /// The managed-block body for a dialect. Empty for "Default".
    pub fn body_for(&self, dialect: Dialect) -> String {
        if dialect.is_lua() {
            self.render_lua()
        } else {
            self.render_hyprlang()
        }
    }

    /// `animations { … }` with `bezier =` / `animation =` lines.
    fn render_hyprlang(&self) -> String {
        let Some(enabled) = self.enabled else {
            return String::new();
        };
        let mut out = String::from("animations {\n");
        out.push_str(&format!(
            "    enabled = {}\n",
            if enabled { "yes" } else { "no" }
        ));
        if let Some(c) = self.curve {
            out.push_str(&format!(
                "    bezier = {}, {}, {}, {}, {}\n",
                c.name,
                num(c.points[0]),
                num(c.points[1]),
                num(c.points[2]),
                num(c.points[3])
            ));
        }
        for a in self.anims {
            out.push_str(&format!(
                "    animation = {}, {}, {}, {}",
                a.leaf,
                if a.enabled { 1 } else { 0 },
                num(a.speed),
                a.curve
            ));
            if let Some(style) = a.style {
                out.push_str(&format!(", {style}"));
            }
            out.push('\n');
        }
        out.push('}');
        out
    }

    /// `hl.config` for the on/off switch, then `hl.curve` / `hl.animation`.
    fn render_lua(&self) -> String {
        let Some(enabled) = self.enabled else {
            return String::new();
        };
        let mut lines = vec![lua::config_call(&[(
            "animations.enabled".to_string(),
            lua::Value::Bool(enabled),
        )])];
        if let Some(c) = self.curve {
            lines.push(lua::named_table_call(
                "hl.curve",
                c.name,
                &[
                    ("type", lua::Value::Str("bezier".into())),
                    (
                        "points",
                        lua::Value::List(vec![
                            lua::Value::List(vec![
                                lua::Value::Float(c.points[0]),
                                lua::Value::Float(c.points[1]),
                            ]),
                            lua::Value::List(vec![
                                lua::Value::Float(c.points[2]),
                                lua::Value::Float(c.points[3]),
                            ]),
                        ]),
                    ),
                ],
            ));
        }
        for a in self.anims {
            let mut fields = vec![
                ("leaf", lua::Value::Str(a.leaf.to_string())),
                ("enabled", lua::Value::Bool(a.enabled)),
            ];
            // A disabled leaf carries nothing else — speed and curve would be
            // noise, and Hyprland's own defaults write it the same way.
            if a.enabled {
                fields.push(("speed", lua::Value::Float(a.speed)));
                fields.push(("bezier", lua::Value::Str(a.curve.to_string())));
                if let Some(style) = a.style {
                    fields.push(("style", lua::Value::Str(style.to_string())));
                }
            }
            lines.push(lua::table_call("hl.animation", &fields));
        }
        lines.join("\n")
    }
}

pub fn preset(name: &str) -> Option<&'static AnimPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

fn block(dialect: Dialect) -> ManagedBlock {
    ManagedBlock::new("animations", dialect.comment_style())
}

/// Animations ride along in the look & feel file, which Omarchy loads last.
fn user_path(paths: &OmarchyPaths, dialect: Dialect) -> PathBuf {
    paths
        .hypr_config()
        .join(format!("looknfeel.{}", dialect.ext()))
}

/// The preset whose body currently occupies the managed block, if recognizable.
/// Absent block → "Default".
pub fn current(paths: &OmarchyPaths) -> &'static str {
    current_for(paths, Dialect::probe(&crate::cmd::RealRunner))
}

/// As [`current`], for a given dialect.
pub fn current_for(paths: &OmarchyPaths, dialect: Dialect) -> &'static str {
    let Ok(text) = std::fs::read_to_string(user_path(paths, dialect)) else {
        return "Default";
    };
    match block(dialect).extract(&text) {
        None => "Default",
        Some(body) => {
            // drop our leading "Managed by …" header before matching
            let marker = if dialect.is_lua() { "--" } else { "#" };
            let core = body
                .lines()
                .skip_while(|l| l.trim_start().starts_with(marker))
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed = core.trim();
            PRESETS
                .iter()
                .find(|p| !p.is_default() && p.body_for(dialect).trim() == trimmed)
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
    let dialect = Dialect::probe(runner);
    let path = user_path(paths, dialect);
    // `None` for a file that doesn't exist yet — the pipeline's hash guard
    // reads it the same way, and treating absent as empty makes it reject.
    let on_disk = std::fs::read_to_string(&path).ok();
    let existing = on_disk.clone().unwrap_or_default();
    let rendered = preset.body_for(dialect);
    let updated = if rendered.trim().is_empty() {
        block(dialect).remove(&existing)
    } else {
        let marker = if dialect.is_lua() { "--" } else { "#" };
        let body = format!(
            "{marker} Managed by Omarchy Studio — animation feel: {}.\n{rendered}",
            preset.name
        );
        block(dialect).upsert(&existing, &body)
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

    /// The structural rewrite must render byte-identical hyprlang to the text
    /// these presets used to carry literally — otherwise `current()` would stop
    /// recognising a block an existing user already has and call it "Custom".
    #[test]
    fn hyprlang_output_is_unchanged_from_the_literal_bodies() {
        assert_eq!(
            preset("Off").unwrap().render_hyprlang(),
            "animations {\n    enabled = no\n}"
        );
        assert_eq!(
            preset("Fast").unwrap().render_hyprlang(),
            "animations {\n    \
             enabled = yes\n    \
             bezier = snappy, 0.05, 0.9, 0.1, 1\n    \
             animation = windows, 1, 2, snappy, popin 90%\n    \
             animation = fade, 1, 2, snappy\n    \
             animation = workspaces, 1, 2, snappy\n    \
             animation = layers, 1, 2, snappy\n    \
             animation = border, 1, 2, snappy\n}"
        );
        assert_eq!(
            preset("Minimal").unwrap().render_hyprlang(),
            "animations {\n    \
             enabled = yes\n    \
             animation = windows, 0, 0, default\n    \
             animation = workspaces, 0, 0, default\n    \
             animation = fade, 1, 4, default\n    \
             animation = layers, 1, 4, default\n}"
        );
        assert_eq!(
            preset("Bouncy").unwrap().render_hyprlang(),
            "animations {\n    \
             enabled = yes\n    \
             bezier = bounce, 0.3, 1.4, 0.5, 1\n    \
             animation = windows, 1, 4, bounce, popin 80%\n    \
             animation = workspaces, 1, 4, bounce\n    \
             animation = fade, 1, 3, bounce\n}"
        );
        assert_eq!(
            preset("Smooth").unwrap().render_hyprlang(),
            "animations {\n    \
             enabled = yes\n    \
             bezier = smooth, 0.25, 1, 0.3, 1\n    \
             animation = windows, 1, 6, smooth\n    \
             animation = fade, 1, 6, smooth\n    \
             animation = workspaces, 1, 6, smooth\n    \
             animation = layers, 1, 6, smooth\n    \
             animation = border, 1, 6, smooth\n}"
        );
        assert!(preset("Default").unwrap().render_hyprlang().is_empty());
    }

    #[test]
    fn presets_bodies_are_valid_hyprlang_animation_blocks() {
        use crate::configfs::hyprlang::HyprDoc;
        for p in PRESETS {
            if p.is_default() {
                continue;
            }
            // must parse and round-trip; "Off" disables, others enable
            let doc = HyprDoc::parse(&format!("{}\n", p.render_hyprlang()));
            assert_eq!(
                HyprDoc::parse(&format!("{}\n", p.render_hyprlang())).to_string(),
                format!("{}\n", p.render_hyprlang())
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
            user_path(&paths, Dialect::Hyprlang),
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

        assert_eq!(current_for(&paths, Dialect::Hyprlang), "Default");
        apply(&paths, preset("Fast").unwrap(), &store, &runner).unwrap();
        assert_eq!(current_for(&paths, Dialect::Hyprlang), "Fast");
        let on_disk = std::fs::read_to_string(user_path(&paths, Dialect::Hyprlang)).unwrap();
        assert!(on_disk.contains("border_size = 2")); // user's own untouched
        assert!(on_disk.contains("bezier = snappy"));

        // switching preset replaces cleanly
        apply(&paths, preset("Off").unwrap(), &store, &runner).unwrap();
        assert_eq!(current_for(&paths, Dialect::Hyprlang), "Off");
        // Default removes the block, restoring the file
        apply(&paths, preset("Default").unwrap(), &store, &runner).unwrap();
        assert_eq!(current_for(&paths, Dialect::Hyprlang), "Default");
        let after = std::fs::read_to_string(user_path(&paths, Dialect::Hyprlang)).unwrap();
        assert!(!after.contains("omarchy-studio:animations"));
    }
}

/// Omarchy 4: the same feels as `hl.curve` / `hl.animation` calls.
#[cfg(test)]
mod lua_tests {
    use super::*;

    #[test]
    fn off_is_a_single_config_call() {
        assert_eq!(
            preset("Off").unwrap().render_lua(),
            "hl.config({\n  animations = {\n    enabled = false,\n  },\n})"
        );
    }

    #[test]
    fn a_curve_and_its_animations_render_as_calls() {
        let out = preset("Fast").unwrap().render_lua();
        assert!(out.contains("enabled = true"));
        assert!(out.contains(
            "hl.curve(\"snappy\", { type = \"bezier\", points = { { 0.05, 0.9 }, { 0.1, 1.0 } } })"
        ));
        assert!(out.contains(
            "hl.animation({ leaf = \"windows\", enabled = true, speed = 2.0, \
             bezier = \"snappy\", style = \"popin 90%\" })"
        ));
        assert!(out.contains(
            "hl.animation({ leaf = \"border\", enabled = true, speed = 2.0, bezier = \"snappy\" })"
        ));
        assert!(!out.contains("bezier = snappy,"), "no hyprlang syntax");
    }

    /// A leaf that's off carries no speed or curve — they'd be noise, and it
    /// matches how Omarchy's own defaults write a disabled leaf.
    #[test]
    fn a_disabled_leaf_carries_only_its_switch() {
        let out = preset("Minimal").unwrap().render_lua();
        assert!(out.contains("hl.animation({ leaf = \"windows\", enabled = false })"));
        assert!(out.contains(
            "hl.animation({ leaf = \"fade\", enabled = true, speed = 4.0, bezier = \"default\" })"
        ));
    }

    #[test]
    fn default_writes_nothing_in_either_dialect() {
        let d = preset("Default").unwrap();
        assert!(d.is_default());
        assert!(d.body_for(Dialect::Lua).is_empty());
        assert!(d.body_for(Dialect::Hyprlang).is_empty());
    }

    #[test]
    fn current_recognises_a_lua_block_it_wrote() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-anim-lua-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        let paths = OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        };
        let path = user_path(&paths, Dialect::Lua);
        let p = preset("Bouncy").unwrap();
        let body = format!(
            "-- Managed by Omarchy Studio — animation feel: {}.\n{}",
            p.name,
            p.body_for(Dialect::Lua)
        );
        std::fs::write(&path, block(Dialect::Lua).upsert("", &body)).unwrap();

        assert_eq!(current_for(&paths, Dialect::Lua), "Bouncy");
        // The same file read as hyprlang has no hyprlang block in it.
        assert_eq!(current_for(&paths, Dialect::Hyprlang), "Default");
    }

    #[test]
    fn every_preset_emits_valid_lua() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for p in PRESETS {
            let source = p.render_lua();
            if source.is_empty() {
                continue;
            }
            let Ok(mut child) = Command::new("luac")
                .args(["-p", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            else {
                eprintln!("skipping: no luac on PATH");
                return;
            };
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(source.as_bytes())
                .expect("write");
            let out = child.wait_with_output().expect("luac");
            assert!(
                out.status.success(),
                "{} emitted invalid Lua:\n{source}\n{}",
                p.name,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

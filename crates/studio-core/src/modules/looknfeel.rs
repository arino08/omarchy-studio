//! Look & Feel schema + model (spec 05 §2, roadmap 0.3.1).
//!
//! A small, curated schema of the Hyprland general/decoration settings people
//! actually reach for — gaps, borders, rounding, opacity, blur, shadow — each
//! with a human label, a value kind (for validation and the right widget), and
//! a category for grouping. The [`LookFeel`] model reads the *effective* value
//! of each setting (Omarchy default overlaid by the user's file) and writes
//! changes into a managed block in the user's `looknfeel.conf`, which Hyprland
//! sources last so those values win. Nothing outside the block is touched.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::configfs::hyprlang::HyprDoc;
use crate::configfs::{atomic_write, CommentStyle, ManagedBlock};
use crate::error::{Result, StudioError};
use crate::omarchy::OmarchyPaths;

/// The kind of a setting's value — drives validation and the editor widget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Bool,
    Enum(&'static [&'static str]),
}

/// Which user config file a setting's managed block lives in. Hyprland merges
/// all sourced files, so this is purely organizational — but input settings
/// belong in `input.conf` (which Omarchy sources) rather than `looknfeel.conf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Looknfeel,
    Input,
}

impl Target {
    /// The user file this target writes to.
    fn user_file(&self) -> &'static str {
        match self {
            Target::Looknfeel => "looknfeel.conf",
            Target::Input => "input.conf",
        }
    }
    /// The Omarchy default file to read base values from.
    fn default_file(&self) -> &'static str {
        match self {
            Target::Looknfeel => "default/hypr/looknfeel.conf",
            Target::Input => "default/hypr/input.conf",
        }
    }
    /// The managed-block marker name in that file.
    fn block_name(&self) -> &'static str {
        match self {
            Target::Looknfeel => "looknfeel",
            Target::Input => "input",
        }
    }
}

/// Every target, for iterating files on load/save.
pub const TARGETS: &[Target] = &[Target::Looknfeel, Target::Input];

/// One editable look & feel setting.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    /// Dotted key, e.g. `decoration.blur.size` (colon form for `hyprctl`).
    pub key: &'static str,
    pub label: &'static str,
    /// UI grouping header / tab.
    pub group: &'static str,
    /// Which config file this setting's override is written to.
    pub target: Target,
    pub kind: Kind,
    /// Hyprland's default, shown when the setting isn't set anywhere.
    pub default: &'static str,
    pub help: &'static str,
}

use Kind::*;
use Target::{Input as In, Looknfeel as Lf};

/// The curated schema. Order is display order within each group; groups appear
/// as tabs in the order first seen here. Every key is verified to exist on the
/// shipped Hyprland (`hyprctl getoption`) — dead keywords are omitted, not
/// written as inert config.
pub const SETTINGS: &[Setting] = &[
    // ── Windows ──
    s(
        "general.gaps_in",
        "Inner gaps",
        "Windows",
        Lf,
        Int { min: 0, max: 60 },
        "5",
        "Space between tiled windows",
    ),
    s(
        "general.gaps_out",
        "Outer gaps",
        "Windows",
        Lf,
        Int { min: 0, max: 100 },
        "10",
        "Space between windows and the screen edge",
    ),
    s(
        "general.border_size",
        "Border width",
        "Windows",
        Lf,
        Int { min: 0, max: 12 },
        "2",
        "Thickness of the window border",
    ),
    s(
        "general.extend_border_grab_area",
        "Border grab area",
        "Windows",
        Lf,
        Int { min: 0, max: 40 },
        "15",
        "Pixels around the border that still grab for resizing",
    ),
    s(
        "general.resize_on_border",
        "Resize on border",
        "Windows",
        Lf,
        Bool,
        "false",
        "Drag a window's border or gap to resize it",
    ),
    s(
        "general.layout",
        "Tiling layout",
        "Windows",
        Lf,
        // `scrolling` is Hyprland 0.56's niri-style layout: new windows open
        // as columns marching right, focus walks the tape.
        Enum(&["dwindle", "master", "scrolling"]),
        "dwindle",
        "How new windows are arranged",
    ),
    // ── Layout ──
    s(
        "dwindle.force_split",
        "Split direction",
        "Layout",
        Lf,
        Int { min: 0, max: 2 },
        "0",
        "0 follows the cursor, 1 always left/top, 2 always right/bottom",
    ),
    s(
        "dwindle.preserve_split",
        "Preserve split",
        "Layout",
        Lf,
        Bool,
        "false",
        "Keep the split direction when windows close",
    ),
    s(
        "dwindle.smart_split",
        "Smart split",
        "Layout",
        Lf,
        Bool,
        "false",
        "Choose the split by where you click in the window",
    ),
    s(
        "master.new_status",
        "Master new window",
        "Layout",
        Lf,
        Enum(&["master", "slave", "inherit"]),
        "slave",
        "Where a new window lands in the master layout",
    ),
    // ── Behavior ──
    s(
        "general.allow_tearing",
        "Allow tearing",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Permit screen tearing for lower-latency games (per-window)",
    ),
    s(
        "misc.focus_on_activate",
        "Focus on activate",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Let apps steal focus when they request activation",
    ),
    s(
        "misc.disable_hyprland_logo",
        "Hide Hyprland logo",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Remove the default background logo on an empty workspace",
    ),
    s(
        "misc.vrr",
        "Variable refresh",
        "Behavior",
        Lf,
        Int { min: 0, max: 2 },
        "0",
        "0 off, 1 on, 2 only for fullscreen",
    ),
    s(
        "misc.middle_click_paste",
        "Middle-click paste",
        "Behavior",
        Lf,
        Bool,
        "true",
        "Paste the primary selection on a middle click",
    ),
    s(
        "misc.enable_swallow",
        "Window swallowing",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Hide a terminal while the GUI app it launched is open",
    ),
    s(
        "misc.key_press_enables_dpms",
        "Wake on key",
        "Behavior",
        Lf,
        Bool,
        "false",
        "A key press wakes the displays from DPMS sleep",
    ),
    s(
        "misc.mouse_move_enables_dpms",
        "Wake on mouse",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Mouse movement wakes the displays from DPMS sleep",
    ),
    s(
        "binds.workspace_back_and_forth",
        "Workspace back-and-forth",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Switching to the current workspace returns to the previous one",
    ),
    s(
        "binds.allow_workspace_cycles",
        "Workspace cycles",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Let previous-workspace cycle through history",
    ),
    s(
        "xwayland.force_zero_scaling",
        "XWayland crisp scaling",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Force scale 1 for XWayland apps to avoid blur (they self-scale)",
    ),
    s(
        "cursor.hide_on_key_press",
        "Hide cursor on type",
        "Behavior",
        Lf,
        Bool,
        "false",
        "Hide the cursor while you're typing",
    ),
    // ── Corners & opacity ──
    s(
        "decoration.rounding",
        "Corner rounding",
        "Corners & opacity",
        Lf,
        Int { min: 0, max: 30 },
        "0",
        "Radius of rounded window corners",
    ),
    s(
        "decoration.active_opacity",
        "Active opacity",
        "Corners & opacity",
        Lf,
        Float { min: 0.0, max: 1.0 },
        "1.0",
        "Opacity of the focused window",
    ),
    s(
        "decoration.inactive_opacity",
        "Inactive opacity",
        "Corners & opacity",
        Lf,
        Float { min: 0.0, max: 1.0 },
        "1.0",
        "Opacity of unfocused windows",
    ),
    s(
        "decoration.fullscreen_opacity",
        "Fullscreen opacity",
        "Corners & opacity",
        Lf,
        Float { min: 0.0, max: 1.0 },
        "1.0",
        "Opacity of a fullscreen window",
    ),
    s(
        "decoration.dim_inactive",
        "Dim inactive",
        "Corners & opacity",
        Lf,
        Bool,
        "false",
        "Darken windows that aren't focused",
    ),
    s(
        "decoration.dim_strength",
        "Dim strength",
        "Corners & opacity",
        Lf,
        Float { min: 0.0, max: 1.0 },
        "0.5",
        "How strongly inactive windows are dimmed",
    ),
    s(
        "decoration.dim_special",
        "Dim special",
        "Corners & opacity",
        Lf,
        Float { min: 0.0, max: 1.0 },
        "0.2",
        "How strongly the special workspace dims what's behind it",
    ),
    // ── Blur ──
    s(
        "decoration.blur.enabled",
        "Blur",
        "Blur",
        Lf,
        Bool,
        "true",
        "Blur behind translucent windows",
    ),
    s(
        "decoration.blur.size",
        "Blur size",
        "Blur",
        Lf,
        Int { min: 1, max: 20 },
        "2",
        "Radius of the background blur",
    ),
    s(
        "decoration.blur.passes",
        "Blur passes",
        "Blur",
        Lf,
        Int { min: 1, max: 6 },
        "2",
        "More passes = smoother, heavier blur",
    ),
    s(
        "decoration.blur.special",
        "Blur special",
        "Blur",
        Lf,
        Bool,
        "false",
        "Also blur behind the special workspace",
    ),
    s(
        "decoration.blur.brightness",
        "Blur brightness",
        "Blur",
        Lf,
        Float { min: 0.0, max: 2.0 },
        "0.8",
        "Brightness of the blurred background",
    ),
    s(
        "decoration.blur.contrast",
        "Blur contrast",
        "Blur",
        Lf,
        Float { min: 0.0, max: 2.0 },
        "0.9",
        "Contrast of the blurred background",
    ),
    s(
        "decoration.blur.noise",
        "Blur noise",
        "Blur",
        Lf,
        Float { min: 0.0, max: 1.0 },
        "0.02",
        "Film-grain noise added over the blur",
    ),
    s(
        "decoration.blur.popups",
        "Blur popups",
        "Blur",
        Lf,
        Bool,
        "false",
        "Blur behind menus and popups too",
    ),
    // ── Shadow ──
    s(
        "decoration.shadow.enabled",
        "Shadow",
        "Shadow",
        Lf,
        Bool,
        "true",
        "Drop shadow under windows",
    ),
    s(
        "decoration.shadow.range",
        "Shadow size",
        "Shadow",
        Lf,
        Int { min: 0, max: 60 },
        "2",
        "How far the shadow spreads",
    ),
    s(
        "decoration.shadow.render_power",
        "Shadow softness",
        "Shadow",
        Lf,
        Int { min: 1, max: 4 },
        "3",
        "Falloff curve of the shadow",
    ),
    // ── Input ──
    s(
        "input.sensitivity",
        "Mouse sensitivity",
        "Input",
        In,
        Float {
            min: -1.0,
            max: 1.0,
        },
        "0",
        "Pointer speed, -1 slowest to 1 fastest (0 = unchanged)",
    ),
    s(
        "input.follow_mouse",
        "Focus follows mouse",
        "Input",
        In,
        Int { min: 0, max: 3 },
        "1",
        "0 off, 1 on, 2 loose, 3 full — how focus tracks the pointer",
    ),
    s(
        "input.accel_profile",
        "Accel profile",
        "Input",
        In,
        Enum(&["adaptive", "flat"]),
        "adaptive",
        "Pointer acceleration curve (flat = 1:1)",
    ),
    s(
        "input.force_no_accel",
        "Disable acceleration",
        "Input",
        In,
        Bool,
        "false",
        "Bypass all pointer acceleration (raw input)",
    ),
    s(
        "input.left_handed",
        "Left-handed",
        "Input",
        In,
        Bool,
        "false",
        "Swap left and right mouse buttons",
    ),
    s(
        "input.repeat_rate",
        "Key repeat rate",
        "Input",
        In,
        Int { min: 1, max: 100 },
        "25",
        "Repeats per second while a key is held",
    ),
    s(
        "input.repeat_delay",
        "Key repeat delay",
        "Input",
        In,
        Int {
            min: 100,
            max: 2000,
        },
        "600",
        "Milliseconds before a held key starts repeating",
    ),
    s(
        "input.numlock_by_default",
        "Numlock on start",
        "Input",
        In,
        Bool,
        "false",
        "Enable Num Lock when Hyprland starts",
    ),
    s(
        "input.natural_scroll",
        "Natural scroll (mouse)",
        "Input",
        In,
        Bool,
        "false",
        "Reverse the mouse scroll direction",
    ),
    s(
        "input.scroll_factor",
        "Scroll speed",
        "Input",
        In,
        Float {
            min: 0.1,
            max: 10.0,
        },
        "1.0",
        "Multiplier for scroll distance",
    ),
    s(
        "input.scroll_method",
        "Scroll method",
        "Input",
        In,
        Enum(&["2fg", "edge", "on_button_down", "no_scroll"]),
        "2fg",
        "How scrolling is detected on the touchpad",
    ),
    // ── Touchpad ──
    s(
        "input.touchpad.natural_scroll",
        "Natural scroll",
        "Touchpad",
        In,
        Bool,
        "false",
        "Reverse the touchpad scroll direction",
    ),
    s(
        "input.touchpad.disable_while_typing",
        "Disable while typing",
        "Touchpad",
        In,
        Bool,
        "true",
        "Ignore the touchpad while the keyboard is active",
    ),
    s(
        "input.touchpad.tap-to-click",
        "Tap to click",
        "Touchpad",
        In,
        Bool,
        "true",
        "A tap on the touchpad registers as a click",
    ),
    s(
        "input.touchpad.drag_lock",
        "Drag lock",
        "Touchpad",
        In,
        Bool,
        "false",
        "Keep dragging after briefly lifting your finger",
    ),
    s(
        "input.touchpad.middle_button_emulation",
        "Middle-button emulation",
        "Touchpad",
        In,
        Bool,
        "false",
        "Press left and right together for a middle click",
    ),
];

#[allow(clippy::too_many_arguments)]
const fn s(
    key: &'static str,
    label: &'static str,
    group: &'static str,
    target: Target,
    kind: Kind,
    default: &'static str,
    help: &'static str,
) -> Setting {
    Setting {
        key,
        label,
        group,
        target,
        kind,
        default,
        help,
    }
}

pub fn lookup(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// A named, complete look — a batch of setting values applied together.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub blurb: &'static str,
    pub values: &'static [(&'static str, &'static str)],
}

/// Hand-tuned look & feel presets (roadmap 0.3.4). Each defines the full set,
/// so switching between them lands on a clean, predictable result.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Omarchy default",
        blurb: "The stock Omarchy look — no Studio overrides",
        values: &[],
    },
    Preset {
        name: "Minimal",
        blurb: "No gaps, no rounding, no effects — pure tiling",
        values: &[
            ("general.gaps_in", "0"),
            ("general.gaps_out", "0"),
            ("general.border_size", "1"),
            ("decoration.rounding", "0"),
            ("decoration.blur.enabled", "false"),
            ("decoration.shadow.enabled", "false"),
            ("decoration.active_opacity", "1.0"),
            ("decoration.inactive_opacity", "1.0"),
        ],
    },
    Preset {
        name: "Cozy",
        blurb: "Generous gaps, rounded corners, soft blur & shadow",
        values: &[
            ("general.gaps_in", "8"),
            ("general.gaps_out", "16"),
            ("general.border_size", "2"),
            ("decoration.rounding", "12"),
            ("decoration.blur.enabled", "true"),
            ("decoration.blur.size", "6"),
            ("decoration.blur.passes", "3"),
            ("decoration.shadow.enabled", "true"),
            ("decoration.shadow.range", "12"),
        ],
    },
    Preset {
        name: "Frosted glass",
        blurb: "Translucent windows over a heavy background blur",
        values: &[
            ("general.gaps_in", "6"),
            ("general.gaps_out", "12"),
            ("decoration.rounding", "10"),
            ("decoration.active_opacity", "0.92"),
            ("decoration.inactive_opacity", "0.80"),
            ("decoration.blur.enabled", "true"),
            ("decoration.blur.size", "8"),
            ("decoration.blur.passes", "4"),
        ],
    },
    Preset {
        name: "Performance",
        blurb: "Effects off for the lightest possible compositing",
        values: &[
            ("general.gaps_in", "4"),
            ("general.gaps_out", "8"),
            ("decoration.rounding", "0"),
            ("decoration.blur.enabled", "false"),
            ("decoration.shadow.enabled", "false"),
            ("decoration.active_opacity", "1.0"),
            ("decoration.inactive_opacity", "1.0"),
            ("decoration.dim_inactive", "false"),
        ],
    },
    Preset {
        name: "Focus",
        blurb: "Dim and fade unfocused windows to spotlight the active one",
        values: &[
            ("general.gaps_in", "6"),
            ("general.gaps_out", "12"),
            ("decoration.rounding", "8"),
            ("decoration.dim_inactive", "true"),
            ("decoration.dim_strength", "0.25"),
            ("decoration.inactive_opacity", "0.90"),
            ("decoration.active_opacity", "1.0"),
        ],
    },
];

pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Path of the preview-active marker. It exists while a live `hyprctl keyword`
/// preview is applied but not yet saved; if Studio dies mid-preview, the next
/// launch sees this and reloads Hyprland to discard the stale live values.
pub fn preview_marker(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("looknfeel.preview-active")
}

pub fn mark_preview_active(state_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(preview_marker(state_dir), b"1");
}

pub fn clear_preview_marker(state_dir: &std::path::Path) {
    let _ = std::fs::remove_file(preview_marker(state_dir));
}

pub fn preview_was_active(state_dir: &std::path::Path) -> bool {
    preview_marker(state_dir).exists()
}

/// Groups in display order (deduplicated, stable).
pub fn groups() -> Vec<&'static str> {
    let mut seen = Vec::new();
    for s in SETTINGS {
        if !seen.contains(&s.group) {
            seen.push(s.group);
        }
    }
    seen
}

impl Kind {
    /// Validate and normalize a candidate value, or return why it's rejected.
    pub fn validate(&self, raw: &str) -> std::result::Result<String, String> {
        let raw = raw.trim();
        match self {
            Kind::Int { min, max } => {
                let v: i64 = raw
                    .parse()
                    .map_err(|_| format!("“{raw}” isn't a whole number"))?;
                if v < *min || v > *max {
                    return Err(format!("must be between {min} and {max}"));
                }
                Ok(v.to_string())
            }
            Kind::Float { min, max } => {
                let v: f64 = raw.parse().map_err(|_| format!("“{raw}” isn't a number"))?;
                if v < *min || v > *max {
                    return Err(format!("must be between {min} and {max}"));
                }
                Ok(format!("{v}"))
            }
            Kind::Bool => match raw.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Ok("true".into()),
                "false" | "no" | "off" | "0" => Ok("false".into()),
                _ => Err("must be on or off".into()),
            },
            Kind::Enum(opts) => {
                if opts.contains(&raw) {
                    Ok(raw.to_string())
                } else {
                    Err(format!("must be one of: {}", opts.join(", ")))
                }
            }
        }
    }
}

/// The look & feel model: effective base values overlaid by Studio overrides.
pub struct LookFeel {
    /// Effective values from Omarchy default + the user's own (non-Studio) file.
    base: BTreeMap<String, String>,
    /// Studio's overrides, written to the managed block. Dotted key → value.
    overrides: BTreeMap<String, String>,
}

fn block(target: Target) -> ManagedBlock {
    ManagedBlock::new(target.block_name(), CommentStyle::Hash)
}

fn user_path(paths: &OmarchyPaths, target: Target) -> PathBuf {
    paths.hypr_config().join(target.user_file())
}

fn default_path(paths: &OmarchyPaths, target: Target) -> PathBuf {
    paths.system.join(target.default_file())
}

impl LookFeel {
    pub fn load(paths: &OmarchyPaths) -> Self {
        let mut base = BTreeMap::new();
        let mut overrides = BTreeMap::new();

        for &target in TARGETS {
            // Base: the Omarchy default file overlaid by the user's own file,
            // with our managed block stripped so we read the user's values not
            // ours. A key can appear in either file (e.g. `misc` lives in both).
            for path in [default_path(paths, target), user_path(paths, target)] {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let cleaned = block(target).remove(&text);
                    let doc = HyprDoc::parse(&cleaned);
                    for setting in SETTINGS {
                        if let Some(v) = doc.get(setting.key) {
                            base.insert(setting.key.to_string(), strip_inline_comment(&v));
                        }
                    }
                }
            }
            // Overrides: whatever is in this target's managed block already.
            if let Ok(text) = std::fs::read_to_string(user_path(paths, target)) {
                if let Some(body) = block(target).extract(&text) {
                    let doc = HyprDoc::parse(body);
                    for e in doc.entries() {
                        overrides.insert(e.dotted(), e.value);
                    }
                }
            }
        }

        Self { base, overrides }
    }

    /// The effective value shown to the user: override, else base, else the
    /// schema default.
    pub fn value(&self, key: &str) -> String {
        self.overrides
            .get(key)
            .or_else(|| self.base.get(key))
            .cloned()
            .unwrap_or_else(|| {
                lookup(key)
                    .map(|s| s.default.to_string())
                    .unwrap_or_default()
            })
    }

    pub fn is_overridden(&self, key: &str) -> bool {
        self.overrides.contains_key(key)
    }

    /// Set (and validate) an override. Returns the rejection reason on failure.
    pub fn set(&mut self, key: &str, raw: &str) -> std::result::Result<(), String> {
        let setting = lookup(key).ok_or_else(|| format!("unknown setting {key}"))?;
        let normalized = setting.kind.validate(raw)?;
        self.overrides.insert(key.to_string(), normalized);
        Ok(())
    }

    /// Drop an override, reverting to the base/default value.
    pub fn clear(&mut self, key: &str) {
        self.overrides.remove(key);
    }

    /// Drop every override (Reset All).
    pub fn clear_all(&mut self) {
        self.overrides.clear();
    }

    pub fn any_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }

    /// Apply one setting live via `hyprctl keyword`, without writing anything —
    /// the preview primitive. Ephemeral: undone by the next reload.
    pub fn preview_one(&self, key: &str, runner: &dyn crate::cmd::CommandRunner) -> Result<()> {
        let value = self.value(key);
        let out = runner.run(&crate::omarchy::cmds::hypr_keyword(&colon_key(key), &value))?;
        if !out.ok() {
            return Err(StudioError::External {
                cmd: format!("hyprctl keyword {}", colon_key(key)),
                detail: out.stderr.trim().to_string(),
            });
        }
        Ok(())
    }

    /// Apply every setting live in one batch (a preset "try"). Best-effort:
    /// the first failure is returned, but the rest are still attempted.
    pub fn preview_all(&self, runner: &dyn crate::cmd::CommandRunner) -> Result<()> {
        let mut first_err = None;
        for s in SETTINGS {
            if let Err(e) = self.preview_one(s.key, runner) {
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Replace all overrides with a preset's values (a clean, known look).
    pub fn apply_preset(&mut self, preset: &Preset) {
        self.overrides.clear();
        for (key, value) in preset.values {
            // presets are authored against the schema, so this always validates
            let _ = self.set(key, value);
        }
    }

    /// Render one target's managed block body as nested category blocks, from
    /// the overrides whose settings belong to that target. Returns None when
    /// this target has no overrides (its block should be removed).
    pub fn render_block_body(&self, target: Target) -> Option<String> {
        let entries: Vec<(Vec<String>, String)> = self
            .overrides
            .iter()
            .filter(|(k, _)| lookup(k).map(|s| s.target) == Some(target))
            .map(|(k, v)| (k.split('.').map(str::to_string).collect(), v.clone()))
            .collect();
        if entries.is_empty() {
            return None;
        }
        let mut out = String::from("# Managed by Omarchy Studio — your look & feel tweaks.\n");
        render_nested(&entries, 0, &mut out);
        Some(out.trim_end().to_string())
    }

    /// The user files Studio may write for these settings — snapshot these
    /// before applying so undo restores every touched file.
    pub fn managed_paths(paths: &OmarchyPaths) -> Vec<PathBuf> {
        TARGETS.iter().map(|&t| user_path(paths, t)).collect()
    }

    /// Persist each target's managed block (a target with no overrides has its
    /// block removed). Returns the files written.
    pub fn save(&self, paths: &OmarchyPaths) -> Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for &target in TARGETS {
            let path = user_path(paths, target);
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            let updated = match self.render_block_body(target) {
                Some(body) => block(target).upsert(&existing, &body),
                None => block(target).remove(&existing),
            };
            if updated != existing {
                atomic_write(&path, &updated)?;
                written.push(path);
            }
        }
        Ok(written)
    }

    /// Plan each target's managed block as a pipeline edit, writing nothing.
    /// Targets whose content wouldn't change are omitted, so an apply that
    /// changes one file doesn't snapshot the other.
    pub fn plan(&self, paths: &OmarchyPaths) -> Vec<crate::engine::FileEdit> {
        let mut edits = Vec::new();
        for &target in TARGETS {
            let path = user_path(paths, target);
            // `None` for a file that doesn't exist yet — the pipeline's hash
            // guard reads the same way, and treating a missing file as empty
            // here would make the guard reject the write.
            let on_disk = std::fs::read_to_string(&path).ok();
            let existing = on_disk.clone().unwrap_or_default();
            let updated = match self.render_block_body(target) {
                Some(body) => block(target).upsert(&existing, &body),
                None => block(target).remove(&existing),
            };
            if updated != existing {
                edits.push(crate::engine::FileEdit::new(
                    path,
                    on_disk.as_deref(),
                    updated,
                ));
            }
        }
        edits
    }

    /// Apply through the pipeline: drift-check → pre-snapshot → hash-guarded
    /// write → `hyprctl reload` → verify → post, rolling back if the config we
    /// produced doesn't load. The caller does *not* snapshot; the pipeline
    /// owns that, and doing both would double every entry in the log.
    pub fn apply(
        &self,
        paths: &OmarchyPaths,
        store: &crate::snapshot::SnapshotStore,
        runner: &dyn crate::cmd::CommandRunner,
        summary: &str,
    ) -> Result<crate::engine::ApplyReport> {
        let plan = crate::engine::ApplyPlan {
            summary: summary.to_string(),
            module: "looknfeel".into(),
            edits: self.plan(paths),
            reload: vec![crate::engine::ReloadStep::HyprReload],
            verify: crate::engine::hypr_verification(runner),
            risk: crate::engine::Risk::Risky,
            trailers: Vec::new(),
        };
        crate::engine::Pipeline::new(store, runner).apply(&plan, false)
    }
}

/// Strip a hyprlang inline comment from a value read out of a config file:
/// `0 # -1.0 - 1.0, means no change` → `0`. A `#` only starts a comment when
/// preceded by whitespace (or at the start), so values are left intact.
fn strip_inline_comment(v: &str) -> String {
    let bytes = v.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return v[..i].trim().to_string();
        }
    }
    v.trim().to_string()
}

/// Hyprland's colon form of a dotted key: `decoration.blur.size` →
/// `decoration:blur:size` (what `hyprctl keyword` expects).
pub fn colon_key(dotted: &str) -> String {
    dotted.replace('.', ":")
}

/// Emit `path…value` entries as nested `category { … }` blocks. Entries whose
/// remaining path is a single segment are leaves (`key = value`); longer paths
/// nest under their first segment.
fn render_nested(entries: &[(Vec<String>, String)], depth: usize, out: &mut String) {
    let indent = "    ".repeat(depth);
    // leaves at this level
    for (path, value) in entries.iter().filter(|(p, _)| p.len() == 1) {
        out.push_str(&format!("{indent}{} = {value}\n", path[0]));
    }
    // group the deeper entries by their first segment, preserving first-seen order
    let mut order: Vec<String> = Vec::new();
    for (path, _) in entries.iter().filter(|(p, _)| p.len() > 1) {
        if !order.contains(&path[0]) {
            order.push(path[0].clone());
        }
    }
    for cat in order {
        out.push_str(&format!("{indent}{cat} {{\n"));
        let children: Vec<(Vec<String>, String)> = entries
            .iter()
            .filter(|(p, _)| p.len() > 1 && p[0] == cat)
            .map(|(p, v)| (p[1..].to_vec(), v.clone()))
            .collect();
        render_nested(&children, depth + 1, out);
        out.push_str(&format!("{indent}}}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_coherent() {
        for s in SETTINGS {
            // defaults must validate against their own kind
            assert!(
                s.kind.validate(s.default).is_ok(),
                "default {:?} invalid for {}",
                s.default,
                s.key
            );
        }
        // keys are unique
        let mut seen = std::collections::HashSet::new();
        for s in SETTINGS {
            assert!(seen.insert(s.key), "duplicate key {}", s.key);
        }
        assert!(lookup("decoration.blur.size").is_some());
        assert!(groups().contains(&"Blur"));
        assert!(groups().contains(&"Input"));
        assert!(groups().contains(&"Touchpad"));
        assert_eq!(colon_key("decoration.blur.size"), "decoration:blur:size");
        // input settings target input.conf; the rest target looknfeel.conf
        assert_eq!(lookup("input.repeat_rate").unwrap().target, Target::Input);
        assert_eq!(
            lookup("input.touchpad.tap-to-click").unwrap().target,
            Target::Input
        );
        assert_eq!(lookup("general.gaps_in").unwrap().target, Target::Looknfeel);
    }

    #[test]
    fn validation_enforces_kind_and_range() {
        let int = Kind::Int { min: 0, max: 10 };
        assert_eq!(int.validate("7").unwrap(), "7");
        assert!(int.validate("20").is_err());
        assert!(int.validate("x").is_err());
        assert_eq!(Kind::Bool.validate("YES").unwrap(), "true");
        assert_eq!(Kind::Bool.validate("off").unwrap(), "false");
        let en = Kind::Enum(&["dwindle", "master"]);
        assert!(en.validate("master").is_ok());
        assert!(en.validate("floating").is_err());
    }

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-lnf-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("sys/omarchy/default/hypr")).unwrap();
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("sys/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    #[test]
    fn effective_value_overlays_default_user_and_override() {
        let paths = fake_paths("effective");
        std::fs::write(
            paths.system.join("default/hypr/looknfeel.conf"),
            "general {\n    gaps_in = 5\n    border_size = 2\n}\n",
        )
        .unwrap();
        // user's own file bumps border_size; no Studio block yet
        std::fs::write(
            user_path(&paths, Target::Looknfeel),
            "general {\n    border_size = 4\n}\n",
        )
        .unwrap();

        let mut lf = LookFeel::load(&paths);
        assert_eq!(lf.value("general.gaps_in"), "5"); // from default
        assert_eq!(lf.value("general.border_size"), "4"); // user overrides default
        assert_eq!(lf.value("decoration.rounding"), "0"); // schema default (absent)
        assert!(!lf.is_overridden("general.gaps_in"));

        // set an override, save, and confirm it round-trips + wins
        lf.set("general.gaps_in", "12").unwrap();
        assert!(lf.set("general.gaps_in", "999").is_err()); // out of range, unchanged
        lf.save(&paths).unwrap();

        let reloaded = LookFeel::load(&paths);
        assert_eq!(reloaded.value("general.gaps_in"), "12");
        assert!(reloaded.is_overridden("general.gaps_in"));
        assert_eq!(reloaded.value("general.border_size"), "4"); // user's own preserved
        let on_disk = std::fs::read_to_string(user_path(&paths, Target::Looknfeel)).unwrap();
        assert!(on_disk.contains("border_size = 4")); // untouched
        assert!(on_disk.contains("omarchy-studio:looknfeel"));

        // clearing all overrides removes the block, restoring the user's file
        let mut r2 = LookFeel::load(&paths);
        r2.clear("general.gaps_in");
        r2.save(&paths).unwrap();
        let after = std::fs::read_to_string(user_path(&paths, Target::Looknfeel)).unwrap();
        assert!(!after.contains("omarchy-studio:looknfeel"));
        assert!(after.contains("border_size = 4"));
    }

    #[test]
    fn base_value_strips_inline_comment() {
        assert_eq!(strip_inline_comment("0 # -1.0 - 1.0, no change"), "0");
        assert_eq!(strip_inline_comment("  5\t# gaps"), "5");
        assert_eq!(strip_inline_comment("dwindle"), "dwindle");
        assert_eq!(strip_inline_comment("rgba(11223344)"), "rgba(11223344)");

        // and it flows through base reading: the Omarchy default's commented
        // sensitivity line resolves to a clean, nudge-able value.
        let paths = fake_paths("inline");
        std::fs::write(
            paths.system.join("default/hypr/input.conf"),
            "input {\n    sensitivity = 0 # -1.0 - 1.0, 0 means no modification.\n}\n",
        )
        .unwrap();
        let lf = LookFeel::load(&paths);
        assert_eq!(lf.value("input.sensitivity"), "0");
    }

    #[test]
    fn input_settings_write_to_input_conf() {
        let paths = fake_paths("targets");
        // an input override and a looknfeel override
        let mut lf = LookFeel::load(&paths);
        lf.set("input.repeat_rate", "35").unwrap();
        lf.set("input.touchpad.tap-to-click", "false").unwrap();
        lf.set("general.gaps_in", "9").unwrap();
        let written = lf.save(&paths).unwrap();
        assert_eq!(written.len(), 2); // both files touched

        let input_conf = std::fs::read_to_string(user_path(&paths, Target::Input)).unwrap();
        let lnf_conf = std::fs::read_to_string(user_path(&paths, Target::Looknfeel)).unwrap();
        // input keys land in input.conf, not looknfeel.conf
        assert!(input_conf.contains("omarchy-studio:input"));
        assert!(input_conf.contains("repeat_rate = 35"));
        assert!(input_conf.contains("tap-to-click = false")); // hyphen key survives
        assert!(!input_conf.contains("gaps_in"));
        assert!(lnf_conf.contains("omarchy-studio:looknfeel"));
        assert!(lnf_conf.contains("gaps_in = 9"));
        assert!(!lnf_conf.contains("repeat_rate"));

        // round-trips back from the split files, including the hyphenated key
        let reloaded = LookFeel::load(&paths);
        assert_eq!(reloaded.value("input.repeat_rate"), "35");
        assert_eq!(reloaded.value("input.touchpad.tap-to-click"), "false");
        assert_eq!(reloaded.value("general.gaps_in"), "9");
        assert!(reloaded.is_overridden("input.touchpad.tap-to-click"));

        // clearing only the input overrides removes the input block, keeps lnf
        let mut r = reloaded;
        r.clear("input.repeat_rate");
        r.clear("input.touchpad.tap-to-click");
        r.save(&paths).unwrap();
        let input_after = std::fs::read_to_string(user_path(&paths, Target::Input)).unwrap();
        assert!(!input_after.contains("omarchy-studio:input"));
        let lnf_after = std::fs::read_to_string(user_path(&paths, Target::Looknfeel)).unwrap();
        assert!(lnf_after.contains("gaps_in = 9"));
    }

    #[test]
    fn presets_are_valid_and_apply_cleanly() {
        // every preset value must reference a real setting and validate
        for p in PRESETS {
            for (key, value) in p.values {
                let s = lookup(key).unwrap_or_else(|| panic!("preset {} bad key {key}", p.name));
                assert!(
                    s.kind.validate(value).is_ok(),
                    "preset {} value {value} invalid for {key}",
                    p.name
                );
            }
        }
        // applying replaces overrides wholesale
        let paths = fake_paths("preset");
        std::fs::write(user_path(&paths, Target::Looknfeel), "").unwrap();
        let mut lf = LookFeel::load(&paths);
        lf.set("general.gaps_in", "40").unwrap();
        lf.apply_preset(preset("Minimal").unwrap());
        assert_eq!(lf.value("general.gaps_in"), "0"); // preset wins, old override gone
        assert_eq!(lf.value("decoration.blur.enabled"), "false");
        // "Omarchy default" clears everything
        lf.apply_preset(preset("Omarchy default").unwrap());
        assert!(!lf.any_overrides());
    }

    #[test]
    fn preview_marker_lifecycle() {
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        assert!(!preview_was_active(&dir));
        mark_preview_active(&dir);
        assert!(preview_was_active(&dir));
        clear_preview_marker(&dir);
        assert!(!preview_was_active(&dir));
    }

    #[test]
    fn nested_block_renders_categories() {
        let entries = vec![
            (vec!["general".into(), "gaps_in".into()], "8".to_string()),
            (
                vec!["decoration".into(), "rounding".into()],
                "12".to_string(),
            ),
            (
                vec!["decoration".into(), "blur".into(), "size".into()],
                "4".to_string(),
            ),
        ];
        let mut out = String::new();
        render_nested(&entries, 0, &mut out);
        assert!(out.contains("general {\n    gaps_in = 8\n}"));
        assert!(out.contains("decoration {\n    rounding = 12\n"));
        assert!(out.contains("    blur {\n        size = 4\n    }"));
        // and it parses back as valid hyprlang with the right dotted keys
        let doc = HyprDoc::parse(&out);
        assert_eq!(doc.get("decoration.blur.size").as_deref(), Some("4"));
        assert_eq!(doc.get("general.gaps_in").as_deref(), Some("8"));
    }
}

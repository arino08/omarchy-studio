//! Monitors (roadmap 0.8.4).
//!
//! Detect displays from `hyprctl monitors -j`, arrange them, and persist the
//! layout to `~/.config/hypr/monitors.conf` as `monitor=` lines inside a
//! Studio-owned managed block — always with a `monitor=,preferred,auto,1`
//! hotplug fallback so an unplugged/added display still comes up. Writes go
//! through the snapshot pipeline (pre-snapshot → write → `hyprctl reload` →
//! re-query verify → rollback), which a-la-carchy's monitor wizard lacks.
//!
//! The layout math (effective size under scale and rotation, absolute
//! placement) is pure and unit-tested; the frontend drives it.

use crate::cmd::{Cmd, CommandRunner};
use crate::configfs::{CommentStyle, ManagedBlock};
use crate::error::{Result, StudioError};
use crate::omarchy::OmarchyPaths;
use serde::Deserialize;
use std::path::PathBuf;

/// A live display as reported by Hyprland.
#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub make: String,
    #[serde(default)]
    pub model: String,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "refreshRate", default)]
    pub refresh_rate: f64,
    pub x: i32,
    pub y: i32,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub transform: u8,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(rename = "dpmsStatus", default)]
    pub dpms: bool,
}

fn one() -> f64 {
    1.0
}

impl Monitor {
    /// Laptop panels are `eDP-*` (embedded DisplayPort) or `LVDS-*`.
    pub fn is_laptop(&self) -> bool {
        let n = self.name.to_ascii_uppercase();
        n.starts_with("EDP") || n.starts_with("LVDS")
    }

    /// A human make/model label, falling back to the connector name.
    pub fn label(&self) -> String {
        let mm = format!("{} {}", self.make, self.model);
        let mm = mm.trim();
        if mm.is_empty() {
            self.name.clone()
        } else {
            mm.to_string()
        }
    }

    /// The logical size the compositor lays out with: native resolution divided
    /// by scale, with width/height swapped under a 90°/270° rotation.
    pub fn effective_size(&self) -> (u32, u32) {
        effective_size(self.width, self.height, self.scale, self.transform)
    }

    /// The mode string for a `monitor=` line, e.g. `1920x1080@120.21`.
    pub fn mode(&self) -> String {
        if self.refresh_rate > 0.0 {
            format!("{}x{}@{:.2}", self.width, self.height, self.refresh_rate)
        } else {
            format!("{}x{}", self.width, self.height)
        }
    }
}

/// Native size under `scale` and `transform` (Hyprland's 0–7 codes). Odd codes
/// (1/3 = 90°/270°, 5/7 = flipped-90/270) swap width and height.
pub fn effective_size(width: u32, height: u32, scale: f64, transform: u8) -> (u32, u32) {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let w = (width as f64 / s).round() as u32;
    let h = (height as f64 / s).round() as u32;
    if matches!(transform, 1 | 3 | 5 | 7) {
        (h, w)
    } else {
        (w, h)
    }
}

/// Parse `hyprctl monitors -j`.
pub fn parse(json: &str) -> Result<Vec<Monitor>> {
    serde_json::from_str(json).map_err(|e| StudioError::External {
        cmd: "hyprctl monitors -j".into(),
        detail: e.to_string(),
    })
}

/// Query the live monitor list.
pub fn load(runner: &dyn CommandRunner) -> Result<Vec<Monitor>> {
    let out = runner.run(&Cmd::new("hyprctl").args(["monitors", "-j"]))?;
    if !out.ok() {
        return Err(StudioError::External {
            cmd: "hyprctl monitors -j".into(),
            detail: out.stderr.trim().to_string(),
        });
    }
    parse(&out.stdout)
}

// ------------------------------------------------------------------ layout

/// Where to put a display relative to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    LeftOf,
    RightOf,
    Above,
    Below,
}

/// Position `size` (a display's effective w×h) relative to an already-placed
/// `base` rect (x, y, w, h), touching on the given side.
pub fn place(base: (i32, i32, u32, u32), size: (u32, u32), side: Side) -> (i32, i32) {
    let (bx, by, bw, bh) = base;
    let (w, h) = size;
    match side {
        Side::RightOf => (bx + bw as i32, by),
        Side::LeftOf => (bx - w as i32, by),
        Side::Below => (bx, by + bh as i32),
        Side::Above => (bx, by - h as i32),
    }
}

/// Left-to-right row layout: place displays touching at y = 0 in order,
/// returning each one's top-left position. Uses effective sizes.
pub fn row_positions(sizes: &[(u32, u32)]) -> Vec<(i32, i32)> {
    let mut out = Vec::with_capacity(sizes.len());
    let mut x = 0i32;
    for &(w, _) in sizes {
        out.push((x, 0));
        x += w as i32;
    }
    out
}

// ------------------------------------------------------------------ config

/// One display's desired settings, rendered to a `monitor=` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSetting {
    pub name: String,
    /// `WxH@Hz` or `preferred`.
    pub mode: String,
    pub x: i32,
    pub y: i32,
    /// Scale rendered to two decimals (`1`, `1.5`, `1.25`).
    pub scale: String,
    pub transform: u8,
    pub disabled: bool,
}

impl MonitorSetting {
    /// Snapshot a live monitor's current state as an editable setting.
    pub fn from_monitor(m: &Monitor) -> Self {
        Self {
            name: m.name.clone(),
            mode: m.mode(),
            x: m.x,
            y: m.y,
            scale: fmt_scale(m.scale),
            transform: m.transform,
            disabled: m.disabled,
        }
    }

    /// The `monitor=` directive for this display.
    pub fn line(&self) -> String {
        if self.disabled {
            return format!("monitor = {}, disable", self.name);
        }
        format!(
            "monitor = {}, {}, {}x{}, {}, transform, {}",
            self.name, self.mode, self.x, self.y, self.scale, self.transform
        )
    }
}

/// Trim a scale float to the shortest exact-looking form (`1`, `1.5`).
pub fn fmt_scale(scale: f64) -> String {
    let s = format!("{scale:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// A full monitors layout: one setting per display plus the hotplug fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub monitors: Vec<MonitorSetting>,
}

impl Layout {
    pub fn from_monitors(mons: &[Monitor]) -> Self {
        Self {
            monitors: mons.iter().map(MonitorSetting::from_monitor).collect(),
        }
    }

    /// The managed-block body: one `monitor=` line per display, then a
    /// `monitor=,preferred,auto,1` catch-all so a hotplugged/unknown display
    /// still lights up.
    pub fn render_body(&self) -> String {
        let mut lines: Vec<String> = self.monitors.iter().map(MonitorSetting::line).collect();
        lines.push("monitor = , preferred, auto, 1".to_string());
        lines.join("\n")
    }
}

/// `~/.config/hypr/monitors.conf` — Omarchy sources it; our managed block wins.
pub fn conf_path(paths: &OmarchyPaths) -> PathBuf {
    paths.hypr_config().join("monitors.conf")
}

fn block() -> ManagedBlock {
    ManagedBlock::new("monitors", CommentStyle::Hash)
}

/// Upsert the layout's managed block into `monitors.conf` (leaving the user's
/// own lines outside it untouched) and return the written text. Does not touch
/// disk — the frontend snapshots then writes.
pub fn render_conf(existing: &str, layout: &Layout) -> String {
    block().upsert(existing, &layout.render_body())
}

/// Read the current on-disk conf (empty string if absent).
pub fn read_conf(paths: &OmarchyPaths) -> String {
    std::fs::read_to_string(conf_path(paths)).unwrap_or_default()
}

/// Plan the managed block as a pipeline edit, writing nothing. `None` when the
/// conf already describes exactly this layout.
pub fn plan(paths: &OmarchyPaths, layout: &Layout) -> Option<crate::engine::FileEdit> {
    let path = conf_path(paths);
    // `None` for a file that doesn't exist yet — the pipeline's hash guard
    // reads it the same way, and treating absent as empty makes it reject.
    let on_disk = std::fs::read_to_string(&path).ok();
    let existing = on_disk.clone().unwrap_or_default();
    let updated = render_conf(&existing, layout);
    (updated != existing).then(|| crate::engine::FileEdit::new(path, on_disk.as_deref(), updated))
}

/// Apply a layout through the pipeline: snapshot, hash-guarded write, reload,
/// verify, and roll back if the monitor lines stop Hyprland's config loading.
///
/// Writing a layout is useful even where Hyprland isn't running — you can set
/// up displays from a TTY — so when `hyprctl` isn't usable the plan carries
/// neither a reload nor a check, and the write still happens. That keeps the
/// old CLI behaviour ("wrote it, reload unavailable") rather than failing.
pub fn apply(
    paths: &OmarchyPaths,
    layout: &Layout,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn CommandRunner,
    summary: &str,
) -> Result<bool> {
    let Some(edit) = plan(paths, layout) else {
        return Ok(false); // already this layout
    };
    // One probe decides both: no usable hyprctl means nothing to reload and
    // nothing that could verify the result.
    let verify = crate::engine::hypr_verification(runner);
    let reload = if verify.is_empty() {
        Vec::new()
    } else {
        vec![crate::engine::ReloadStep::HyprReload]
    };
    let plan = crate::engine::ApplyPlan {
        summary: summary.to_string(),
        module: "monitors".into(),
        edits: vec![edit],
        reload,
        verify,
        risk: crate::engine::Risk::Risky,
        trailers: Vec::new(),
    };
    crate::engine::Pipeline::new(store, runner).apply(&plan, false)?;
    Ok(true)
}

// ------------------------------------------------------------------ commands

/// `hyprctl notify` to flash a monitor's name on-screen (the identify action).
pub fn identify_cmd(index: usize, name: &str) -> Cmd {
    Cmd::new("hyprctl").args([
        "notify".to_string(),
        index.to_string(),
        "2000".to_string(),
        "0".to_string(),
        format!("Studio: {name}"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = r#"[
      {"id":0,"name":"eDP-1","description":"Lenovo 0x9059","make":"Lenovo","model":"0x9059",
       "width":1920,"height":1080,"refreshRate":120.213,"x":0,"y":0,"scale":1.0,"transform":0,
       "focused":true,"disabled":false,"dpmsStatus":false},
      {"id":1,"name":"HDMI-A-1","description":"Lenovo D22-20","make":"Lenovo","model":"D22-20",
       "width":1920,"height":1080,"refreshRate":60.0,"x":1920,"y":0,"scale":1.0,"transform":0,
       "focused":false,"disabled":false,"dpmsStatus":false}
    ]"#;

    #[test]
    fn parses_hyprctl_json() {
        let mons = parse(TWO).unwrap();
        assert_eq!(mons.len(), 2);
        assert_eq!(mons[0].name, "eDP-1");
        assert!(mons[0].is_laptop());
        assert!(!mons[1].is_laptop());
        assert_eq!(mons[0].mode(), "1920x1080@120.21");
    }

    #[test]
    fn effective_size_scales_and_rotates() {
        assert_eq!(effective_size(3840, 2160, 2.0, 0), (1920, 1080));
        assert_eq!(effective_size(1920, 1080, 1.0, 0), (1920, 1080));
        // 90° rotation swaps axes.
        assert_eq!(effective_size(1920, 1080, 1.0, 1), (1080, 1920));
        assert_eq!(effective_size(1920, 1080, 1.0, 3), (1080, 1920));
        // fractional scale rounds.
        assert_eq!(effective_size(2560, 1440, 1.25, 0), (2048, 1152));
    }

    #[test]
    fn place_computes_touching_coords() {
        let base = (0, 0, 1920, 1080);
        assert_eq!(place(base, (1080, 1920), Side::RightOf), (1920, 0));
        assert_eq!(place(base, (1080, 1920), Side::LeftOf), (-1080, 0));
        assert_eq!(place(base, (1920, 1080), Side::Below), (0, 1080));
        assert_eq!(place(base, (1920, 1080), Side::Above), (0, -1080));
    }

    #[test]
    fn row_positions_lay_out_left_to_right() {
        let pos = row_positions(&[(1920, 1080), (2560, 1440), (1080, 1920)]);
        assert_eq!(pos, vec![(0, 0), (1920, 0), (4480, 0)]);
    }

    #[test]
    fn fmt_scale_trims() {
        assert_eq!(fmt_scale(1.0), "1");
        assert_eq!(fmt_scale(1.5), "1.5");
        assert_eq!(fmt_scale(1.25), "1.25");
    }

    #[test]
    fn setting_line_and_disable() {
        let mons = parse(TWO).unwrap();
        let s = MonitorSetting::from_monitor(&mons[0]);
        assert_eq!(
            s.line(),
            "monitor = eDP-1, 1920x1080@120.21, 0x0, 1, transform, 0"
        );
        let mut off = s.clone();
        off.disabled = true;
        assert_eq!(off.line(), "monitor = eDP-1, disable");
    }

    #[test]
    fn render_body_includes_hotplug_fallback() {
        let mons = parse(TWO).unwrap();
        let layout = Layout::from_monitors(&mons);
        let body = layout.render_body();
        assert!(body.contains("monitor = eDP-1,"));
        assert!(body.contains("monitor = HDMI-A-1,"));
        assert!(body.trim_end().ends_with("monitor = , preferred, auto, 1"));
    }

    #[test]
    fn render_conf_preserves_user_lines() {
        let mons = parse(TWO).unwrap();
        let layout = Layout::from_monitors(&mons);
        let existing = "# my own note\nmonitor = DP-9, disable\n";
        let out = render_conf(existing, &layout);
        assert!(out.contains("# my own note"));
        assert!(out.contains("monitor = DP-9, disable"));
        assert!(out.contains("omarchy-studio:monitors"));
        assert!(out.contains("monitor = eDP-1,"));
        // Re-rendering is idempotent (managed block replaced, not duplicated).
        let again = render_conf(&out, &layout);
        assert_eq!(again.matches("omarchy-studio:monitors").count(), 2); // open+close
    }
}

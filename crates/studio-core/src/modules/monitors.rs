//! Monitors (roadmap 0.8.4).
//!
//! Detect displays from `hyprctl monitors -j`, arrange them, and persist the
//! layout to `~/.config/hypr/monitors.conf` as `monitor=` lines inside a
//! Studio-owned managed block — always with a `monitor=,preferred,auto,1`
//! hotplug fallback so an unplugged/added display still comes up. Writes go
//! through the snapshot pipeline (pre-snapshot → write → `hyprctl reload` →
//! re-query verify → rollback), which a-la-carchy's monitor wizard lacks.
//!
//! Modes (resolution + refresh rate) come from Hyprland's `availableModes`, so
//! a requested mode is checked against what the panel advertises before it is
//! written — an unsupported rate returns the list of real ones instead of a
//! `monitor=` line Hyprland would quietly fall back from.
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
    /// Every mode the EDID advertises, as Hyprland prints them
    /// (`"3440x1440@100.00Hz"`). Empty on a Hyprland too old to report them.
    #[serde(rename = "availableModes", default)]
    pub available_modes: Vec<String>,
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

// ------------------------------------------------------------------- modes

/// One entry from a display's `availableModes` — a resolution plus the refresh
/// rate it can run at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
}

impl Mode {
    /// The `WxH@Hz` form a `monitor=` line takes.
    pub fn to_spec(&self) -> String {
        format!("{}x{}@{:.2}", self.width, self.height, self.refresh)
    }

    /// How it reads in a list: `3440x1440 @ 100Hz`.
    pub fn label(&self) -> String {
        format!(
            "{}x{} @ {}Hz",
            self.width,
            self.height,
            fmt_scale(self.refresh)
        )
    }

    /// Two modes are the same mode when they agree to the tenth of a hertz —
    /// Hyprland reports 100.00 where the kernel says 99.998.
    pub fn matches(&self, other: &Mode) -> bool {
        self.width == other.width
            && self.height == other.height
            && (self.refresh - other.refresh).abs() < 0.1
    }

    /// Parse `"3440x1440@100.00Hz"`, `"3440x1440@100"`, or `"3440x1440"`.
    pub fn parse(s: &str) -> Option<Mode> {
        let s = s
            .trim()
            .trim_end_matches("Hz")
            .trim_end_matches("hz")
            .trim();
        let (res, rate) = match s.split_once('@') {
            Some((r, hz)) => (r, hz.trim().parse::<f64>().ok()?),
            None => (s, 0.0),
        };
        let (w, h) = res.trim().split_once('x')?;
        Some(Mode {
            width: w.trim().parse().ok()?,
            height: h.trim().parse().ok()?,
            refresh: rate,
        })
    }
}

/// What the user asked for on the command line or in the TUI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModeRequest {
    /// `preferred` — hand the choice back to Hyprland.
    Preferred,
    /// A resolution, with the highest refresh available there.
    Resolution(u32, u32),
    /// A resolution at a specific refresh.
    Exact(Mode),
    /// Just a refresh rate — keep whatever resolution is current.
    Refresh(f64),
}

impl ModeRequest {
    /// Read a user-typed spec: `preferred`, `3440x1440`, `3440x1440@100`, `100`.
    pub fn parse(s: &str) -> Option<ModeRequest> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("preferred") || s.eq_ignore_ascii_case("auto") {
            return Some(ModeRequest::Preferred);
        }
        if let Some(m) = Mode::parse(s) {
            return Some(if m.refresh > 0.0 {
                ModeRequest::Exact(m)
            } else {
                ModeRequest::Resolution(m.width, m.height)
            });
        }
        // A bare number is a refresh rate: `monitor rate HDMI-A-1 100`.
        let hz = s.trim_end_matches("Hz").trim_end_matches("hz").trim();
        hz.parse::<f64>()
            .ok()
            .filter(|h| *h > 0.0)
            .map(ModeRequest::Refresh)
    }
}

impl Monitor {
    /// `availableModes` parsed, deduplicated, and ordered the way a picker wants
    /// them: biggest resolution first, fastest refresh first within it.
    pub fn modes(&self) -> Vec<Mode> {
        let mut modes: Vec<Mode> = self
            .available_modes
            .iter()
            .filter_map(|s| Mode::parse(s))
            .filter(|m| m.refresh > 0.0)
            .collect();
        modes.sort_by(|a, b| {
            let area = (b.width as u64 * b.height as u64).cmp(&(a.width as u64 * a.height as u64));
            area.then((b.width).cmp(&a.width))
                .then(b.refresh.total_cmp(&a.refresh))
        });
        modes.dedup_by(|a, b| a.matches(b));
        modes
    }

    /// Distinct resolutions, largest first.
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = Vec::new();
        for m in self.modes() {
            if !out.contains(&(m.width, m.height)) {
                out.push((m.width, m.height));
            }
        }
        out
    }

    /// Refresh rates available at one resolution, fastest first.
    pub fn refresh_rates(&self, width: u32, height: u32) -> Vec<f64> {
        self.modes()
            .into_iter()
            .filter(|m| m.width == width && m.height == height)
            .map(|m| m.refresh)
            .collect()
    }

    /// Turn a request into a mode this panel actually has.
    ///
    /// Returns `Ok(None)` for `preferred`. The error is user-facing text that
    /// names what the display can do instead — asking a 100Hz panel for 200Hz
    /// should say so, not write a line that silently falls back.
    pub fn resolve_mode(&self, req: ModeRequest) -> std::result::Result<Option<Mode>, String> {
        let modes = self.modes();
        if modes.is_empty() {
            return Err(format!(
                "{} reports no modes — this Hyprland may be too old to list them; \
                 set the mode by hand in monitors.conf",
                self.name
            ));
        }
        match req {
            ModeRequest::Preferred => Ok(None),
            ModeRequest::Resolution(w, h) => modes
                .iter()
                .find(|m| m.width == w && m.height == h)
                .copied()
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "{} can't do {w}x{h}. It supports: {}",
                        self.name,
                        list_resolutions(&modes)
                    )
                }),
            ModeRequest::Exact(want) => modes
                .iter()
                .find(|m| m.matches(&want))
                .copied()
                .map(Some)
                .ok_or_else(|| {
                    let at_res: Vec<&Mode> = modes
                        .iter()
                        .filter(|m| m.width == want.width && m.height == want.height)
                        .collect();
                    if at_res.is_empty() {
                        format!(
                            "{} can't do {}x{}. It supports: {}",
                            self.name,
                            want.width,
                            want.height,
                            list_resolutions(&modes)
                        )
                    } else {
                        format!(
                            "{} can't do {}x{} at {}Hz. At that resolution it supports: {}",
                            self.name,
                            want.width,
                            want.height,
                            fmt_scale(want.refresh),
                            at_res
                                .iter()
                                .map(|m| format!("{}Hz", fmt_scale(m.refresh)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                }),
            ModeRequest::Refresh(hz) => {
                let (w, h) = (self.width, self.height);
                let want = Mode {
                    width: w,
                    height: h,
                    refresh: hz,
                };
                modes
                    .iter()
                    .find(|m| m.matches(&want))
                    .copied()
                    .map(Some)
                    .ok_or_else(|| {
                        format!(
                            "{} can't do {}Hz at its current {w}x{h}. Available there: {}",
                            self.name,
                            fmt_scale(hz),
                            self.refresh_rates(w, h)
                                .iter()
                                .map(|r| format!("{}Hz", fmt_scale(*r)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
            }
        }
    }
}

/// `3440x1440, 1920x1080, …` for an error message.
fn list_resolutions(modes: &[Mode]) -> String {
    let mut seen: Vec<(u32, u32)> = Vec::new();
    for m in modes {
        if !seen.contains(&(m.width, m.height)) {
            seen.push((m.width, m.height));
        }
    }
    seen.iter()
        .map(|(w, h)| format!("{w}x{h}"))
        .collect::<Vec<_>>()
        .join(", ")
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

    /// Point one display at a mode (`None` = `preferred`). False when no
    /// display in the layout goes by that name.
    pub fn set_mode(&mut self, name: &str, mode: Option<Mode>) -> bool {
        let mut hit = false;
        for s in &mut self.monitors {
            if s.name == name {
                s.mode = mode
                    .map(|m| m.to_spec())
                    .unwrap_or_else(|| "preferred".into());
                hit = true;
            }
        }
        hit
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

    /// An Acer ED340CUR as Hyprland reports it: a 100Hz ultrawide.
    const ULTRAWIDE: &str = r#"[
      {"id":0,"name":"HDMI-A-1","description":"Acer ED340CUR","make":"Acer","model":"ED340CUR",
       "width":3440,"height":1440,"refreshRate":59.999,"x":0,"y":0,"scale":1.0,"transform":0,
       "focused":true,"disabled":false,"dpmsStatus":true,
       "availableModes":["3440x1440@60.00Hz","3840x2160@60.00Hz","3440x1440@100.00Hz",
                         "1920x1080@120.00Hz","1920x1080@60.00Hz","1920x1080@60.00Hz"]}
    ]"#;

    fn acer() -> Monitor {
        parse(ULTRAWIDE).unwrap().remove(0)
    }

    #[test]
    fn mode_parses_every_shape() {
        assert_eq!(
            Mode::parse("3440x1440@100.00Hz"),
            Some(Mode {
                width: 3440,
                height: 1440,
                refresh: 100.0
            })
        );
        assert_eq!(Mode::parse("3440x1440@100").unwrap().refresh, 100.0);
        assert_eq!(Mode::parse("1920x1080").unwrap().refresh, 0.0);
        assert_eq!(Mode::parse("garbage"), None);
        assert_eq!(Mode::parse("100"), None);
    }

    #[test]
    fn modes_are_sorted_and_deduped() {
        let m = acer();
        let modes = m.modes();
        // 3840x2160 is the largest area, then the ultrawide's two rates.
        assert_eq!(modes[0].label(), "3840x2160 @ 60Hz");
        assert_eq!(modes[1].label(), "3440x1440 @ 100Hz");
        assert_eq!(modes[2].label(), "3440x1440 @ 60Hz");
        // The duplicate 1920x1080@60 collapsed.
        assert_eq!(modes.len(), 5);
        assert_eq!(
            m.resolutions(),
            vec![(3840, 2160), (3440, 1440), (1920, 1080)]
        );
        assert_eq!(m.refresh_rates(3440, 1440), vec![100.0, 60.0]);
    }

    #[test]
    fn mode_request_parses_user_specs() {
        assert_eq!(
            ModeRequest::parse("preferred"),
            Some(ModeRequest::Preferred)
        );
        assert_eq!(
            ModeRequest::parse("3440x1440"),
            Some(ModeRequest::Resolution(3440, 1440))
        );
        assert!(matches!(
            ModeRequest::parse("3440x1440@100"),
            Some(ModeRequest::Exact(_))
        ));
        assert_eq!(ModeRequest::parse("100"), Some(ModeRequest::Refresh(100.0)));
        assert_eq!(
            ModeRequest::parse("100Hz"),
            Some(ModeRequest::Refresh(100.0))
        );
        assert_eq!(ModeRequest::parse("nonsense"), None);
    }

    #[test]
    fn resolve_picks_fastest_rate_for_a_resolution() {
        let m = acer();
        let got = m
            .resolve_mode(ModeRequest::Resolution(3440, 1440))
            .unwrap()
            .unwrap();
        assert_eq!(got.refresh, 100.0);
        assert_eq!(got.to_spec(), "3440x1440@100.00");
    }

    #[test]
    fn resolve_preferred_yields_none() {
        assert_eq!(acer().resolve_mode(ModeRequest::Preferred).unwrap(), None);
    }

    #[test]
    fn resolve_tolerates_rounding_between_hyprctl_and_the_kernel() {
        // Live refreshRate is 59.999; asking for 60 must still match.
        let got = acer().resolve_mode(ModeRequest::Refresh(60.0)).unwrap();
        assert_eq!(got.unwrap().refresh, 60.0);
    }

    #[test]
    fn resolve_rejects_a_rate_the_panel_lacks() {
        // The whole point: a 100Hz panel asked for 200Hz says so.
        let err = acer()
            .resolve_mode(ModeRequest::Refresh(200.0))
            .unwrap_err();
        assert!(err.contains("can't do 200Hz"), "{err}");
        assert!(err.contains("100Hz"), "{err}");
        assert!(err.contains("60Hz"), "{err}");
    }

    #[test]
    fn resolve_rejects_an_unknown_resolution() {
        let err = acer()
            .resolve_mode(ModeRequest::Resolution(5120, 1440))
            .unwrap_err();
        assert!(err.contains("can't do 5120x1440"), "{err}");
        assert!(err.contains("3440x1440"), "{err}");
    }

    #[test]
    fn resolve_rejects_a_rate_wrong_for_that_resolution() {
        let err = acer()
            .resolve_mode(ModeRequest::Exact(Mode {
                width: 3440,
                height: 1440,
                refresh: 120.0,
            }))
            .unwrap_err();
        assert!(err.contains("at 120Hz"), "{err}");
        assert!(err.contains("100Hz, 60Hz"), "{err}");
    }

    #[test]
    fn resolve_explains_a_hyprland_that_lists_no_modes() {
        let mut m = acer();
        m.available_modes.clear();
        let err = m.resolve_mode(ModeRequest::Refresh(100.0)).unwrap_err();
        assert!(err.contains("reports no modes"), "{err}");
    }

    #[test]
    fn set_mode_rewrites_only_the_named_display() {
        let mons = parse(TWO).unwrap();
        let mut layout = Layout::from_monitors(&mons);
        assert!(layout.set_mode(
            "HDMI-A-1",
            Some(Mode {
                width: 1920,
                height: 1080,
                refresh: 100.0
            })
        ));
        assert_eq!(layout.monitors[0].mode, "1920x1080@120.21"); // untouched
        assert_eq!(layout.monitors[1].mode, "1920x1080@100.00");
        assert!(layout.set_mode("HDMI-A-1", None));
        assert_eq!(
            layout.monitors[1].line(),
            "monitor = HDMI-A-1, preferred, 1920x0, 1, transform, 0"
        );
        assert!(!layout.set_mode("DP-99", None));
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

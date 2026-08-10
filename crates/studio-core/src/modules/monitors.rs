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
//! The layout math (effective size under scale and rotation, placing a display
//! flush against a neighbour, re-origining the result) is pure and
//! unit-tested; the frontend drives it. `Layout::check` reports overlapping
//! and unreachable displays, but only ever as a warning — Hyprland loads both
//! quite happily, and a deliberate overlap is somebody's mirror setup.

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

impl Side {
    /// Read `left-of`/`left`, `right-of`/`right`, `above`/`up`, `below`/`down`.
    pub fn parse(s: &str) -> Option<Side> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "left-of" | "left" | "l" => Some(Side::LeftOf),
            "right-of" | "right" | "r" => Some(Side::RightOf),
            "above" | "up" | "top" | "u" => Some(Side::Above),
            "below" | "down" | "under" | "d" => Some(Side::Below),
            _ => None,
        }
    }

    /// How it reads back to the user.
    pub fn label(&self) -> &'static str {
        match self {
            Side::LeftOf => "left of",
            Side::RightOf => "right of",
            Side::Above => "above",
            Side::Below => "below",
        }
    }

    /// True for sides that stack horizontally, so the cross axis is vertical.
    pub fn is_horizontal(&self) -> bool {
        matches!(self, Side::LeftOf | Side::RightOf)
    }
}

/// Where a display sits on the axis it is *not* being stacked along: placing a
/// short panel right of a tall one still has to decide top, middle, or bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Top edges flush (or left edges, stacking vertically).
    #[default]
    Start,
    /// Centres line up — what a laptop beside an ultrawide usually wants.
    Center,
    /// Bottom edges flush (or right edges, stacking vertically).
    End,
}

impl Align {
    pub fn parse(s: &str) -> Option<Align> {
        match s.trim().to_ascii_lowercase().as_str() {
            "start" | "top" | "left" => Some(Align::Start),
            "center" | "centre" | "middle" => Some(Align::Center),
            "end" | "bottom" | "right" => Some(Align::End),
            _ => None,
        }
    }

    /// Names the edge it lines up, which differs per axis.
    pub fn label(&self, horizontal: bool) -> &'static str {
        match (self, horizontal) {
            (Align::Start, true) => "top",
            (Align::Center, _) => "center",
            (Align::End, true) => "bottom",
            (Align::Start, false) => "left",
            (Align::End, false) => "right",
        }
    }

    pub fn next(&self) -> Align {
        match self {
            Align::Start => Align::Center,
            Align::Center => Align::End,
            Align::End => Align::Start,
        }
    }

    /// Offset for a `len`-long span inside a `base`-long one.
    fn offset(&self, base: u32, len: u32) -> i32 {
        match self {
            Align::Start => 0,
            Align::Center => (base as i32 - len as i32) / 2,
            Align::End => base as i32 - len as i32,
        }
    }
}

/// A display's footprint in Hyprland's logical coordinate space (post-scale,
/// post-rotation), which is the space `monitor=` positions live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    /// Shared interior area — touching edges don't count as overlapping.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// A shared border of non-zero length: the cursor can cross here. Corner
    /// contact is not enough, so the overlap on the shared edge must be > 0.
    pub fn touches(&self, other: &Rect) -> bool {
        let x_overlap = self.x.max(other.x) < self.right().min(other.right());
        let y_overlap = self.y.max(other.y) < self.bottom().min(other.bottom());
        (self.right() == other.x || other.right() == self.x) && y_overlap
            || (self.bottom() == other.y || other.bottom() == self.y) && x_overlap
    }
}

/// Position `size` (a display's effective w×h) relative to an already-placed
/// `base` rect (x, y, w, h), touching on the given side, flush at the start of
/// the cross axis. Kept for callers that don't care about alignment.
pub fn place(base: (i32, i32, u32, u32), size: (u32, u32), side: Side) -> (i32, i32) {
    let (bx, by, bw, bh) = base;
    place_aligned(Rect::new(bx, by, bw, bh), size, side, Align::Start)
}

/// Position `size` against `base` on `side`, lining the two up on the cross
/// axis according to `align`.
pub fn place_aligned(base: Rect, size: (u32, u32), side: Side, align: Align) -> (i32, i32) {
    let (w, h) = size;
    match side {
        Side::RightOf => (base.right(), base.y + align.offset(base.h, h)),
        Side::LeftOf => (base.x - w as i32, base.y + align.offset(base.h, h)),
        Side::Below => (base.x + align.offset(base.w, w), base.bottom()),
        Side::Above => (base.x + align.offset(base.w, w), base.y - h as i32),
    }
}

/// What's wrong with an arrangement. Neither stops Hyprland from loading, so
/// these are warnings the frontend shows — not errors that block a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutIssue {
    /// Two displays claim the same logical pixels; Hyprland will render the
    /// overlap to both and the cursor behaves oddly across it.
    Overlap { a: String, b: String },
    /// An island with no shared edge — the pointer can't reach it, because
    /// there is no border to cross.
    Detached { name: String },
}

impl LayoutIssue {
    pub fn message(&self) -> String {
        match self {
            LayoutIssue::Overlap { a, b } => {
                format!("{a} and {b} overlap — they'll fight over the same pixels")
            }
            LayoutIssue::Detached { name } => {
                format!("{name} has a gap around it — the cursor can't reach that screen")
            }
        }
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

    /// The scale this setting asks for, falling back to 1 on an unparseable
    /// string (the field is rendered by `fmt_scale`, so that shouldn't happen).
    pub fn scale_value(&self) -> f64 {
        self.scale
            .parse::<f64>()
            .ok()
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0)
    }

    /// Native size this setting selects. `None` when it says `preferred` —
    /// only Hyprland knows what that resolves to.
    pub fn mode_size(&self) -> Option<(u32, u32)> {
        Mode::parse(&self.mode).map(|m| (m.width, m.height))
    }

    /// Logical size after scale and rotation. Falls back to the live display
    /// when the mode is `preferred`; `None` if neither can say.
    pub fn effective_size(&self, live: Option<&Monitor>) -> Option<(u32, u32)> {
        let (w, h) = self
            .mode_size()
            .or_else(|| live.map(|m| (m.width, m.height)))?;
        Some(effective_size(w, h, self.scale_value(), self.transform))
    }

    /// This display's footprint, or `None` for a disabled display or one whose
    /// size can't be determined.
    pub fn rect(&self, live: Option<&Monitor>) -> Option<Rect> {
        if self.disabled {
            return None;
        }
        let (w, h) = self.effective_size(live)?;
        Some(Rect::new(self.x, self.y, w, h))
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

    // -------------------------------------------------------- arrangement

    /// Every enabled display's footprint, paired with its name, in layout
    /// order. Displays that are off (or whose size is unknowable) drop out.
    pub fn rects(&self, live: &[Monitor]) -> Vec<(String, Rect)> {
        self.monitors
            .iter()
            .filter_map(|s| {
                let l = live.iter().find(|m| m.name == s.name);
                s.rect(l).map(|r| (s.name.clone(), r))
            })
            .collect()
    }

    /// Move one display to an absolute logical position. False when no display
    /// in the layout goes by that name.
    pub fn set_position(&mut self, name: &str, x: i32, y: i32) -> bool {
        let mut hit = false;
        for s in &mut self.monitors {
            if s.name == name {
                (s.x, s.y) = (x, y);
                hit = true;
            }
        }
        hit
    }

    /// Put `name` flush against `anchor` on `side`, lined up per `align`, then
    /// re-origin the whole arrangement so nothing sits at negative coordinates.
    ///
    /// The error is user-facing: a name that isn't a display, an anchor that is
    /// the display itself, or a display that's currently disabled (moving an
    /// off screen would write a position Hyprland ignores).
    pub fn place_relative(
        &mut self,
        name: &str,
        anchor: &str,
        side: Side,
        align: Align,
        live: &[Monitor],
    ) -> std::result::Result<(), String> {
        if name == anchor {
            return Err(format!("{name} can't be placed relative to itself"));
        }
        let size = self.require_size(name, live)?;
        let base = self.require_rect(anchor, live)?;
        let (x, y) = place_aligned(base, size, side, align);
        self.set_position(name, x, y);
        self.normalize(live);
        Ok(())
    }

    /// Lay the enabled displays out edge to edge along one axis.
    ///
    /// `order` names the displays left-to-right (or top-to-bottom); any enabled
    /// display it omits is appended in layout order, so a partial order like
    /// `["HDMI-A-1"]` still produces a complete arrangement.
    pub fn arrange(
        &mut self,
        side: Side,
        order: &[String],
        align: Align,
        live: &[Monitor],
    ) -> std::result::Result<(), String> {
        let mut seq: Vec<String> = Vec::new();
        for want in order {
            let known = self.monitors.iter().any(|s| &s.name == want);
            if !known {
                return Err(format!("no display named `{want}` in this layout"));
            }
            if !seq.contains(want) {
                seq.push(want.clone());
            }
        }
        for (name, _) in self.rects(live) {
            if !seq.contains(&name) {
                seq.push(name);
            }
        }
        // Drop anything disabled — an off display has no footprint to chain.
        seq.retain(|n| self.rect_of(n, live).is_some());
        let Some((first, rest)) = seq.split_first() else {
            return Err("no enabled displays to arrange".into());
        };
        let size = self.require_size(first, live)?;
        self.set_position(first, 0, 0);
        let mut prev = Rect::new(0, 0, size.0, size.1);
        for name in rest {
            let size = self.require_size(name, live)?;
            let (x, y) = place_aligned(prev, size, side, align);
            self.set_position(name, x, y);
            prev = Rect::new(x, y, size.0, size.1);
        }
        self.normalize(live);
        Ok(())
    }

    /// Shift every display by the same offset so the top-left-most edge sits at
    /// the origin. Hyprland accepts negative coordinates, but keeping the
    /// arrangement in positive space makes `monitors.conf` readable and keeps
    /// relative moves from drifting away from 0,0 over successive edits.
    pub fn normalize(&mut self, live: &[Monitor]) {
        let rects = self.rects(live);
        let (Some(min_x), Some(min_y)) = (
            rects.iter().map(|(_, r)| r.x).min(),
            rects.iter().map(|(_, r)| r.y).min(),
        ) else {
            return;
        };
        if (min_x, min_y) == (0, 0) {
            return;
        }
        let names: Vec<String> = rects.into_iter().map(|(n, _)| n).collect();
        for s in &mut self.monitors {
            if names.contains(&s.name) {
                s.x -= min_x;
                s.y -= min_y;
            }
        }
    }

    /// Overlaps and unreachable islands in the current arrangement. Empty means
    /// every enabled display tiles cleanly and the cursor can reach all of them.
    pub fn check(&self, live: &[Monitor]) -> Vec<LayoutIssue> {
        let rects = self.rects(live);
        let mut issues = Vec::new();
        for (i, (a, ra)) in rects.iter().enumerate() {
            for (b, rb) in rects.iter().skip(i + 1) {
                if ra.overlaps(rb) {
                    issues.push(LayoutIssue::Overlap {
                        a: a.clone(),
                        b: b.clone(),
                    });
                }
            }
        }
        // Flood-fill the "shares an edge with" graph from the first display;
        // anything unvisited is an island the pointer can't cross into.
        // Overlapping counts as connected — that's a different complaint,
        // already reported above, and it is emphatically not a gap.
        if rects.len() > 1 {
            // Start from the biggest display: with two disconnected screens
            // either could be called the island, and the small one floating
            // away from the main desktop is the answer a user expects.
            let anchor = rects
                .iter()
                .enumerate()
                .max_by_key(|(i, (_, r))| (r.w as u64 * r.h as u64, std::cmp::Reverse(*i)))
                .map(|(i, _)| i)
                .unwrap_or(0);
            let mut seen = vec![false; rects.len()];
            let mut stack = vec![anchor];
            seen[anchor] = true;
            while let Some(i) = stack.pop() {
                for (j, (_, rj)) in rects.iter().enumerate() {
                    if !seen[j] && (rects[i].1.touches(rj) || rects[i].1.overlaps(rj)) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            for (i, reached) in seen.iter().enumerate() {
                if !reached {
                    issues.push(LayoutIssue::Detached {
                        name: rects[i].0.clone(),
                    });
                }
            }
        }
        issues
    }

    fn rect_of(&self, name: &str, live: &[Monitor]) -> Option<Rect> {
        let s = self.monitors.iter().find(|s| s.name == name)?;
        s.rect(live.iter().find(|m| m.name == name))
    }

    /// A named display's footprint, or a user-facing reason there isn't one.
    fn require_rect(&self, name: &str, live: &[Monitor]) -> std::result::Result<Rect, String> {
        if !self.monitors.iter().any(|s| s.name == name) {
            return Err(format!("no display named `{name}` — see `monitor list`"));
        }
        self.rect_of(name, live)
            .ok_or_else(|| format!("{name} is disabled — enable it before arranging around it"))
    }

    fn require_size(
        &self,
        name: &str,
        live: &[Monitor],
    ) -> std::result::Result<(u32, u32), String> {
        self.require_rect(name, live).map(|r| (r.w, r.h))
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

    // A 3440x1440 ultrawide plus a 1920x1080 laptop panel at scale 1.5, which
    // is 1280x720 effective — the mismatch that makes alignment matter.
    const DESK: &str = r#"[
      {"id":0,"name":"eDP-1","description":"Lenovo 0x9059","make":"Lenovo","model":"0x9059",
       "width":1920,"height":1080,"refreshRate":120.213,"x":3440,"y":0,"scale":1.5,"transform":0,
       "focused":false,"disabled":false,"dpmsStatus":true},
      {"id":1,"name":"HDMI-A-1","description":"Acer ED340CUR","make":"Acer","model":"ED340CUR",
       "width":3440,"height":1440,"refreshRate":100.0,"x":0,"y":0,"scale":1.0,"transform":0,
       "focused":true,"disabled":false,"dpmsStatus":true}
    ]"#;

    fn desk() -> (Vec<Monitor>, Layout) {
        let live = parse(DESK).unwrap();
        let layout = Layout::from_monitors(&live);
        (live, layout)
    }

    #[test]
    fn setting_reports_its_effective_footprint() {
        let (live, layout) = desk();
        let laptop = &layout.monitors[0];
        // 1920x1080 at 1.5 is 1280x720 logical.
        assert_eq!(laptop.effective_size(Some(&live[0])), Some((1280, 720)));
        assert_eq!(
            laptop.rect(Some(&live[0])),
            Some(Rect::new(3440, 0, 1280, 720))
        );
    }

    #[test]
    fn preferred_falls_back_to_the_live_size() {
        let (live, mut layout) = desk();
        layout.set_mode("eDP-1", None);
        assert_eq!(layout.monitors[0].mode, "preferred");
        assert_eq!(
            layout.monitors[0].effective_size(Some(&live[0])),
            Some((1280, 720))
        );
        // With nothing live to ask, `preferred` has no knowable size.
        assert_eq!(layout.monitors[0].effective_size(None), None);
    }

    #[test]
    fn disabled_displays_have_no_footprint() {
        let (live, mut layout) = desk();
        layout.monitors[0].disabled = true;
        assert_eq!(layout.monitors[0].rect(Some(&live[0])), None);
        assert_eq!(layout.rects(&live).len(), 1);
    }

    #[test]
    fn place_aligned_lines_up_the_cross_axis() {
        let base = Rect::new(0, 0, 3440, 1440);
        let small = (1280, 720);
        assert_eq!(
            place_aligned(base, small, Side::RightOf, Align::Start),
            (3440, 0)
        );
        assert_eq!(
            place_aligned(base, small, Side::RightOf, Align::Center),
            (3440, 360)
        );
        assert_eq!(
            place_aligned(base, small, Side::RightOf, Align::End),
            (3440, 720)
        );
        assert_eq!(
            place_aligned(base, small, Side::LeftOf, Align::End),
            (-1280, 720)
        );
        assert_eq!(
            place_aligned(base, small, Side::Below, Align::Center),
            (1080, 1440)
        );
        assert_eq!(
            place_aligned(base, small, Side::Above, Align::Center),
            (1080, -720)
        );
        // The alignment-free helper stays flush at the start.
        assert_eq!(place((0, 0, 3440, 1440), small, Side::Below), (0, 1440));
    }

    #[test]
    fn rects_distinguish_touching_from_overlapping() {
        let a = Rect::new(0, 0, 100, 100);
        assert!(a.touches(&Rect::new(100, 0, 50, 50)));
        assert!(!a.overlaps(&Rect::new(100, 0, 50, 50)));
        assert!(a.overlaps(&Rect::new(99, 0, 50, 50)));
        // Corner-to-corner shares a point, not an edge: not reachable.
        assert!(!a.touches(&Rect::new(100, 100, 50, 50)));
        // A gap is neither.
        assert!(!a.touches(&Rect::new(120, 0, 50, 50)));
    }

    #[test]
    fn place_relative_puts_the_laptop_under_the_ultrawide() {
        let (live, mut layout) = desk();
        layout
            .place_relative("eDP-1", "HDMI-A-1", Side::Below, Align::Center, &live)
            .unwrap();
        // Centred under a 3440-wide screen: (3440-1280)/2 = 1080.
        assert_eq!(layout.monitors[0].x, 1080);
        assert_eq!(layout.monitors[0].y, 1440);
        assert_eq!((layout.monitors[1].x, layout.monitors[1].y), (0, 0));
        assert!(layout.check(&live).is_empty());
    }

    #[test]
    fn place_relative_left_renormalizes_to_the_origin() {
        let (live, mut layout) = desk();
        layout
            .place_relative("eDP-1", "HDMI-A-1", Side::LeftOf, Align::End, &live)
            .unwrap();
        // Laptop lands at -1280; normalize slides the pair back to 0.
        assert_eq!((layout.monitors[0].x, layout.monitors[0].y), (0, 720));
        assert_eq!((layout.monitors[1].x, layout.monitors[1].y), (1280, 0));
        assert!(layout.check(&live).is_empty());
    }

    #[test]
    fn place_relative_rejects_bad_targets() {
        let (live, mut layout) = desk();
        let err = layout
            .place_relative("eDP-1", "eDP-1", Side::LeftOf, Align::Start, &live)
            .unwrap_err();
        assert!(err.contains("relative to itself"), "{err}");

        let err = layout
            .place_relative("DP-9", "eDP-1", Side::LeftOf, Align::Start, &live)
            .unwrap_err();
        assert!(err.contains("no display named"), "{err}");

        layout.monitors[1].disabled = true;
        let err = layout
            .place_relative("eDP-1", "HDMI-A-1", Side::LeftOf, Align::Start, &live)
            .unwrap_err();
        assert!(err.contains("is disabled"), "{err}");
    }

    #[test]
    fn arrange_chains_displays_in_the_given_order() {
        let (live, mut layout) = desk();
        layout
            .arrange(
                Side::RightOf,
                &["HDMI-A-1".to_string()],
                Align::Center,
                &live,
            )
            .unwrap();
        // Named first, so the ultrawide anchors at the origin and the laptop
        // (unnamed, appended) centres against its right edge.
        assert_eq!((layout.monitors[1].x, layout.monitors[1].y), (0, 0));
        assert_eq!((layout.monitors[0].x, layout.monitors[0].y), (3440, 360));
        assert!(layout.check(&live).is_empty());
    }

    #[test]
    fn arrange_stacks_vertically_too() {
        let (live, mut layout) = desk();
        layout
            .arrange(Side::Below, &["eDP-1".to_string()], Align::Start, &live)
            .unwrap();
        assert_eq!((layout.monitors[0].x, layout.monitors[0].y), (0, 0));
        assert_eq!((layout.monitors[1].x, layout.monitors[1].y), (0, 720));
        assert!(layout.check(&live).is_empty());
    }

    #[test]
    fn arrange_skips_disabled_displays() {
        let (live, mut layout) = desk();
        layout.monitors[0].disabled = true;
        layout
            .arrange(Side::RightOf, &[], Align::Start, &live)
            .unwrap();
        assert_eq!((layout.monitors[1].x, layout.monitors[1].y), (0, 0));
        // A layout with nothing lit has nothing to arrange.
        layout.monitors[1].disabled = true;
        let err = layout
            .arrange(Side::RightOf, &[], Align::Start, &live)
            .unwrap_err();
        assert!(err.contains("no enabled displays"), "{err}");
    }

    #[test]
    fn arrange_rejects_an_unknown_name_in_the_order() {
        let (live, mut layout) = desk();
        let err = layout
            .arrange(Side::RightOf, &["DP-9".to_string()], Align::Start, &live)
            .unwrap_err();
        assert!(err.contains("no display named `DP-9`"), "{err}");
    }

    #[test]
    fn check_catches_overlap_and_islands() {
        let (live, mut layout) = desk();
        // Drag the laptop on top of the ultrawide.
        layout.set_position("eDP-1", 100, 100);
        let issues = layout.check(&live);
        assert_eq!(
            issues,
            vec![LayoutIssue::Overlap {
                a: "eDP-1".into(),
                b: "HDMI-A-1".into()
            }]
        );
        assert!(issues[0].message().contains("overlap"));

        // Float it clear of everything: reachable by neither edge.
        layout.set_position("eDP-1", 5000, 5000);
        let issues = layout.check(&live);
        assert_eq!(
            issues,
            vec![LayoutIssue::Detached {
                name: "eDP-1".into()
            }]
        );
        assert!(issues[0].message().contains("cursor can't reach"));
    }

    #[test]
    fn check_is_quiet_on_a_single_display() {
        let live = parse(ULTRAWIDE).unwrap();
        let layout = Layout::from_monitors(&live);
        assert!(layout.check(&live).is_empty());
    }

    #[test]
    fn normalize_slides_negative_coords_back() {
        let (live, mut layout) = desk();
        layout.set_position("eDP-1", -500, -200);
        layout.normalize(&live);
        assert_eq!((layout.monitors[0].x, layout.monitors[0].y), (0, 0));
        assert_eq!((layout.monitors[1].x, layout.monitors[1].y), (500, 200));
        // Already at the origin: a second pass changes nothing.
        let before = layout.clone();
        layout.normalize(&live);
        assert_eq!(layout, before);
    }

    #[test]
    fn set_position_reports_unknown_names() {
        let (_, mut layout) = desk();
        assert!(layout.set_position("eDP-1", 10, 20));
        assert_eq!((layout.monitors[0].x, layout.monitors[0].y), (10, 20));
        assert!(!layout.set_position("DP-9", 0, 0));
    }

    #[test]
    fn side_and_align_parse_what_users_type() {
        assert_eq!(Side::parse("left-of"), Some(Side::LeftOf));
        assert_eq!(Side::parse("RIGHT"), Some(Side::RightOf));
        assert_eq!(Side::parse("above"), Some(Side::Above));
        assert_eq!(Side::parse("down"), Some(Side::Below));
        assert_eq!(Side::parse("sideways"), None);
        assert_eq!(Align::parse("centre"), Some(Align::Center));
        assert_eq!(Align::parse("bottom"), Some(Align::End));
        assert_eq!(Align::parse("nope"), None);
        assert_eq!(Align::Start.label(true), "top");
        assert_eq!(Align::Start.label(false), "left");
        assert_eq!(Align::Start.next().next(), Align::End);
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

//! Monitors screen (roadmap 0.8.4, arrangement in 0.8.7).
//!
//! Lists the displays Hyprland reports, lets you nudge scale, flip a monitor
//! on/off, arrange them relative to one another, and identify which physical
//! panel is which. A proportional map above the list shows where each screen
//! actually sits. Save persists the layout to `monitors.conf` (managed block +
//! hotplug fallback) through the snapshot pipeline, so a bad layout is one
//! `snapshot undo` away.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout as LLayout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use studio_core::cmd::RealRunner;
use studio_core::modules::monitors::{self as mon, Align, Layout, Monitor, Side};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

pub enum MonitorsAction {
    None,
    /// Flash each monitor's name on-screen.
    Identify,
    /// Persist the edited layout (App snapshots + reloads).
    Save(Layout),
}

/// An in-progress arrangement: the display under the cursor is being anchored
/// against another one, with the result previewed live on the map. `before` is
/// the layout as it stood when the mode was entered — every side/align change
/// re-places from there, so cycling the options can't accumulate drift, and
/// Esc restores it exactly.
struct Placing {
    subject: String,
    anchor: usize,
    side: Side,
    align: Align,
    before: Layout,
}

pub struct MonitorsScreen {
    live: Vec<Monitor>,
    layout: Layout,
    cursor: usize,
    dirty: bool,
    /// Fatal: we couldn't read the displays at all, so the screen has nothing
    /// to show. Rendered in place of the list.
    error: Option<String>,
    /// Transient: the last edit that couldn't go anywhere ("only one rate").
    /// Rendered in the footer, cleared by the next edit that works.
    notice: Option<String>,
    /// `Some` while the arrangement is being edited interactively.
    placing: Option<Placing>,
}

impl MonitorsScreen {
    pub fn load(_paths: &OmarchyPaths) -> Self {
        let (live, error) = match mon::load(&RealRunner) {
            Ok(m) => (m, None),
            Err(e) => (Vec::new(), Some(friendly(&e))),
        };
        let layout = Layout::from_monitors(&live);
        Self {
            live,
            layout,
            cursor: 0,
            dirty: false,
            error,
            notice: None,
            placing: None,
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths) {
        let keep = self.cursor;
        *self = Self::load(paths);
        self.cursor = crate::tui::ui::clamp_index(keep, self.layout.monitors.len());
    }

    pub fn hint(&self) -> &'static str {
        if self.placing.is_some() {
            "←→↑↓ side · Tab anchor · a align · ⏎ keep · Esc cancel"
        } else {
            "↑↓ move · p place · r rate · m resolution · +/- scale · d disable · i identify · s save"
        }
    }

    /// The live monitor behind the row under the cursor.
    fn live_at_cursor(&self) -> Option<&Monitor> {
        let name = &self.layout.monitors.get(self.cursor)?.name;
        self.live.iter().find(|m| &m.name == name)
    }

    /// The mode the row is currently set to, read back from its `WxH@Hz` string
    /// (falls back to what the display is actually running).
    fn mode_at_cursor(&self) -> Option<mon::Mode> {
        let s = self.layout.monitors.get(self.cursor)?;
        let live = self.live_at_cursor()?;
        mon::Mode::parse(&s.mode)
            .filter(|m| m.refresh > 0.0)
            .or(Some(mon::Mode {
                width: live.width,
                height: live.height,
                refresh: live.refresh_rate,
            }))
    }

    /// Step to the next refresh rate available at the row's current resolution.
    fn cycle_rate(&mut self) {
        let (Some(live), Some(current)) = (self.live_at_cursor(), self.mode_at_cursor()) else {
            return;
        };
        let rates = live.refresh_rates(current.width, current.height);
        if rates.len() < 2 {
            self.notice = Some(format!(
                "{} has only one refresh rate at {}x{}",
                live.name, current.width, current.height
            ));
            return;
        }
        let at = rates
            .iter()
            .position(|r| (r - current.refresh).abs() < 0.1)
            .unwrap_or(0);
        let next = rates[(at + 1) % rates.len()];
        let name = live.name.clone();
        self.notice = None;
        self.layout.set_mode(
            &name,
            Some(mon::Mode {
                refresh: next,
                ..current
            }),
        );
        self.dirty = true;
    }

    /// Step to the next resolution, keeping the fastest rate it offers.
    fn cycle_resolution(&mut self) {
        let (Some(live), Some(current)) = (self.live_at_cursor(), self.mode_at_cursor()) else {
            return;
        };
        let res = live.resolutions();
        if res.len() < 2 {
            self.notice = Some(format!("{} reports only one resolution", live.name));
            return;
        }
        let at = res
            .iter()
            .position(|&(w, h)| (w, h) == (current.width, current.height))
            .unwrap_or(0);
        let (w, h) = res[(at + 1) % res.len()];
        let name = live.name.clone();
        // `resolutions()` came from the mode list, so a rate is guaranteed.
        let Some(&fastest) = live.refresh_rates(w, h).first() else {
            return;
        };
        self.notice = None;
        self.layout.set_mode(
            &name,
            Some(mon::Mode {
                width: w,
                height: h,
                refresh: fastest,
            }),
        );
        self.dirty = true;
    }

    fn nudge_scale(&mut self, delta: f64) {
        let Some(s) = self.layout.monitors.get_mut(self.cursor) else {
            return;
        };
        let current: f64 = s.scale.parse().unwrap_or(1.0);
        let next = (current + delta).clamp(0.5, 3.0);
        s.scale = mon::fmt_scale((next * 100.0).round() / 100.0);
        self.dirty = true;
    }

    // ----------------------------------------------------------- placement

    /// Indices into `layout.monitors` of every display that is currently on —
    /// the only ones that can anchor an arrangement or be moved by one.
    fn enabled(&self) -> Vec<usize> {
        (0..self.layout.monitors.len())
            .filter(|&i| !self.layout.monitors[i].disabled)
            .collect()
    }

    /// Enter placement mode for the display under the cursor.
    fn start_placing(&mut self) {
        let Some(subject) = self.layout.monitors.get(self.cursor) else {
            return;
        };
        if subject.disabled {
            self.notice = Some(format!(
                "{} is off — press d to enable it first",
                subject.name
            ));
            return;
        }
        let others: Vec<usize> = self
            .enabled()
            .into_iter()
            .filter(|&i| i != self.cursor)
            .collect();
        let Some(&anchor) = others.first() else {
            self.notice = Some("only one display is on — nothing to arrange it against".into());
            return;
        };
        self.notice = None;
        self.placing = Some(Placing {
            subject: subject.name.clone(),
            anchor,
            // Start from where the display already sits, so opening the mode
            // on an arrangement you like doesn't immediately disturb it.
            side: self
                .current_side(self.cursor, anchor)
                .unwrap_or(Side::RightOf),
            align: Align::Start,
            before: self.layout.clone(),
        });
        self.apply_placement();
    }

    /// Which side of `anchor` the display at `idx` currently sits on, judged by
    /// the larger of the two centre offsets. `None` if either has no footprint.
    fn current_side(&self, idx: usize, anchor: usize) -> Option<Side> {
        let rect = |i: usize| {
            let s = self.layout.monitors.get(i)?;
            s.rect(self.live.iter().find(|m| m.name == s.name))
        };
        let (a, b) = (rect(idx)?, rect(anchor)?);
        let dx = (a.x + a.w as i32 / 2) - (b.x + b.w as i32 / 2);
        let dy = (a.y + a.h as i32 / 2) - (b.y + b.h as i32 / 2);
        Some(if dx.abs() >= dy.abs() {
            if dx < 0 {
                Side::LeftOf
            } else {
                Side::RightOf
            }
        } else if dy < 0 {
            Side::Above
        } else {
            Side::Below
        })
    }

    /// Re-derive the layout from the pre-placement snapshot plus the current
    /// side/align choice. A rejected placement leaves the preview untouched and
    /// explains itself in the footer.
    fn apply_placement(&mut self) {
        let Some(p) = &self.placing else { return };
        let Some(anchor) = p.before.monitors.get(p.anchor).map(|s| s.name.clone()) else {
            return;
        };
        let mut next = p.before.clone();
        match next.place_relative(&p.subject, &anchor, p.side, p.align, &self.live) {
            Ok(()) => {
                self.layout = next;
                self.notice = None;
            }
            Err(msg) => self.notice = Some(msg),
        }
    }

    /// Step the anchor to the next enabled display that isn't the subject.
    fn cycle_anchor(&mut self) {
        let subject = self.placing.as_ref().map(|p| p.subject.clone());
        let others: Vec<usize> = self
            .enabled()
            .into_iter()
            .filter(|&i| Some(&self.layout.monitors[i].name) != subject.as_ref())
            .collect();
        let Some(p) = &mut self.placing else { return };
        if others.len() < 2 {
            return;
        }
        let at = others.iter().position(|&i| i == p.anchor).unwrap_or(0);
        p.anchor = others[(at + 1) % others.len()];
        self.apply_placement();
    }

    /// Placement-mode keys. Returns false for a key it doesn't own, so the
    /// caller can fall through to the normal bindings.
    fn handle_placing(&mut self, key: KeyEvent) -> bool {
        let Some(p) = &mut self.placing else {
            return false;
        };
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => p.side = Side::LeftOf,
            KeyCode::Right | KeyCode::Char('l') => p.side = Side::RightOf,
            KeyCode::Up | KeyCode::Char('k') => p.side = Side::Above,
            KeyCode::Down | KeyCode::Char('j') => p.side = Side::Below,
            KeyCode::Char('a') => p.align = p.align.next(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.cycle_anchor();
                return true;
            }
            KeyCode::Enter => {
                // Keep the preview; it differs from `before` only if a
                // placement actually landed.
                let changed = self.layout != p.before;
                self.placing = None;
                self.dirty |= changed;
                return true;
            }
            KeyCode::Esc => {
                self.layout = p.before.clone();
                self.placing = None;
                self.notice = None;
                return true;
            }
            _ => return false,
        }
        self.apply_placement();
        true
    }

    pub fn handle(&mut self, key: KeyEvent) -> MonitorsAction {
        if self.placing.is_some() {
            self.handle_placing(key);
            return MonitorsAction::None;
        }
        if crate::tui::ui::list_nav(key.code, &mut self.cursor, self.layout.monitors.len()) {
            return MonitorsAction::None;
        }
        match key.code {
            KeyCode::Char('p') => self.start_placing(),
            KeyCode::Char('r') => self.cycle_rate(),
            KeyCode::Char('m') => self.cycle_resolution(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_scale(0.25),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_scale(-0.25),
            KeyCode::Char('d') => {
                if let Some(s) = self.layout.monitors.get_mut(self.cursor) {
                    s.disabled = !s.disabled;
                    self.dirty = true;
                }
            }
            KeyCode::Char('i') => return MonitorsAction::Identify,
            KeyCode::Char('s') if self.dirty => return MonitorsAction::Save(self.layout.clone()),
            _ => {}
        }
        MonitorsAction::None
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        if let Some(msg) = &self.error {
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled("Can't read your monitors", skin.accent_bold())),
                    Line::from(""),
                    Line::from(Span::styled(msg.clone(), skin.body())),
                    Line::from(Span::styled("(are you in a Hyprland session?)", skin.dim())),
                ]),
                area,
            );
            return;
        }
        let rows = LLayout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(9),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(area);
        self.render_map(f, rows[0], skin);

        let mut items: Vec<ListItem> = Vec::new();
        for (i, s) in self.layout.monitors.iter().enumerate() {
            let on = i == self.cursor;
            let live = self.live.iter().find(|m| m.name == s.name);
            let laptop = live.map(Monitor::is_laptop).unwrap_or(false);
            let name_style = if on { skin.selection() } else { skin.body() };
            let marker = if on { "▸ " } else { "  " };
            let state = if s.disabled { " · OFF" } else { "" };
            let tag = if laptop { "  laptop" } else { "" };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!("{marker}{:<11}", s.name), name_style),
                Span::styled(
                    format!("{}  @ {}x{}  scale {}{state}", s.mode, s.x, s.y, s.scale),
                    if s.disabled { skin.dim() } else { skin.body() },
                ),
                Span::styled(tag, skin.dim()),
            ])));
            if let Some(m) = live {
                let (w, h) = mode_size(&s.mode, m);
                let (ew, eh) = effective_of(m, &s.scale, (w, h));
                if (ew, eh) != (w, h) {
                    items.push(ListItem::new(Line::from(Span::styled(
                        format!("             effective {ew}x{eh}"),
                        skin.dim(),
                    ))));
                }
                if on {
                    let rates = m.refresh_rates(w, h);
                    if rates.len() > 1 {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!(
                                "             r cycles: {}",
                                rates
                                    .iter()
                                    .map(|r| format!("{}Hz", mon::fmt_scale(*r)))
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            ),
                            skin.dim(),
                        ))));
                    }
                }
            }
        }
        f.render_widget(List::new(items), rows[1]);

        // Priority in the one status slot: what you just did wrong, then what
        // the arrangement is wrong about, then the unsaved reminder.
        let issues = self.layout.check(&self.live);
        let status = if let Some(msg) = &self.notice {
            Span::styled(format!("  ·  {msg}"), skin.warn())
        } else if let Some(issue) = issues.first() {
            Span::styled(format!("  ·  {}", issue.message()), skin.warn())
        } else if self.dirty {
            Span::styled("  ·  unsaved — s to apply & reload", skin.warn())
        } else {
            Span::styled("", skin.dim())
        };
        let title = match &self.placing {
            Some(p) => {
                let anchor = p
                    .before
                    .monitors
                    .get(p.anchor)
                    .map(|s| s.name.as_str())
                    .unwrap_or("?");
                Span::styled(
                    format!(
                        "Placing {} {} {} ({})",
                        p.subject,
                        p.side.label(),
                        anchor,
                        p.align.label(p.side.is_horizontal())
                    ),
                    skin.accent_bold(),
                )
            }
            None => Span::styled("Displays", skin.dim()),
        };
        let footer = Paragraph::new(vec![
            Line::from(vec![title, status]),
            Line::from(Span::styled(self.hint(), skin.dim())),
        ]);
        f.render_widget(footer, rows[2]);
    }

    /// Paint the arrangement as proportional boxes. Terminal cells are roughly
    /// twice as tall as they are wide, so a square patch of desktop has to map
    /// to twice as many columns as rows or every layout looks stretched.
    fn render_map(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let rects = self.layout.rects(&self.live);
        if rects.is_empty() || area.width < 8 || area.height < 3 {
            let msg = if rects.is_empty() {
                "every display is off"
            } else {
                "(map needs a taller window)"
            };
            f.render_widget(Paragraph::new(Span::styled(msg, skin.dim())), area);
            return;
        }
        let min_x = rects.iter().map(|(_, r)| r.x).min().unwrap_or(0);
        let min_y = rects.iter().map(|(_, r)| r.y).min().unwrap_or(0);
        let span_x =
            (rects.iter().map(|(_, r)| r.right()).max().unwrap_or(1) - min_x).max(1) as f64;
        let span_y =
            (rects.iter().map(|(_, r)| r.bottom()).max().unwrap_or(1) - min_y).max(1) as f64;
        // Leave a column/row of slack so a full-width layout still fits.
        let k = (((area.width - 1) as f64) / span_x).min(((area.height - 1) as f64) * 2.0 / span_y);

        let (cols, rows) = (area.width as usize, area.height as usize);
        let mut grid: Vec<Vec<(char, Option<usize>)>> = vec![vec![(' ', None); cols]; rows];
        for (i, (name, r)) in rects.iter().enumerate() {
            let to_col = |px: i32| (((px - min_x) as f64) * k).round() as usize;
            let to_row = |px: i32| (((px - min_y) as f64) * k / 2.0).round() as usize;
            let (x0, y0) = (to_col(r.x), to_row(r.y));
            if x0 >= cols || y0 >= rows {
                continue;
            }
            // Every box needs a border pair plus something between them.
            let x1 = to_col(r.right()).max(x0 + 3).min(cols);
            let y1 = to_row(r.bottom()).max(y0 + 3).min(rows);
            if x1 <= x0 + 1 || y1 <= y0 + 1 {
                continue;
            }
            draw_box(&mut grid, x0, y0, x1, y1, i);
            let (label, size) = (name.as_str(), format!("{}x{}", r.w, r.h));
            let mid = y0 + (y1 - y0) / 2;
            write_centered(&mut grid, mid, x0 + 1, x1 - 1, label, i);
            // Only annotate the size when it won't crowd out the name.
            if y1 - y0 >= 5 {
                write_centered(&mut grid, mid + 1, x0 + 1, x1 - 1, &size, i);
            }
        }

        let cursor_name = self
            .layout
            .monitors
            .get(self.cursor)
            .map(|s| s.name.as_str());
        let anchor_name = self
            .placing
            .as_ref()
            .and_then(|p| p.before.monitors.get(p.anchor).map(|s| s.name.as_str()));
        let style_for = |i: usize| -> Style {
            let name = rects[i].0.as_str();
            match &self.placing {
                Some(p) if p.subject == name => skin.selection(),
                Some(_) if Some(name) == anchor_name => skin.accent_bold(),
                Some(_) => skin.dim(),
                None if Some(name) == cursor_name => skin.selection(),
                None => skin.body(),
            }
        };

        // Collapse each row into runs that share an owner, so a box is one span.
        let lines: Vec<Line> = grid
            .into_iter()
            .map(|row| {
                let mut spans: Vec<Span> = Vec::new();
                let mut run = String::new();
                let mut owner: Option<usize> = None;
                for (ch, who) in row {
                    if who != owner && !run.is_empty() {
                        spans.push(styled_run(&run, owner, &style_for, skin));
                        run.clear();
                    }
                    owner = who;
                    run.push(ch);
                }
                if !run.is_empty() {
                    spans.push(styled_run(&run, owner, &style_for, skin));
                }
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn styled_run(
    text: &str,
    owner: Option<usize>,
    style_for: &dyn Fn(usize) -> Style,
    skin: &Skin,
) -> Span<'static> {
    let style = owner.map(style_for).unwrap_or_else(|| skin.dim());
    Span::styled(text.to_string(), style)
}

/// Box-draw the half-open rect `[x0,x1) × [y0,y1)` into the grid, tagging every
/// cell with the display that owns it.
fn draw_box(
    grid: &mut [Vec<(char, Option<usize>)>],
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    owner: usize,
) {
    for (y, row) in grid.iter_mut().enumerate().take(y1).skip(y0) {
        for (x, cell) in row.iter_mut().enumerate().take(x1).skip(x0) {
            let ch = match (y == y0, y == y1 - 1, x == x0, x == x1 - 1) {
                (true, _, true, _) => '┌',
                (true, _, _, true) => '┐',
                (_, true, true, _) => '└',
                (_, true, _, true) => '┘',
                (true, _, _, _) | (_, true, _, _) => '─',
                (_, _, true, _) | (_, _, _, true) => '│',
                _ => ' ',
            };
            *cell = (ch, Some(owner));
        }
    }
}

/// Write `text` centred in `[x0,x1)` on row `y`, truncated to fit.
fn write_centered(
    grid: &mut [Vec<(char, Option<usize>)>],
    y: usize,
    x0: usize,
    x1: usize,
    text: &str,
    owner: usize,
) {
    if y >= grid.len() || x1 <= x0 {
        return;
    }
    let room = x1 - x0;
    let text: String = text.chars().take(room).collect();
    let start = x0 + (room - text.chars().count()) / 2;
    for (i, ch) in text.chars().enumerate() {
        if let Some(cell) = grid[y].get_mut(start + i) {
            *cell = (ch, Some(owner));
        }
    }
}

/// The resolution a row is set to — its edited `WxH@Hz`, or the live one when
/// the row says `preferred`.
fn mode_size(mode: &str, m: &Monitor) -> (u32, u32) {
    mon::Mode::parse(mode)
        .map(|p| (p.width, p.height))
        .unwrap_or((m.width, m.height))
}

/// Effective size using the edited scale string (falls back to the live scale).
fn effective_of(m: &Monitor, scale_str: &str, (w, h): (u32, u32)) -> (u32, u32) {
    let scale = scale_str.parse().unwrap_or(m.scale);
    mon::effective_size(w, h, scale, m.transform)
}

fn friendly(e: &studio_core::StudioError) -> String {
    match e {
        studio_core::StudioError::External { detail, .. } => detail.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The ultrawide-plus-laptop desk: a 3440x1440 screen at the origin and a
    /// 1920x1080 panel at scale 1.5 (1280x720 effective) sitting to its right.
    const DESK: &str = r#"[
      {"id":0,"name":"eDP-1","description":"Lenovo","make":"Lenovo","model":"0x9059",
       "width":1920,"height":1080,"refreshRate":120.213,"x":3440,"y":0,"scale":1.5,"transform":0,
       "focused":false,"disabled":false,"dpmsStatus":true},
      {"id":1,"name":"HDMI-A-1","description":"Acer","make":"Acer","model":"ED340CUR",
       "width":3440,"height":1440,"refreshRate":100.0,"x":0,"y":0,"scale":1.0,"transform":0,
       "focused":true,"disabled":false,"dpmsStatus":true}
    ]"#;

    fn screen() -> MonitorsScreen {
        let live = mon::parse(DESK).unwrap();
        let layout = Layout::from_monitors(&live);
        MonitorsScreen {
            live,
            layout,
            cursor: 0,
            dirty: false,
            error: None,
            notice: None,
            placing: None,
        }
    }

    fn press(s: &mut MonitorsScreen, code: KeyCode) {
        s.handle(KeyEvent::from(code));
    }

    /// Render at a fixed size and return the screen as text rows.
    fn draw(s: &MonitorsScreen) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f, Rect::new(0, 0, 80, 24), &Skin::default()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn map_draws_both_displays_side_by_side() {
        let rows = draw(&screen());
        let map = rows[..9].join("\n");
        assert!(map.contains('┌') && map.contains('┘'), "no boxes:\n{map}");
        assert!(map.contains("HDMI-A-1"), "missing ultrawide:\n{map}");
        assert!(map.contains("eDP-1"), "missing laptop:\n{map}");
        // Labels sit on different rows (the boxes differ in height), so compare
        // the columns they start at.
        let col = |needle: &str| {
            rows[..9]
                .iter()
                .find_map(|l| l.find(needle))
                .unwrap_or_else(|| panic!("{needle} not on the map:\n{map}"))
        };
        assert!(
            col("HDMI-A-1") < col("eDP-1"),
            "ultrawide should sit left:\n{map}"
        );
    }

    #[test]
    fn map_survives_a_single_display_and_an_all_off_layout() {
        let mut s = screen();
        s.layout.monitors[0].disabled = true;
        assert!(draw(&s)[..9].join("\n").contains("HDMI-A-1"));
        s.layout.monitors[1].disabled = true;
        assert!(draw(&s)[..9].join("\n").contains("every display is off"));
    }

    #[test]
    fn placing_moves_the_laptop_below_and_centers_it() {
        let mut s = screen();
        assert_eq!(s.layout.monitors[0].name, "eDP-1");
        press(&mut s, KeyCode::Char('p'));
        assert!(s.placing.is_some(), "should have entered placement");
        press(&mut s, KeyCode::Down);
        assert_eq!((s.layout.monitors[0].x, s.layout.monitors[0].y), (0, 1440));
        // Align cycles Start → Center: (3440-1280)/2 = 1080.
        press(&mut s, KeyCode::Char('a'));
        assert_eq!(
            (s.layout.monitors[0].x, s.layout.monitors[0].y),
            (1080, 1440)
        );
        press(&mut s, KeyCode::Enter);
        assert!(s.placing.is_none());
        assert!(s.dirty, "a confirmed placement is unsaved work");
        assert!(s.layout.check(&s.live).is_empty());
    }

    #[test]
    fn escape_restores_the_arrangement_exactly() {
        let mut s = screen();
        let before = s.layout.clone();
        press(&mut s, KeyCode::Char('p'));
        press(&mut s, KeyCode::Left);
        press(&mut s, KeyCode::Char('a'));
        assert_ne!(s.layout, before);
        press(&mut s, KeyCode::Esc);
        assert_eq!(s.layout, before);
        assert!(!s.dirty, "a cancelled placement is not an edit");
    }

    #[test]
    fn placement_opens_on_the_side_the_display_already_sits() {
        let mut s = screen();
        // The laptop starts to the right of the ultrawide, so entering
        // placement must not shove it somewhere else.
        let before = s.layout.clone();
        press(&mut s, KeyCode::Char('p'));
        assert_eq!(s.placing.as_ref().unwrap().side, Side::RightOf);
        assert_eq!(s.layout, before);
    }

    #[test]
    fn placing_refuses_a_disabled_or_lonely_display() {
        let mut s = screen();
        s.layout.monitors[0].disabled = true;
        press(&mut s, KeyCode::Char('p'));
        assert!(s.placing.is_none());
        assert!(s.notice.as_ref().unwrap().contains("press d to enable"));

        // Only the ultrawide is on: nothing to anchor against.
        s.cursor = 1;
        press(&mut s, KeyCode::Char('p'));
        assert!(s.placing.is_none());
        assert!(s.notice.as_ref().unwrap().contains("only one display"));
    }

    #[test]
    fn arrows_do_not_move_the_list_cursor_while_placing() {
        let mut s = screen();
        press(&mut s, KeyCode::Char('p'));
        press(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 0, "Down picks a side, not a row");
    }

    #[test]
    fn footer_surfaces_a_broken_arrangement() {
        let mut s = screen();
        s.layout.set_position("eDP-1", 9000, 9000);
        let rows = draw(&s);
        let footer = rows[22..].join(" ");
        assert!(footer.contains("cursor can't reach"), "{footer}");
    }
}

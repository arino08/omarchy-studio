//! Monitors screen (roadmap 0.8.4).
//!
//! Lists the displays Hyprland reports, lets you nudge scale, flip a monitor
//! on/off, and identify which physical panel is which. Save persists the layout
//! to `monitors.conf` (managed block + hotplug fallback) through the snapshot
//! pipeline, so a bad layout is one `snapshot undo` away.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout as LLayout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use studio_core::cmd::RealRunner;
use studio_core::modules::monitors::{self as mon, Layout, Monitor};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

pub enum MonitorsAction {
    None,
    /// Flash each monitor's name on-screen.
    Identify,
    /// Persist the edited layout (App snapshots + reloads).
    Save(Layout),
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
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths) {
        let keep = self.cursor;
        *self = Self::load(paths);
        self.cursor = crate::tui::ui::clamp_index(keep, self.layout.monitors.len());
    }

    pub fn hint(&self) -> &'static str {
        "↑↓ move · r rate · m resolution · +/- scale · d disable · i identify · s save"
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

    pub fn handle(&mut self, key: KeyEvent) -> MonitorsAction {
        if crate::tui::ui::list_nav(key.code, &mut self.cursor, self.layout.monitors.len()) {
            return MonitorsAction::None;
        }
        match key.code {
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
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(area);

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
        f.render_widget(List::new(items), rows[0]);

        let dirty = if self.dirty {
            Span::styled("  ·  unsaved — s to apply & reload", skin.warn())
        } else {
            Span::styled("", skin.dim())
        };
        let status = match &self.notice {
            Some(msg) => Span::styled(format!("  ·  {msg}"), skin.warn()),
            None => dirty,
        };
        let footer = Paragraph::new(vec![
            Line::from(vec![Span::styled("Displays", skin.dim()), status]),
            Line::from(Span::styled(self.hint(), skin.dim())),
        ]);
        f.render_widget(footer, rows[1]);
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

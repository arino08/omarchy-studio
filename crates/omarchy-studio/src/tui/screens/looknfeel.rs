//! Look & Feel screen (spec 05 §2, roadmap 0.3.1 + 0.3.2).
//!
//! A schema-driven form over the Hyprland look & feel settings. Adjusting a
//! value applies it *live* via `hyprctl keyword` (no file written) so the
//! desktop changes under you; Save persists the batch into the managed block
//! with a snapshot for undo. A preview marker on disk lets a future launch
//! notice if Studio died mid-preview and offer to restore.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use ratatui::widgets::{Block, Borders, Clear};

use studio_core::cmd::CommandRunner;
use studio_core::modules::looknfeel::{groups, Kind, LookFeel, Setting, PRESETS, SETTINGS};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

/// What the screen wants the App to do after a key.
pub enum LookFeelAction {
    None,
    /// Preview the setting at this schema index live.
    Preview(usize),
    /// Preview the whole batch live (after applying a preset).
    PreviewAll,
    /// Persist the batch (App snapshots first).
    Save,
    /// Reset every override and reload to the persisted config.
    ResetAll,
    /// Open the behavior-toggles overlay (App fills in live states).
    OpenToggles,
    /// Flip the toggle with this id (App runs the Omarchy script).
    FlipToggle(String),
}

/// A row in the toggles overlay: id, label, and cached on/off (None = unknown).
pub struct ToggleRow {
    pub id: String,
    pub label: String,
    pub on: Option<bool>,
}

struct ToggleOverlay {
    rows: Vec<ToggleRow>,
    sel: usize,
}

pub struct LookFeelScreen {
    model: LookFeel,
    /// Absolute index into `SETTINGS` (always within the active group).
    selected: usize,
    /// Index into `groups()` — the active category tab.
    active_group: usize,
    scroll: usize,
    /// True once anything has been previewed but not yet saved.
    pub dirty: bool,
    /// Preset picker overlay: Some(selected preset index) while open.
    picker: Option<usize>,
    /// Behavior-toggles overlay.
    toggles: Option<ToggleOverlay>,
}

impl LookFeelScreen {
    pub fn load(paths: &OmarchyPaths) -> Self {
        Self {
            model: LookFeel::load(paths),
            selected: 0,
            active_group: 0,
            scroll: 0,
            dirty: false,
            picker: None,
            toggles: None,
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths) {
        let keep = self.selected;
        self.model = LookFeel::load(paths);
        self.selected = keep.min(SETTINGS.len().saturating_sub(1));
        self.active_group = groups()
            .iter()
            .position(|g| *g == SETTINGS[self.selected].group)
            .unwrap_or(0);
        self.dirty = false;
        self.picker = None;
        self.toggles = None;
    }

    /// Absolute `SETTINGS` indices in the active group, in schema order.
    fn group_indices(&self) -> Vec<usize> {
        let group = groups()[self.active_group];
        SETTINGS
            .iter()
            .enumerate()
            .filter(|(_, s)| s.group == group)
            .map(|(i, _)| i)
            .collect()
    }

    /// Switch to another category tab, landing on its first setting.
    fn switch_group(&mut self, dir: i64) {
        let n = groups().len() as i64;
        self.active_group = ((self.active_group as i64 + dir).rem_euclid(n)) as usize;
        self.selected = self.group_indices()[0];
        self.scroll = 0;
    }

    /// Populate + open the toggles overlay with freshly-read states.
    pub fn open_toggles(&mut self, rows: Vec<ToggleRow>) {
        self.toggles = Some(ToggleOverlay { rows, sel: 0 });
    }

    /// Update one toggle's cached state after a flip.
    pub fn set_toggle_state(&mut self, id: &str, on: Option<bool>) {
        if let Some(ov) = self.toggles.as_mut() {
            if let Some(r) = ov.rows.iter_mut().find(|r| r.id == id) {
                r.on = on;
            }
        }
    }

    fn cur(&self) -> &Setting {
        &SETTINGS[self.selected]
    }

    /// True while a modal (preset picker / toggles) owns input — the App
    /// suspends its global chords so digits/letters reach us.
    pub fn is_modal(&self) -> bool {
        self.picker.is_some() || self.toggles.is_some()
    }

    pub fn handle(&mut self, key: KeyEvent) -> LookFeelAction {
        if self.picker.is_some() {
            return self.handle_picker(key);
        }
        if self.toggles.is_some() {
            return self.handle_toggles(key);
        }
        let idx = self.group_indices();
        // Navigate a position *within* this group's settings, then map back to
        // the absolute index the rest of the screen keys off.
        let mut pos = idx.iter().position(|&i| i == self.selected).unwrap_or(0);
        if crate::tui::ui::list_nav(key.code, &mut pos, idx.len()) {
            if let Some(&sel) = idx.get(pos) {
                self.selected = sel;
            }
            return LookFeelAction::None;
        }
        match key.code {
            KeyCode::Char('p') => {
                self.picker = Some(0);
                return LookFeelAction::None;
            }
            KeyCode::Char('t') => return LookFeelAction::OpenToggles,
            // Tab / shift-tab move between the category tabs ([ ] alias them).
            KeyCode::Tab | KeyCode::Char(']') => self.switch_group(1),
            KeyCode::BackTab | KeyCode::Char('[') => self.switch_group(-1),
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => return self.nudge(1),
            KeyCode::Left | KeyCode::Char('-') | KeyCode::Char(' ') => return self.nudge(-1),
            KeyCode::Enter => return self.nudge(1),
            KeyCode::Char('r') => return self.reset_selected(),
            KeyCode::Char('R') => {
                self.model.clear_all();
                self.dirty = false;
                return LookFeelAction::ResetAll;
            }
            KeyCode::Char('s') => return LookFeelAction::Save,
            _ => {}
        }
        LookFeelAction::None
    }

    /// Change the selected setting by one step in `dir` and request a preview.
    fn nudge(&mut self, dir: i64) -> LookFeelAction {
        let setting = *self.cur();
        let current = self.model.value(setting.key);
        let next = match setting.kind {
            Kind::Bool => {
                let on = matches!(current.as_str(), "true" | "yes" | "on" | "1");
                if on {
                    "false".to_string()
                } else {
                    "true".to_string()
                }
            }
            Kind::Enum(opts) => {
                let i = opts.iter().position(|o| *o == current).unwrap_or(0) as i64;
                let j = (i + dir).rem_euclid(opts.len() as i64) as usize;
                opts[j].to_string()
            }
            Kind::Int { min, max } => {
                let v: i64 = current.parse().unwrap_or(min);
                (v + dir).clamp(min, max).to_string()
            }
            Kind::Float { min, max } => {
                let v: f64 = current.parse().unwrap_or(min);
                let step = 0.05;
                let nv = (v + dir as f64 * step).clamp(min, max);
                format!("{:.2}", nv)
            }
        };
        if self.model.set(setting.key, &next).is_ok() {
            self.dirty = true;
            return LookFeelAction::Preview(self.selected);
        }
        LookFeelAction::None
    }

    fn reset_selected(&mut self) -> LookFeelAction {
        let key = self.cur().key;
        self.model.clear(key);
        self.dirty = true;
        // preview the reverted (base/default) value live
        LookFeelAction::Preview(self.selected)
    }

    fn handle_toggles(&mut self, key: KeyEvent) -> LookFeelAction {
        let Some(ov) = self.toggles.as_mut() else {
            return LookFeelAction::None;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('t') => self.toggles = None,
            KeyCode::Down => ov.sel = (ov.sel + 1).min(ov.rows.len() - 1),
            KeyCode::Up => ov.sel = ov.sel.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let id = ov.rows[ov.sel].id.clone();
                return LookFeelAction::FlipToggle(id);
            }
            _ => {}
        }
        LookFeelAction::None
    }

    fn handle_picker(&mut self, key: KeyEvent) -> LookFeelAction {
        let Some(sel) = self.picker.as_mut() else {
            return LookFeelAction::None;
        };
        match key.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Down => *sel = (*sel + 1).min(PRESETS.len() - 1),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Enter => {
                let preset = &PRESETS[*sel];
                self.model.apply_preset(preset);
                self.dirty = true;
                self.picker = None;
                return LookFeelAction::PreviewAll;
            }
            _ => {}
        }
        LookFeelAction::None
    }

    // ── IO delegated from the App (keeps the screen itself pure of runners) ──

    pub fn preview(
        &self,
        idx: usize,
        runner: &dyn CommandRunner,
    ) -> studio_core::error::Result<()> {
        self.model.preview_one(SETTINGS[idx].key, runner)
    }

    pub fn preview_all(&self, runner: &dyn CommandRunner) -> studio_core::error::Result<()> {
        self.model.preview_all(runner)
    }

    /// Persist through the apply pipeline — the store is the App's, since it
    /// owns every side effect. `summary` becomes the snapshot subject.
    pub fn commit(
        &self,
        paths: &OmarchyPaths,
        store: &studio_core::snapshot::SnapshotStore,
        runner: &dyn CommandRunner,
        summary: &str,
    ) -> studio_core::error::Result<()> {
        self.model.apply(paths, store, runner, summary).map(|_| ())
    }

    pub fn any_overrides(&self) -> bool {
        self.model.any_overrides()
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, skin: &Skin) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(area);

        self.render_tabs(f, rows[0], skin);

        // Only the active group's settings; selection scrolls within it.
        let idx = self.group_indices();
        let sel_row = idx.iter().position(|&i| i == self.selected).unwrap_or(0);
        let height = rows[1].height as usize;
        if sel_row < self.scroll {
            self.scroll = sel_row;
        } else if sel_row >= self.scroll + height {
            self.scroll = sel_row + 1 - height;
        }
        let items: Vec<ListItem> = idx
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|&i| self.setting_row(i, &SETTINGS[i], skin))
            .collect();
        f.render_widget(List::new(items), rows[1]);

        self.render_footer(f, rows[2], skin);

        if let Some(sel) = self.picker {
            self.render_picker(f, area, skin, sel);
        }
        if let Some(ov) = &self.toggles {
            self.render_toggles(f, area, skin, ov);
        }
    }

    /// The category tab strip. The active group is highlighted; an overridden
    /// count hint could go here later.
    fn render_tabs(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let mut spans = Vec::new();
        for (i, g) in groups().iter().enumerate() {
            if i == self.active_group {
                spans.push(Span::styled(format!(" {g} "), skin.selection()));
            } else {
                spans.push(Span::styled(format!(" {g} "), skin.dim()));
            }
            spans.push(Span::styled("·", skin.dim()));
        }
        spans.pop(); // trailing separator
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_toggles(&self, f: &mut Frame, area: Rect, skin: &Skin, ov: &ToggleOverlay) {
        let rect = crate::tui::ui::centered_rect(area, 48, ov.rows.len() as u16 + 2);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Behavior toggles — Enter to flip, Esc to close ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let items: Vec<ListItem> = ov
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let on = i == ov.sel;
                let mark = match r.on {
                    Some(true) => "[on] ",
                    Some(false) => "[off]",
                    None => "[ · ]",
                };
                let style = if on { skin.selection() } else { skin.body() };
                ListItem::new(Line::from(vec![
                    Span::styled(if on { "▸ " } else { "  " }, skin.accent_bold()),
                    Span::styled(format!("{mark} "), skin.accent_bold()),
                    Span::styled(r.label.clone(), style),
                ]))
            })
            .collect();
        f.render_widget(List::new(items), inner);
    }

    fn render_picker(&self, f: &mut Frame, area: Rect, skin: &Skin, sel: usize) {
        // each preset renders on two lines (name + blurb), plus the border.
        let rect = crate::tui::ui::centered_rect(area, 52, PRESETS.len() as u16 * 2 + 2);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Presets — Enter to try, Esc to cancel ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        let items: Vec<ListItem> = PRESETS
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let on = i == sel;
                let name = if on {
                    Span::styled(format!("▸ {}", p.name), skin.selection())
                } else {
                    Span::styled(format!("  {}", p.name), skin.body())
                };
                ListItem::new(vec![
                    Line::from(name),
                    Line::from(Span::styled(format!("    {}", p.blurb), skin.dim())),
                ])
            })
            .collect();
        f.render_widget(List::new(items), inner);
    }

    fn setting_row<'a>(&self, i: usize, s: &'a Setting, skin: &Skin) -> ListItem<'a> {
        let selected = i == self.selected;
        let value = display_value(&self.model.value(s.key), s.kind);
        let overridden = self.model.is_overridden(s.key);
        let base = if selected {
            skin.selection()
        } else {
            skin.body()
        };
        let dot = if overridden { "●" } else { " " };
        ListItem::new(Line::from(vec![
            Span::styled(if selected { "  ▸ " } else { "    " }, skin.accent_bold()),
            Span::styled(dot.to_string(), skin.accent_bold()),
            Span::styled(format!(" {:<20}", s.label), base),
            Span::styled(format!("{value:>10}"), skin.accent_bold()),
        ]))
    }

    fn render_footer(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let s = self.cur();
        let range = match s.kind {
            Kind::Int { min, max } => format!("  ({min}–{max})"),
            Kind::Float { min, max } => format!("  ({min}–{max})"),
            Kind::Enum(opts) => format!("  ({})", opts.join(" / ")),
            Kind::Bool => String::new(),
        };
        let save = if self.dirty {
            Span::styled("  ·  unsaved — s to keep", skin.warn())
        } else {
            Span::styled("", skin.dim())
        };
        let p = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(format!("{}{range}", s.help), skin.dim()),
                save,
            ]),
            Line::from(Span::styled(
                "tab category   ←→ adjust   p presets   t toggles   r reset   R reset all   s save",
                skin.dim(),
            )),
        ])
        .wrap(Wrap { trim: true });
        f.render_widget(p, area);
    }
}

fn display_value(raw: &str, kind: Kind) -> String {
    match kind {
        Kind::Bool => {
            if matches!(raw, "true" | "yes" | "on" | "1") {
                "on".into()
            } else {
                "off".into()
            }
        }
        _ => raw.to_string(),
    }
}

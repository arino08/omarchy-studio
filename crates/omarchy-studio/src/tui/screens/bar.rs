//! Omarchy 4 shell: bar layout (roadmap O4.5).
//!
//! Lists the bar's three lanes; reorder within a lane, move to an adjacent
//! one, add a widget from the catalog, remove one, or flip position/
//! transparency. Every action applies immediately through Omarchy's own
//! `omarchy bar`/`omarchy plugin` commands — there is no local "dirty" buffer
//! to save, the same split as the Tweaks screen: this screen reports intent,
//! the App runs the command and owns the snapshot store.
//!
//! Reordering targets a *neighbor id* (`--before`/`--after`) rather than a
//! raw index, so a move can't be thrown off by the list having shifted under
//! it — the same relative addressing `omarchy-bar`'s own docs show.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use studio_core::cmd::RealRunner;
use studio_core::modules::shell::{bar_widget_catalog, PluginInfo, Section, Shell};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

/// What the user asked to do. The App performs it (owns the snapshot store
/// and the real command run) and then calls [`BarScreen::reload`].
pub enum BarAction {
    None,
    /// `omarchy bar move <id> <args>`.
    Move {
        id: String,
        args: Vec<String>,
    },
    /// `omarchy plugin disable <id>`.
    Remove {
        id: String,
    },
    /// `omarchy plugin enable <id> <args>`.
    Add {
        id: String,
        args: Vec<String>,
    },
    /// `omarchy bar position <pos>`.
    Position(&'static str),
    /// `omarchy bar transparent toggle`.
    ToggleTransparent,
}

const POSITIONS: [&str; 4] = ["top", "bottom", "left", "right"];

pub struct BarScreen {
    shell: Shell,
    cursor: usize,
    /// Bar-widget catalog for the add picker — one `omarchy plugin list`
    /// exec, cached for the screen's lifetime rather than re-run per frame.
    catalog: Vec<PluginInfo>,
    catalog_error: Option<String>,
    picker: Option<usize>,
}

impl BarScreen {
    pub fn load(paths: &OmarchyPaths) -> Self {
        let shell = Shell::load(paths);
        let (catalog, catalog_error) = match bar_widget_catalog(&RealRunner) {
            Ok(c) => (c, None),
            Err(e) => (Vec::new(), Some(format!("{e:?}"))),
        };
        Self {
            shell,
            cursor: 0,
            catalog,
            catalog_error,
            picker: None,
        }
    }

    /// Re-read `shell.json` after an apply, keeping the catalog (one exec —
    /// no need to re-run it for a layout change) and the cursor position.
    pub fn reload(&mut self, paths: &OmarchyPaths) {
        self.shell = Shell::load(paths);
        self.clamp_cursor();
    }

    pub fn is_modal(&self) -> bool {
        self.picker.is_some()
    }

    pub fn hint(&self) -> &'static str {
        "↑↓ move · shift+↑↓ reorder · ←→ change lane · a add · d remove · p position · t transparency"
    }

    /// (section, widget id) pairs in display order.
    fn flat(&self) -> Vec<(Section, String)> {
        let mut v = Vec::new();
        for s in Section::ALL {
            for w in self.shell.cfg.bar.layout.section(s) {
                v.push((s, w.id.clone()));
            }
        }
        v
    }

    fn clamp_cursor(&mut self) {
        let n = self.flat().len();
        if n == 0 {
            self.cursor = 0;
        } else if self.cursor >= n {
            self.cursor = n - 1;
        }
    }

    fn selected(&self) -> Option<(Section, String)> {
        self.flat().get(self.cursor).cloned()
    }

    pub fn handle(&mut self, key: KeyEvent) -> BarAction {
        if self.picker.is_some() {
            return self.handle_picker(key);
        }
        let n = self.flat().len();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Down if shift => return self.move_within(1),
            KeyCode::Up if shift => return self.move_within(-1),
            KeyCode::Right | KeyCode::Char('>') => return self.move_lane(1),
            KeyCode::Left | KeyCode::Char('<') => return self.move_lane(-1),
            KeyCode::Down if n > 0 => self.cursor = (self.cursor + 1).min(n - 1),
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = n.saturating_sub(1),
            KeyCode::Char('a') if !self.catalog.is_empty() => self.picker = Some(0),
            KeyCode::Char('d') | KeyCode::Char('x') => {
                if let Some((_, id)) = self.selected() {
                    return BarAction::Remove { id };
                }
            }
            KeyCode::Char('p') => {
                let cur = self.shell.cfg.bar.position.as_str();
                let idx = POSITIONS.iter().position(|p| *p == cur).unwrap_or(0);
                return BarAction::Position(POSITIONS[(idx + 1) % POSITIONS.len()]);
            }
            KeyCode::Char('t') => return BarAction::ToggleTransparent,
            _ => {}
        }
        BarAction::None
    }

    /// Swap with the neighbor `dir` away in the same lane, addressed by id
    /// (`--after`/`--before`) rather than an index that could drift.
    fn move_within(&mut self, dir: i64) -> BarAction {
        let Some((section, id)) = self.selected() else {
            return BarAction::None;
        };
        let widgets = self.shell.cfg.bar.layout.section(section);
        let Some(pos) = widgets.iter().position(|w| w.id == id) else {
            return BarAction::None;
        };
        let target = pos as i64 + dir;
        if target < 0 || target as usize >= widgets.len() {
            return BarAction::None;
        }
        let neighbor = widgets[target as usize].id.clone();
        let flag = if dir > 0 { "--after" } else { "--before" };
        self.cursor = (self.cursor as i64 + dir).max(0) as usize;
        BarAction::Move {
            id,
            args: vec![flag.to_string(), neighbor],
        }
    }

    /// Move the selected widget to the adjacent lane (appended there).
    fn move_lane(&mut self, dir: i64) -> BarAction {
        let Some((section, id)) = self.selected() else {
            return BarAction::None;
        };
        let idx = Section::ALL.iter().position(|s| *s == section).unwrap_or(0);
        let target = idx as i64 + dir;
        if target < 0 || target as usize >= Section::ALL.len() {
            return BarAction::None;
        }
        let to = Section::ALL[target as usize];
        BarAction::Move {
            id,
            args: vec!["--section".to_string(), to.as_str().to_string()],
        }
    }

    fn handle_picker(&mut self, key: KeyEvent) -> BarAction {
        let Some(sel) = self.picker.as_mut() else {
            return BarAction::None;
        };
        match key.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Down => *sel = (*sel + 1).min(self.catalog.len().saturating_sub(1)),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Enter => {
                let id = self.catalog[*sel].id.clone();
                let section = self.selected().map(|(s, _)| s).unwrap_or(Section::Left);
                self.picker = None;
                return BarAction::Add {
                    id,
                    args: vec!["--section".to_string(), section.as_str().to_string()],
                };
            }
            _ => {}
        }
        BarAction::None
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(area);

        let header = Line::from(vec![
            Span::styled("  Omarchy 4 shell bar", skin.dim()),
            Span::styled("  ·  position ", skin.dim()),
            Span::styled(self.shell.cfg.bar.position.clone(), skin.accent_bold()),
            Span::styled("  ·  transparent ", skin.dim()),
            Span::styled(
                if self.shell.cfg.bar.transparent {
                    "on"
                } else {
                    "off"
                },
                skin.accent_bold(),
            ),
        ]);
        f.render_widget(Paragraph::new(header), rows[0]);

        let mut items: Vec<ListItem> = Vec::new();
        let mut flat_i = 0usize;
        for section in Section::ALL {
            let title = match section {
                Section::Left => "Left",
                Section::Center => "Center",
                Section::Right => "Right",
            };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {title}"),
                skin.dim(),
            ))));
            let widgets = self.shell.cfg.bar.layout.section(section);
            if widgets.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "      (empty)",
                    skin.dim(),
                ))));
            }
            for w in widgets {
                let selected = flat_i == self.cursor;
                let style = if selected {
                    skin.selection()
                } else {
                    skin.body()
                };
                let name = self
                    .catalog
                    .iter()
                    .find(|p| p.id == w.id)
                    .map(|p| p.name.as_str())
                    .unwrap_or(w.id.as_str());
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(if selected { "  ▸ " } else { "    " }, skin.accent_bold()),
                    Span::styled(format!("{name:<24}"), style),
                    Span::styled(w.id.clone(), skin.dim()),
                ])));
                flat_i += 1;
            }
        }
        f.render_widget(List::new(items), rows[1]);

        let mut footer_lines = vec![Line::from(Span::styled(
            "Bar layout — Omarchy 4 shell",
            skin.dim(),
        ))];
        if let Some(err) = &self.catalog_error {
            footer_lines.push(Line::from(Span::styled(
                format!("couldn't load the widget catalog ({err}) — add is unavailable"),
                skin.warn(),
            )));
        } else {
            footer_lines.push(Line::from(Span::styled(self.hint(), skin.dim())));
        }
        f.render_widget(Paragraph::new(footer_lines), rows[2]);

        if let Some(sel) = self.picker {
            self.render_picker(f, area, skin, sel);
        }
    }

    fn render_picker(&self, f: &mut Frame, area: Rect, skin: &Skin, sel: usize) {
        let rect = crate::tui::ui::centered_rect(area, 56, 16);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Add a widget — Enter to add, Esc to cancel ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let view = (inner.height as usize).max(1);
        let scroll = sel.saturating_sub(view.saturating_sub(1));
        let items: Vec<ListItem> = self
            .catalog
            .iter()
            .enumerate()
            .skip(scroll)
            .take(view)
            .map(|(i, p)| {
                let on = i == sel;
                let mark = if p.enabled { "on bar  " } else { "        " };
                let name_style = if on { skin.selection() } else { skin.body() };
                let marker = if on { "▸ " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{marker}{mark}"), skin.dim()),
                    Span::styled(format!("{:<28}", p.name), name_style),
                    Span::styled(p.id.clone(), skin.dim()),
                ]))
            })
            .collect();
        f.render_widget(List::new(items), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::modules::shell::{BarLayout, BarSection, BarWidget, ShellConfig};

    fn screen_with(left: &[&str], center: &[&str], right: &[&str]) -> BarScreen {
        let widget = |id: &str| BarWidget {
            id: id.to_string(),
            settings: Default::default(),
        };
        let cfg = ShellConfig {
            bar: BarSection {
                layout: BarLayout {
                    left: left.iter().map(|s| widget(s)).collect(),
                    center: center.iter().map(|s| widget(s)).collect(),
                    right: right.iter().map(|s| widget(s)).collect(),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        BarScreen {
            shell: Shell::from_config(std::path::PathBuf::from("/tmp/test-shell.json"), cfg),
            cursor: 0,
            catalog: Vec::new(),
            catalog_error: None,
            picker: None,
        }
    }

    #[test]
    fn move_within_targets_the_neighbor_by_id_not_index() {
        let mut s = screen_with(&["a", "b", "c"], &[], &[]);
        s.cursor = 0; // "a"
        match s.move_within(1) {
            BarAction::Move { id, args } => {
                assert_eq!(id, "a");
                assert_eq!(args, vec!["--after", "b"]);
            }
            _ => panic!("expected a move"),
        }
        assert_eq!(s.cursor, 1, "cursor follows the moved item");
    }

    #[test]
    fn move_within_at_the_edge_is_a_no_op() {
        let mut s = screen_with(&["a", "b"], &[], &[]);
        s.cursor = 0;
        assert!(matches!(s.move_within(-1), BarAction::None));
    }

    #[test]
    fn move_lane_targets_the_adjacent_section() {
        let mut s = screen_with(&["a"], &["b"], &[]);
        s.cursor = 0; // "a" in left
        match s.move_lane(1) {
            BarAction::Move { id, args } => {
                assert_eq!(id, "a");
                assert_eq!(args, vec!["--section", "center"]);
            }
            _ => panic!("expected a move"),
        }
        // Right of the last lane is a no-op.
        let mut s = screen_with(&[], &[], &["z"]);
        assert!(matches!(s.move_lane(1), BarAction::None));
    }
}

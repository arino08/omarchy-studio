//! Keybinds screen (spec 05, roadmap 0.2.6).
//!
//! Shows the *effective* keymap — what Hyprland is actually doing right now,
//! read from `hyprctl binds -j` and attributed to the layer that defines each
//! bind. The user can disable a shortcut or press a new chord to rebind an
//! action; both are written as a managed override block in their own
//! bindings.conf and applied with a live reload. No config syntax in sight.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use studio_core::cmd::CommandRunner;
use studio_core::configfs::Layer;
use studio_core::modules::dispatchers::{label_for, lookup};
use studio_core::modules::keybinds::{
    apply_overrides, load_effective, read_overrides, AttributedBind, ConfigBind, Override,
};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::chord;
use crate::tui::theme::Skin;

/// What the screen wants the App to do after a key.
pub enum KeybindAction {
    None,
    /// Persist the current overrides and live-reload (App snapshots first).
    Commit(String),
}

pub struct KeybindsScreen {
    /// Effective binds, sorted for display.
    binds: Vec<AttributedBind>,
    /// Session overrides (seeded from the existing block, mutated by edits).
    overrides: Vec<Override>,
    selected: usize,
    scroll: usize,
    /// Friendly message when the keymap can't be loaded (e.g. Hyprland down).
    error: Option<String>,
    /// True while waiting for the user to press a new chord.
    capturing: bool,
}

impl KeybindsScreen {
    pub fn load(paths: &OmarchyPaths, runner: &dyn CommandRunner) -> Self {
        let overrides = read_overrides(paths);
        let (binds, error) = match load_effective(paths, runner) {
            Ok(mut b) => {
                b.sort_by(|x, y| {
                    (x.runtime.modmask, x.runtime.key.to_ascii_uppercase())
                        .cmp(&(y.runtime.modmask, y.runtime.key.to_ascii_uppercase()))
                });
                (b, None)
            }
            Err(e) => (Vec::new(), Some(friendly(&e))),
        };
        Self {
            binds,
            overrides,
            selected: 0,
            scroll: 0,
            error,
            capturing: false,
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths, runner: &dyn CommandRunner) {
        let keep = self.selected;
        *self = Self::load(paths, runner);
        self.selected = crate::tui::ui::clamp_index(keep, self.binds.len());
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    fn selected_bind(&self) -> Option<&AttributedBind> {
        self.binds.get(self.selected)
    }

    pub fn handle(&mut self, key: KeyEvent) -> KeybindAction {
        if self.capturing {
            return self.handle_capture(key);
        }
        if self.error.is_some() {
            return KeybindAction::None;
        }
        let n = self.binds.len();
        if crate::tui::ui::list_nav(key.code, &mut self.selected, n) {
            return KeybindAction::None;
        }
        match key.code {
            KeyCode::Char('x') => return self.disable_selected(),
            KeyCode::Char('r') if n > 0 => self.capturing = true,
            _ => {}
        }
        KeybindAction::None
    }

    /// Disable the selected chord: append an `unbind` override.
    fn disable_selected(&mut self) -> KeybindAction {
        let Some(b) = self.selected_bind() else {
            return KeybindAction::None;
        };
        let (mask, key) = (b.runtime.modmask, b.runtime.key.clone());
        let chord = studio_core::modules::keybinds::render_chord(mask, &key);
        self.push_disable(mask, &key);
        KeybindAction::Commit(format!("Disabled {chord}"))
    }

    fn handle_capture(&mut self, key: KeyEvent) -> KeybindAction {
        if key.code == KeyCode::Esc {
            self.capturing = false;
            return KeybindAction::None;
        }
        let Some((mask, new_key)) = chord::capture(&key) else {
            return KeybindAction::None; // bare modifier: keep waiting
        };
        self.capturing = false;

        let Some(b) = self.selected_bind() else {
            return KeybindAction::None;
        };
        // Rebind = move this action to the new chord: cancel the old chord and
        // bind the new one to the same action.
        let (old_mask, old_key) = (b.runtime.modmask, b.runtime.key.clone());
        let new = ConfigBind {
            flags: "bind".into(),
            modmask: mask,
            key: new_key.clone(),
            description: b.source.as_ref().and_then(|s| s.config.description.clone()),
            dispatcher: b.runtime.dispatcher.clone(),
            arg: b.runtime.arg.clone(),
        };
        let new_chord = studio_core::modules::keybinds::render_chord(mask, &new_key);
        let action = label_for(&b.runtime.dispatcher).to_string();
        self.push_disable(old_mask, &old_key);
        self.overrides.push(Override::Set(new));
        KeybindAction::Commit(format!("{action} → {new_chord}"))
    }

    /// Add a Disable override for a chord, de-duplicating.
    fn push_disable(&mut self, mask: u16, key: &str) {
        let dup = self.overrides.iter().any(
            |o| matches!(o, Override::Disable { modmask, key: k } if *modmask == mask && k == key),
        );
        if !dup {
            self.overrides.push(Override::Disable {
                modmask: mask,
                key: key.to_string(),
            });
        }
    }

    /// Commit the current overrides through the apply pipeline, which
    /// snapshots and rolls back a config Hyprland refuses. The store is the
    /// App's — screens report intent, the App owns side effects.
    pub fn commit(
        &self,
        paths: &OmarchyPaths,
        store: &studio_core::snapshot::SnapshotStore,
        runner: &dyn CommandRunner,
        summary: &str,
    ) -> studio_core::error::Result<()> {
        apply_overrides(paths, &self.overrides, store, runner, summary).map(|_| ())
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, skin: &Skin) {
        if let Some(msg) = &self.error {
            let mut lines = vec![
                Line::from(Span::styled("Can't read your keybinds", skin.accent_bold())),
                Line::from(""),
            ];
            // `friendly()` separates the cause from the hint with a blank
            // line; a Line never breaks on '\n' itself, so split here or the
            // two run together.
            lines.extend(
                msg.split('\n')
                    .map(|l| Line::from(Span::styled(l.to_string(), skin.body()))),
            );
            let p = Paragraph::new(lines).wrap(Wrap { trim: false });
            f.render_widget(p, area);
            return;
        }

        // reserve a footer line for the detail/help
        let rows = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Min(1),
                ratatui::layout::Constraint::Length(2),
            ])
            .split(area);

        let height = rows[0].height as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }

        let items: Vec<ListItem> = self
            .binds
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(height)
            .map(|(i, b)| self.row(i, b, skin))
            .collect();
        f.render_widget(List::new(items), rows[0]);

        self.render_footer(f, rows[1], skin);

        if self.capturing {
            self.render_capture(f, area, skin);
        }
    }

    fn row<'a>(&self, i: usize, b: &'a AttributedBind, skin: &Skin) -> ListItem<'a> {
        let selected = i == self.selected;
        let chord = b.runtime.chord();
        let action = b
            .source
            .as_ref()
            .and_then(|s| s.config.description.clone())
            .unwrap_or_else(|| label_for(&b.runtime.dispatcher).to_string());
        let tag = source_tag(b);
        let base = if selected {
            skin.selection()
        } else {
            skin.body()
        };
        ListItem::new(Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, skin.accent_bold()),
            Span::styled(format!("{chord:<22}"), skin.accent_bold()),
            Span::styled(format!("{action:<34}"), base),
            Span::styled(tag, skin.dim()),
        ]))
    }

    fn render_footer(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let detail = if let Some(b) = self.selected_bind() {
            let cat = lookup(&b.runtime.dispatcher)
                .map(|d| d.category.label())
                .unwrap_or("Other");
            let arg = if b.runtime.arg.is_empty() {
                String::new()
            } else {
                format!("  ({})", b.runtime.arg)
            };
            format!("{cat} · {}{arg}", b.runtime.dispatcher)
        } else {
            "No binds loaded".into()
        };
        let help = "↑↓ move   r rebind (press a new shortcut)   x disable";
        let p = Paragraph::new(vec![
            Line::from(Span::styled(detail, skin.dim())),
            Line::from(Span::styled(help, skin.dim())),
        ]);
        f.render_widget(p, area);
    }

    fn render_capture(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let rect = crate::tui::ui::centered_rect(area, 46, 5);
        f.render_widget(ratatui::widgets::Clear, rect);
        let action = self
            .selected_bind()
            .map(|b| label_for(&b.runtime.dispatcher).to_string())
            .unwrap_or_default();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Press the new shortcut ");
        let p = Paragraph::new(vec![
            Line::from(Span::styled(format!("Rebinding: {action}"), skin.body())),
            Line::from(""),
            Line::from(Span::styled(
                "… waiting for keys   (Esc to cancel)",
                skin.dim(),
            )),
        ])
        .block(block);
        f.render_widget(p, rect);
    }
}

fn source_tag(b: &AttributedBind) -> &'static str {
    match b.source.as_ref().map(|s| s.layer) {
        Some(Layer::User) => "you",
        Some(Layer::Default) => "omarchy",
        Some(Layer::Theme) => "theme",
        Some(Layer::Toggle) => "toggle",
        Some(Layer::Runtime) | None => "plugin",
    }
}

fn friendly(e: &studio_core::StudioError) -> String {
    match e {
        studio_core::StudioError::External { detail, .. } => {
            format!("{detail}\n\nOpen this screen from inside a running Hyprland session and make sure hyprctl is on your PATH.")
        }
        other => format!("{other:?}"),
    }
}

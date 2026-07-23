//! Lock & Idle screen (spec 05 §7, roadmap 0.5.4).
//!
//! One view over two files: the hypridle idle *timeline* (retime each stage)
//! and the hyprlock *form* (avatar, blur, dim). Edits accumulate in-memory;
//! Save writes both — hypridle restarts, hyprlock backs up on first write. The
//! avatar opens a picker over `~/.config/omarchy/profile_images/`.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use studio_core::modules::lockidle::{Hypridle, Hyprlock, Listener};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

pub enum LockIdleAction {
    None,
    /// Persist idle + lock (App snapshots first).
    Save,
}

/// The editable rows: idle stages first, then the lock-form fields.
enum RowKind {
    Idle(usize), // index into the timeline
    Blur,
    Dim,
    AvatarSize,
    Avatar,
}

pub struct LockIdleScreen {
    /// Kept so the avatar import can resolve the picker directory.
    paths: OmarchyPaths,
    idle: Option<Hypridle>,
    lock: Option<Hyprlock>,
    timeline: Vec<Listener>,
    avatars: Vec<PathBuf>,
    error: Option<String>,
    selected: usize,
    pub dirty: bool,
    /// Avatar picker overlay: Some(index) while open.
    picker: Option<usize>,
    /// `i` in the picker: the path being typed for an image to import.
    import: Option<String>,
    /// Result of the last import, shown in the picker.
    import_msg: Option<String>,
}

impl LockIdleScreen {
    pub fn load(paths: &OmarchyPaths) -> Self {
        let idle = Hypridle::load(paths).ok();
        let lock = Hyprlock::load(paths).ok();
        let timeline = idle.as_ref().map(|i| i.timeline()).unwrap_or_default();
        let avatars = Hyprlock::avatar_choices(paths);
        let error = if idle.is_none() && lock.is_none() {
            Some("Couldn't read hypridle.conf or hyprlock.conf.".into())
        } else {
            None
        };
        Self {
            paths: paths.clone(),
            idle,
            lock,
            timeline,
            avatars,
            error,
            selected: 0,
            dirty: false,
            picker: None,
            import: None,
            import_msg: None,
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths) {
        let keep = self.selected;
        *self = Self::load(paths);
        self.selected = crate::tui::ui::clamp_index(keep, self.rows().len());
    }

    pub fn is_modal(&self) -> bool {
        self.picker.is_some()
    }

    /// The ordered editable rows, given what loaded.
    fn rows(&self) -> Vec<RowKind> {
        let mut v: Vec<RowKind> = Vec::new();
        for i in 0..self.timeline.len() {
            v.push(RowKind::Idle(i));
        }
        if self.lock.is_some() {
            v.push(RowKind::Blur);
            v.push(RowKind::Dim);
            v.push(RowKind::AvatarSize);
            v.push(RowKind::Avatar);
        }
        v
    }

    pub fn handle(&mut self, key: KeyEvent) -> LockIdleAction {
        if self.picker.is_some() {
            return self.handle_picker(key);
        }
        let n = self.rows().len();
        if n == 0 {
            return LockIdleAction::None;
        }
        if crate::tui::ui::list_nav(key.code, &mut self.selected, n) {
            return LockIdleAction::None;
        }
        match key.code {
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => self.nudge(1),
            KeyCode::Left | KeyCode::Char('-') => self.nudge(-1),
            // Opens even with nothing to pick: the picker is where `i` imports
            // an image, so an empty collection has to be reachable — otherwise
            // Enter on the Avatar row silently does nothing and there is no
            // way in at all.
            KeyCode::Enter if matches!(self.rows().get(self.selected), Some(RowKind::Avatar)) => {
                self.picker = Some(0);
                self.import_msg = None;
            }
            KeyCode::Char('s') if self.dirty => return LockIdleAction::Save,
            _ => {}
        }
        LockIdleAction::None
    }

    fn nudge(&mut self, dir: i64) {
        match self.rows().get(self.selected) {
            Some(RowKind::Idle(i)) => {
                let Some(listener) = self.timeline.get(*i).cloned() else {
                    return;
                };
                // step by 30s, floor 5s
                let next = (listener.timeout + dir * 30).max(5);
                if let Some(idle) = self.idle.as_mut() {
                    if idle.set_timeout(&listener, next).is_ok() {
                        self.timeline = idle.timeline();
                        self.dirty = true;
                    }
                }
            }
            Some(RowKind::Blur) => {
                if let Some(lock) = self.lock.as_mut() {
                    let cur: i64 = lock.blur_passes().and_then(|v| v.parse().ok()).unwrap_or(0);
                    if lock.set_blur_passes((cur + dir).clamp(0, 10)) {
                        self.dirty = true;
                    }
                }
            }
            Some(RowKind::Dim) => {
                if let Some(lock) = self.lock.as_mut() {
                    let cur: f64 = lock.dim().and_then(|v| v.parse().ok()).unwrap_or(1.0);
                    if lock.set_dim((cur + dir as f64 * 0.05).clamp(0.0, 1.0)) {
                        self.dirty = true;
                    }
                }
            }
            Some(RowKind::AvatarSize) => {
                if let Some(lock) = self.lock.as_mut() {
                    let cur: i64 = lock
                        .avatar_size()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(200);
                    if lock.set_avatar_size((cur + dir * 10).clamp(32, 512)) {
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_picker(&mut self, key: KeyEvent) -> LockIdleAction {
        // Path entry owns every key while open.
        if let Some(buf) = self.import.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let typed = self.import.take().unwrap_or_default();
                    self.do_import(&typed);
                }
                KeyCode::Esc => self.import = None,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return LockIdleAction::None;
        }

        let Some(sel) = self.picker.as_mut() else {
            return LockIdleAction::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.import_msg = None;
            }
            KeyCode::Char('i') => self.import = Some(String::new()),
            // `len() - 1` underflows on an empty collection, which the picker
            // can now be opened with.
            KeyCode::Down => *sel = (*sel + 1).min(self.avatars.len().saturating_sub(1)),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(choice) = self.avatars.get(*sel).map(|p| p.display().to_string()) {
                    if let Some(lock) = self.lock.as_mut() {
                        if lock.set_avatar(&choice) {
                            self.dirty = true;
                        }
                    }
                    self.picker = None;
                    self.import_msg = None;
                }
            }
            _ => {}
        }
        LockIdleAction::None
    }

    /// Copy an image into the avatar collection and select it, so importing is
    /// one step rather than import-then-find-it.
    fn do_import(&mut self, typed: &str) {
        match Hyprlock::import_avatar(&self.paths, typed) {
            Ok(dest) => {
                self.avatars = Hyprlock::avatar_choices(&self.paths);
                let name = dest
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Some(lock) = self.lock.as_mut() {
                    if lock.set_avatar(&dest.display().to_string()) {
                        self.dirty = true;
                    }
                }
                self.picker = self.avatars.iter().position(|p| *p == dest).or(Some(0));
                self.import_msg = Some(format!("added {name} — s saves"));
            }
            Err(e) => self.import_msg = Some(format!("couldn't import: {}", crate::tui::brief(e))),
        }
    }

    // IO delegated from the App.
    pub fn idle_ref(&self) -> Option<&Hypridle> {
        self.idle.as_ref()
    }
    pub fn lock_ref(&self) -> Option<&Hyprlock> {
        self.lock.as_ref()
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        if let Some(msg) = &self.error {
            f.render_widget(Paragraph::new(msg.clone()).wrap(Wrap { trim: false }), area);
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(area);

        let mut items: Vec<ListItem> = Vec::new();
        let mut i = 0usize;
        items.push(ListItem::new(Line::from(Span::styled(
            "  Idle timeline",
            skin.dim(),
        ))));
        for (ti, l) in self.timeline.iter().enumerate() {
            items.push(self.row(
                i,
                &format!("{:<22}", l.stage.label()),
                &fmt_dur(l.timeout),
                skin,
            ));
            let _ = ti;
            i += 1;
        }
        if let Some(lock) = self.lock.as_ref() {
            items.push(ListItem::new(Line::from(Span::styled(
                "  Lock screen",
                skin.dim(),
            ))));
            let some = |o: Option<String>| o.unwrap_or_else(|| "—".into());
            items.push(self.row(i, "Background blur", &some(lock.blur_passes()), skin));
            i += 1;
            items.push(self.row(i, "Dim", &some(lock.dim()), skin));
            i += 1;
            items.push(self.row(i, "Avatar size", &some(lock.avatar_size()), skin));
            i += 1;
            let avatar = lock.avatar_path().unwrap_or_else(|| "—".into());
            let short = avatar.rsplit('/').next().unwrap_or(&avatar).to_string();
            items.push(self.row(i, "Avatar (Enter to pick)", &short, skin));
        }
        f.render_widget(List::new(items), rows[0]);

        let dirty = if self.dirty {
            Span::styled("  ·  unsaved — s to save", skin.warn())
        } else {
            Span::styled("", skin.dim())
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![Span::styled("Lock & Idle", skin.dim()), dirty]),
                Line::from(Span::styled(
                    "←→ adjust   Enter pick avatar   s save",
                    skin.dim(),
                )),
            ]),
            rows[1],
        );

        if let Some(sel) = self.picker {
            self.render_picker(f, area, skin, sel);
        }
    }

    fn row(&self, i: usize, label: &str, value: &str, skin: &Skin) -> ListItem<'static> {
        let selected = i == self.selected;
        let val_style = if selected {
            skin.accent_bold()
        } else {
            skin.body()
        };
        let label_style = if selected {
            skin.selection()
        } else {
            skin.body()
        };
        ListItem::new(Line::from(vec![
            Span::styled(if selected { "  ▸ " } else { "    " }, skin.accent_bold()),
            Span::styled(format!("{label:<24}"), label_style),
            Span::styled(format!("{value:>16}"), val_style),
        ]))
    }

    fn render_picker(&self, f: &mut Frame, area: Rect, skin: &Skin, sel: usize) {
        // Room for the list, plus the import field and any message.
        let rows = self.avatars.len().max(1) as u16 + 4;
        let rect = crate::tui::ui::centered_rect(area, 64, rows);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Avatar — ⏎ pick · i add an image · esc ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        let mut lines: Vec<Line> = Vec::new();
        if self.avatars.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No images yet — press i to add one",
                skin.dim(),
            )));
        }
        for (i, p) in self.avatars.iter().enumerate() {
            let on = i == sel;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(if on { "▸ " } else { "  " }, skin.accent_bold()),
                Span::styled(name, if on { skin.selection() } else { skin.body() }),
            ]));
        }

        lines.push(Line::from(""));
        match &self.import {
            Some(buf) => lines.push(Line::from(vec![
                Span::styled("  image ", skin.accent_bold()),
                Span::styled(buf.clone(), skin.body()),
                Span::styled("▏", skin.accent_bold()),
            ])),
            None => {
                if let Some(msg) = &self.import_msg {
                    let style = if msg.starts_with("couldn't") {
                        skin.error()
                    } else {
                        skin.ok()
                    };
                    lines.push(Line::from(Span::styled(format!("  {msg}"), style)));
                } else {
                    lines.push(Line::from(Span::styled(
                        "  i — add a PNG, JPEG or WebP from anywhere",
                        skin.dim(),
                    )));
                }
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn fmt_dur(secs: i64) -> String {
    if secs >= 60 && secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

//! Wallpaper browser screen (spec 04 §5, roadmap 0.6.1).
//!
//! The four merged sources as one list with kind/source badges and a ● on
//! what's showing now. Enter applies through Omarchy's script; video entries
//! are gated on the `video_wallpapers` capability with the guided-install
//! line shown inline instead of failing silently (never strand the user).

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use studio_core::modules::wallpapers::{Entry, Kind, Wallpapers};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::imagecell::{ImageCell, Preview};
use crate::tui::theme::Skin;

pub enum WallpaperAction {
    None,
    /// Apply this file (App shells out + reloads).
    Set(PathBuf),
    /// Cycle to Omarchy's next background.
    Next,
    /// Copy this file into the user backgrounds dir.
    Add(PathBuf),
    /// Delete this user-added entry.
    Remove(Entry),
    /// Open in an external viewer (imv, mpv for videos).
    Open(Entry),
    /// Browse wallhaven.cc (opens the browser modal).
    Wallhaven,
    /// Craft a theme from this image (opens the wizard).
    Craft(Entry),
    /// Run a companion tool's contextual action (spec 06 §5).
    Tool {
        exec: String,
        label: String,
        terminal: bool,
    },
}

pub struct WallpapersScreen {
    w: Wallpapers,
    /// mpvpaper present — video entries applicable.
    videos_ok: bool,
    selected: usize,
    /// Add-wallpaper path prompt, when open.
    input: Option<String>,
    /// Contextual actions from present companion tools (label, exec,
    /// needs-terminal) — e.g. "Open in Aether" when installed (spec 06 §5).
    pub context_tools: Vec<(String, String, bool)>,
}

impl WallpapersScreen {
    pub fn load(paths: &OmarchyPaths, videos_ok: bool) -> Self {
        Self {
            w: Wallpapers::load(paths),
            videos_ok,
            selected: 0,
            input: None,
            context_tools: Vec::new(),
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths) {
        let keep = self.selected;
        self.w = Wallpapers::load(paths);
        self.selected = keep.min(self.w.entries.len().saturating_sub(1));
    }

    pub fn is_modal(&self) -> bool {
        self.input.is_some()
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.w.entries.get(self.selected)
    }

    /// The selected entry is a video we can't play (capability gate).
    fn gated(&self) -> bool {
        self.selected_entry()
            .is_some_and(|e| e.kind == Kind::Video && !self.videos_ok)
    }

    pub fn handle(&mut self, key: KeyEvent) -> WallpaperAction {
        if let Some(buf) = self.input.as_mut() {
            match key.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    let path = std::mem::take(buf);
                    self.input = None;
                    if !path.trim().is_empty() {
                        let expanded = shellexpand_home(path.trim());
                        return WallpaperAction::Add(expanded);
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return WallpaperAction::None;
        }

        let n = self.w.entries.len();
        if crate::tui::ui::list_nav(key.code, &mut self.selected, n) {
            return WallpaperAction::None;
        }
        match key.code {
            KeyCode::Enter if n > 0 && !self.gated() => {
                if let Some(e) = self.selected_entry() {
                    return WallpaperAction::Set(e.path.clone());
                }
            }
            KeyCode::Char('n') => return WallpaperAction::Next,
            KeyCode::Char('o') => {
                if let Some(e) = self.selected_entry() {
                    return WallpaperAction::Open(e.clone());
                }
            }
            KeyCode::Char('t') => {
                if let Some(e) = self.selected_entry() {
                    return WallpaperAction::Craft(e.clone());
                }
            }
            KeyCode::Char('A') => {
                if let Some((label, exec, terminal)) = self.context_tools.first() {
                    return WallpaperAction::Tool {
                        exec: exec.clone(),
                        label: label.clone(),
                        terminal: *terminal,
                    };
                }
            }
            KeyCode::Char('a') => self.input = Some(String::new()),
            KeyCode::Char('w') => return WallpaperAction::Wallhaven,
            KeyCode::Char('x') => {
                if let Some(e) = self.selected_entry() {
                    if e.removable() {
                        return WallpaperAction::Remove(e.clone());
                    }
                }
            }
            _ => {}
        }
        WallpaperAction::None
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin, images: &mut ImageCell) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(area);

        // Wide terminals get a live preview pane beside the list.
        let (list_area, preview_area) = if area.width >= 80 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(58), Constraint::Min(20)])
                .split(rows[0]);
            (cols[0], Some(cols[1]))
        } else {
            (rows[0], None)
        };

        if self.w.entries.is_empty() {
            f.render_widget(
                Paragraph::new(format!(
                    "No wallpapers found for `{}`.\nAdd one with a (or `omarchy-studio wallpaper add <file>`).",
                    self.w.theme
                )),
                list_area,
            );
        } else {
            let items: Vec<ListItem> = self
                .w
                .entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let on = i == self.selected;
                    let current = self.w.is_current(e);
                    let gated = e.kind == Kind::Video && !self.videos_ok;
                    let dot = if current { "● " } else { "  " };
                    let name_style = if on {
                        skin.accent_bold()
                    } else if gated {
                        skin.dim()
                    } else {
                        skin.body()
                    };
                    let kind_style = match e.kind {
                        Kind::Video if gated => skin.warn(),
                        Kind::Video | Kind::Gif => skin.accent_bold(),
                        Kind::Image => skin.dim(),
                    };
                    let row = Line::from(vec![
                        Span::styled(if on { "▸ " } else { "  " }, skin.accent_bold()),
                        Span::styled(dot, skin.ok()),
                        Span::styled(format!("{:<38}", e.name()), name_style),
                        Span::styled(format!("{:<7}", e.kind.label()), kind_style),
                        Span::styled(e.source.label(), skin.dim()),
                    ]);
                    let item = ListItem::new(row);
                    if on {
                        item.style(ratatui::style::Style::default().bg(skin.sel_bg))
                    } else {
                        item
                    }
                })
                .collect();
            f.render_widget(List::new(items), list_area);
        }

        if let Some(pane) = preview_area {
            self.render_preview(f, pane, skin, images);
        }

        let mut footer = vec![Line::from(vec![Span::styled(
            format!("{} wallpapers · {}", self.w.entries.len(), self.w.theme),
            skin.dim(),
        )])];
        if self.gated() {
            footer.push(Line::from(Span::styled(
                "video wallpapers need mpvpaper — see `omarchy-studio doctor --deps` for the install",
                skin.warn(),
            )));
        } else {
            let mut hint = String::from(
                "enter set · n next · o open · t theme from this · a add · w wallhaven · x remove",
            );
            if let Some((label, ..)) = self.context_tools.first() {
                let short = label.split(" (").next().unwrap_or(label);
                hint.push_str(&format!(" · A {short}"));
            }
            footer.push(Line::from(Span::styled(hint, skin.dim())));
        }
        f.render_widget(Paragraph::new(footer), rows[1]);

        if let Some(buf) = &self.input {
            self.render_input(f, area, skin, buf);
        }
    }

    /// Right-hand pane: the selected wallpaper, live, in whatever the
    /// terminal can do (kitty/sixel/half-blocks) — text otherwise, with the
    /// `o` escape hatch always spelled out (spec 06 §4: never strand).
    fn render_preview(&self, f: &mut Frame, area: Rect, skin: &Skin, images: &mut ImageCell) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(skin.border())
            .title(Span::styled(" preview ", skin.dim()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(e) = self.selected_entry() else {
            return;
        };
        let fallback = |f: &mut Frame, text: String| {
            f.render_widget(
                Paragraph::new(text)
                    .style(skin.dim())
                    .wrap(ratatui::widgets::Wrap { trim: true }),
                inner,
            );
        };
        match e.kind {
            Kind::Video => fallback(
                f,
                format!("{}\n\nvideo — press o to play it in mpv", e.name()),
            ),
            _ => match images.render(f, inner, &e.path) {
                Preview::Drawn => {}
                Preview::Decoding => fallback(f, format!("{}\n\nloading preview…", e.name())),
                Preview::Failed => fallback(
                    f,
                    format!(
                        "{}\n\ncould not decode — press o to open it in imv",
                        e.name()
                    ),
                ),
            },
        }
    }

    fn render_input(&self, f: &mut Frame, area: Rect, skin: &Skin, buf: &str) {
        let w = 64u16.min(area.width.saturating_sub(2));
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(w)) / 2,
            area.y + area.height / 2 - 1,
            w,
            3,
        );
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(skin.accent_bold())
            .title(" Add wallpaper — path to an image/video, Esc cancels ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("❯ ", skin.accent_bold()),
                Span::styled(buf.to_string(), skin.body()),
                Span::styled("▏", skin.accent_bold()),
            ])),
            inner,
        );
    }
}

/// `~/...` → `$HOME/...` for the add prompt.
fn shellexpand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

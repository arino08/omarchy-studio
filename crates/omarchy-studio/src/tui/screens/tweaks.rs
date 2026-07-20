//! Quick tweaks screen (roadmap 0.8.5).
//!
//! A checklist of one-key, individually-reversible tweaks. Space toggles the
//! selected one — the write happens immediately (each tweak owns a managed
//! block or a directory), and the App reloads Hyprland when the change touches
//! it. Every tweak reports its own live state, so the boxes always reflect disk.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use studio_core::modules::tweaks::{self, Ctx, State, Tweak};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

pub enum TweaksAction {
    None,
    /// The user asked to flip a tweak. The App performs it — screens report
    /// intent, the App owns side effects (and the snapshot store the apply
    /// pipeline needs).
    Toggle {
        index: usize,
        on: bool,
    },
}

pub struct TweaksScreen {
    ctx: Ctx,
    items: Vec<Box<dyn Tweak>>,
    states: Vec<State>,
    cursor: usize,
    error: Option<String>,
}

impl TweaksScreen {
    pub fn load(_paths: &OmarchyPaths) -> Self {
        // Tweaks resolve their own paths (and injected HOME) via Ctx.
        match Ctx::discover() {
            Ok(ctx) => {
                let items = tweaks::catalog();
                let states = items.iter().map(|t| t.state(&ctx)).collect();
                Self {
                    ctx,
                    items,
                    states,
                    cursor: 0,
                    error: None,
                }
            }
            Err(e) => Self {
                ctx: Ctx {
                    paths: OmarchyPaths {
                        system: Default::default(),
                        config: Default::default(),
                        state: Default::default(),
                    },
                    home: Default::default(),
                },
                items: Vec::new(),
                states: Vec::new(),
                cursor: 0,
                error: Some(format!("{e:?}")),
            },
        }
    }

    /// Re-read every tweak's state from disk (after an apply).
    fn refresh(&mut self) {
        self.states = self.items.iter().map(|t| t.state(&self.ctx)).collect();
    }

    pub fn hint(&self) -> &'static str {
        "↑↓ move · Space toggle · esc back"
    }

    pub fn handle(&mut self, key: KeyEvent) -> TweaksAction {
        let n = self.items.len();
        if crate::tui::ui::list_nav(key.code, &mut self.cursor, n) {
            return TweaksAction::None;
        }
        if key.code == KeyCode::Char(' ') && n > 0 {
            return self.toggle();
        }
        TweaksAction::None
    }

    fn toggle(&mut self) -> TweaksAction {
        let i = self.cursor;
        TweaksAction::Toggle {
            index: i,
            on: self.states[i] != State::On,
        }
    }

    /// Apply one tweak on the App's behalf, through the pipeline.
    pub fn apply(
        &mut self,
        index: usize,
        on: bool,
        store: &studio_core::snapshot::SnapshotStore,
        runner: &dyn studio_core::cmd::CommandRunner,
    ) -> Result<String, String> {
        let Some(tweak) = self.items.get(index) else {
            return Err("no such tweak".into());
        };
        let label = tweak.label().to_string();
        let result =
            studio_core::modules::tweaks::apply(tweak.as_ref(), &self.ctx, on, store, runner);
        self.refresh();
        match result {
            Ok(_) => Ok(format!("{label} {}", if on { "on" } else { "off" })),
            Err(studio_core::StudioError::VerifyFailed {
                rolled_back: true, ..
            }) => Err(format!("{label} broke Hyprland's config — reverted")),
            Err(e) => Err(format!("{label}: {}", crate::brief(e))),
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        if let Some(msg) = &self.error {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(msg.clone(), skin.body()))),
                area,
            );
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(area);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let on = self.states[i] == State::On;
                let cursor = i == self.cursor;
                let box_ = if on { "[x]" } else { "[ ]" };
                let name_style = if cursor {
                    skin.selection()
                } else {
                    skin.body()
                };
                let marker = if cursor { "▸ " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{marker}{box_} "), skin.accent_bold()),
                    Span::styled(format!("{:<33}", t.label()), name_style),
                    Span::styled(format!("  {}", t.detail()), skin.dim()),
                ]))
            })
            .collect();
        f.render_widget(List::new(items), rows[0]);

        let footer = Paragraph::new(vec![
            Line::from(Span::styled(
                "Quick tweaks — each one is reversible",
                skin.dim(),
            )),
            Line::from(Span::styled(self.hint(), skin.dim())),
        ]);
        f.render_widget(footer, rows[1]);
    }
}

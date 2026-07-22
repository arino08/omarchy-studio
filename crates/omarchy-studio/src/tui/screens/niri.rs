//! Niri mode — the ScrollOverview plugin (spec: community ask).
//!
//! **ScrollOverview is by [yayuuu](https://github.com/yayuuu/hyprland-scroll-overview),
//! BSD-3-Clause.** Studio vendors nothing: this screen reports the plugin's
//! state, writes the config block and keybind its README documents, and shows
//! the `hyprpm` commands for the steps that need a password.
//!
//! Hyprland ships a native `scrolling` layout, but it isn't a comfortable niri
//! substitute yet, so "niri mode" here means *this plugin loaded*.
//!
//! Enabling and disabling deliberately are **not** run from the TUI: `hyprpm`
//! escalates for those (its cache lives under `/var/cache`), and a TUI can't
//! answer a sudo prompt. Showing the exact command beats a hang or an opaque
//! failure.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use studio_core::cmd::CommandRunner;
use studio_core::modules::scrolloverview::{self as sco, Settings, State};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

pub enum NiriAction {
    None,
    /// Persist the edited settings (the App owns the snapshot store).
    Save,
    /// Write the `source =` line so the settings actually load.
    Source,
    /// Install the toggle keybind on the given chord.
    Bind,
    /// Install (or remove) the scrolling-navigation keybinds.
    NavBinds(bool),
}

/// The adjustable rows, in display order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    /// The window layout itself — the actual "niri mode vs hyprland mode".
    Mode,
    Scale,
    Layout,
    Gap,
    Blur,
    Gesture,
}

impl Row {
    const ALL: [Row; 6] = [
        Row::Mode,
        Row::Scale,
        Row::Layout,
        Row::Gap,
        Row::Blur,
        Row::Gesture,
    ];

    fn label(self) -> &'static str {
        match self {
            Row::Mode => "Window layout",
            Row::Scale => "Overview scale",
            Row::Layout => "Overview direction",
            Row::Gap => "Gap between workspaces",
            Row::Blur => "Blur the backdrop",
            Row::Gesture => "Swipe distance",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Row::Mode => "Niri opens windows as columns you scroll through",
            Row::Scale => "How small the workspace cards are drawn",
            Row::Layout => "Which way workspaces stack in the overview",
            Row::Gap => "Space between cards, in pixels",
            Row::Blur => "Blur behind the overview",
            Row::Gesture => "How far to swipe on the trackpad to open it",
        }
    }
}

pub struct NiriScreen {
    state: State,
    /// `general.layout` from the config — dwindle/master is "hyprland mode",
    /// scrolling is "niri mode".
    mode: String,
    settings: Settings,
    sourced: bool,
    /// The chord bound to the overview, if Studio installed one.
    bind: Option<String>,
    /// Are the scroll-navigation binds in place?
    nav: bool,
    selected: usize,
    pub dirty: bool,
}

impl NiriScreen {
    pub fn load(paths: &OmarchyPaths, runner: &dyn CommandRunner) -> Self {
        Self {
            state: sco::state(runner),
            mode: studio_core::modules::looknfeel::LookFeel::load(paths).value("general.layout"),
            settings: Settings::load(paths),
            sourced: sco::is_sourced(paths),
            bind: current_bind(paths),
            nav: sco::nav_binds_installed(paths),
            selected: 0,
            dirty: false,
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths, runner: &dyn CommandRunner) {
        let keep = self.selected;
        *self = Self::load(paths, runner);
        self.selected = crate::tui::ui::clamp_index(keep, Row::ALL.len());
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The chosen `general.layout` value, for the App to persist.
    pub fn mode(&self) -> &str {
        &self.mode
    }

    pub fn hint(&self) -> String {
        let mut parts = vec!["←→ adjust"];
        if self.dirty {
            parts.push("s save");
        }
        if !self.sourced {
            parts.push("o source it");
        }
        if self.bind.is_none() {
            parts.push("b bind");
        }
        parts.push(if self.nav {
            "n unbind arrows"
        } else {
            "n fix arrows"
        });
        parts.join(" · ")
    }

    pub fn handle(&mut self, key: KeyEvent) -> NiriAction {
        if crate::tui::ui::list_nav(key.code, &mut self.selected, Row::ALL.len()) {
            return NiriAction::None;
        }
        match key.code {
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => self.nudge(1),
            KeyCode::Left | KeyCode::Char('-') => self.nudge(-1),
            KeyCode::Char('s') if self.dirty => return NiriAction::Save,
            KeyCode::Char('o') if !self.sourced => return NiriAction::Source,
            KeyCode::Char('b') => return NiriAction::Bind,
            KeyCode::Char('n') => return NiriAction::NavBinds(!self.nav),
            _ => {}
        }
        NiriAction::None
    }

    /// Adjust the selected setting, clamped to the plugin's documented ranges
    /// so an out-of-range value never reaches the config.
    fn nudge(&mut self, dir: i64) {
        if Row::ALL[self.selected] == Row::Mode {
            // Only two modes matter here; master is reachable from Look & Feel.
            self.mode = if self.mode == "scrolling" {
                "dwindle".into()
            } else {
                "scrolling".into()
            };
            self.dirty = true;
            return;
        }
        let s = &mut self.settings;
        match Row::ALL[self.selected] {
            Row::Mode => unreachable!("handled above"),
            Row::Scale => {
                let next = (s.scale + dir as f64 * 0.05).clamp(0.1, 0.9);
                // Float steps drift; round to the 2dp the file stores.
                s.scale = (next * 100.0).round() / 100.0;
            }
            Row::Layout => {
                s.layout = if s.layout == "vertical" {
                    "horizontal".into()
                } else {
                    "vertical".into()
                };
            }
            Row::Gap => s.workspace_gap = (s.workspace_gap + dir * 10).clamp(0, 1000),
            Row::Blur => s.blur = !s.blur,
            Row::Gesture => s.gesture_distance = (s.gesture_distance + dir * 25).clamp(50, 2000),
        }
        self.dirty = true;
    }

    fn value(&self, row: Row) -> String {
        let s = &self.settings;
        match row {
            Row::Mode => {
                if self.mode == "scrolling" {
                    "Niri".into()
                } else {
                    "Hyprland".into()
                }
            }
            Row::Scale => format!("{:.2}", s.scale),
            Row::Layout => s.layout.clone(),
            Row::Gap => format!("{} px", s.workspace_gap),
            Row::Blur => if s.blur { "on" } else { "off" }.into(),
            Row::Gesture => format!("{} px", s.gesture_distance),
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let mut lines: Vec<Line> = Vec::new();

        // ── status: the one thing someone opens this screen to see
        let (mark, style) = if self.state.is_on() {
            ("●", skin.ok())
        } else {
            ("○", skin.warn())
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {mark} "), style),
            Span::styled("Niri mode  ", skin.body()),
            Span::styled(self.state.label(), style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("     ScrollOverview by {} · {}", sco::AUTHOR, sco::LICENSE),
            skin.dim(),
        )));
        lines.push(Line::from(Span::styled(
            format!("     {}", sco::REPO),
            skin.dim(),
        )));
        lines.push(Line::from(""));

        // ── whatever the user has to do next, in their own words
        for note in self.notes() {
            lines.push(Line::from(Span::styled(format!("  {note}"), skin.warn())));
        }
        if !self.notes().is_empty() {
            lines.push(Line::from(""));
        }

        lines.push(Line::from(Span::styled("  Overview", skin.dim())));
        for (i, &row) in Row::ALL.iter().enumerate() {
            let on = i == self.selected;
            let style = if on { skin.selection() } else { skin.body() };
            lines.push(Line::from(vec![
                Span::styled(if on { "  ▸ " } else { "    " }, skin.accent_bold()),
                Span::styled(format!("{:<26}", row.label()), style),
                Span::styled(format!("{:>10}", self.value(row)), skin.accent_bold()),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("     {}", Row::ALL[self.selected].detail()),
            skin.dim(),
        )));

        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    /// The next actions worth surfacing, in the order they matter.
    fn notes(&self) -> Vec<String> {
        let mut v = Vec::new();
        match self.state {
            State::NoHyprpm => {
                v.push("hyprpm isn't installed — it ships with Hyprland".into());
                return v;
            }
            // Both of these escalate, so they can't run from here.
            State::NotAdded => v.push(format!("not installed — run:  hyprpm add {}", sco::REPO)),
            State::Disabled => v.push(format!(
                "installed but off — run:  hyprpm enable {}",
                sco::PLUGIN
            )),
            State::Enabled => {}
        }
        if !self.sourced {
            v.push("your settings aren't sourced yet — press o".into());
        }
        // The single most confusing thing about the scrolling layout: the
        // arrow keys Omarchy binds do nothing in it.
        if self.mode == "scrolling" && !self.nav {
            v.push("SUPER+←/→ won't scroll until you press n".into());
        }
        match &self.bind {
            Some(chord) => v.push(format!("{chord} toggles the overview")),
            None => v.push("no keybind yet — press b to bind one".into()),
        }
        v
    }
}

/// The chord Studio bound to the overview, read back from the override block.
fn current_bind(paths: &OmarchyPaths) -> Option<String> {
    use studio_core::modules::keybinds::{read_overrides, render_chord, Override};
    read_overrides(paths).into_iter().find_map(|o| match o {
        Override::Set(cb) if cb.dispatcher.starts_with(sco::PLUGIN) => {
            Some(render_chord(cb.modmask, &cb.key))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn screen() -> NiriScreen {
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-niriscreen-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".config/hypr")).unwrap();
        let _ = dir;
        NiriScreen {
            state: State::Enabled,
            mode: "dwindle".into(),
            nav: true,
            settings: Settings::default(),
            sourced: true,
            bind: Some("SUPER+GRAVE".into()),
            selected: 0,
            dirty: false,
        }
    }

    #[test]
    fn adjusting_scale_clamps_to_the_plugins_range() {
        let mut s = screen();
        s.selected = 1; // Overview scale
        for _ in 0..50 {
            s.handle(key(KeyCode::Right));
        }
        assert_eq!(s.settings.scale, 0.9, "0.9 is the documented maximum");
        for _ in 0..50 {
            s.handle(key(KeyCode::Left));
        }
        assert_eq!(s.settings.scale, 0.1, "0.1 is the documented minimum");
    }

    #[test]
    fn scale_steps_stay_on_two_decimals() {
        // Float accumulation would otherwise write 0.6000000000000001.
        let mut s = screen();
        s.selected = 1; // Overview scale
        s.handle(key(KeyCode::Right));
        s.handle(key(KeyCode::Right));
        assert_eq!(s.settings.scale, 0.6);
    }

    #[test]
    fn layout_and_blur_toggle_rather_than_count() {
        let mut s = screen();
        s.selected = 2; // Direction
        s.handle(key(KeyCode::Right));
        assert_eq!(s.settings.layout, "horizontal");
        s.handle(key(KeyCode::Right));
        assert_eq!(s.settings.layout, "vertical");

        s.selected = 4; // Blur
        assert!(!s.settings.blur);
        s.handle(key(KeyCode::Left));
        assert!(s.settings.blur, "either direction flips a toggle");
    }

    #[test]
    fn saving_is_only_offered_once_something_changed() {
        let mut s = screen();
        assert!(matches!(
            s.handle(key(KeyCode::Char('s'))),
            NiriAction::None
        ));
        s.handle(key(KeyCode::Right));
        assert!(s.dirty);
        assert!(matches!(
            s.handle(key(KeyCode::Char('s'))),
            NiriAction::Save
        ));
    }

    #[test]
    fn sourcing_is_only_offered_when_it_is_missing() {
        let mut s = screen();
        assert!(matches!(
            s.handle(key(KeyCode::Char('o'))),
            NiriAction::None
        ));
        s.sourced = false;
        assert!(matches!(
            s.handle(key(KeyCode::Char('o'))),
            NiriAction::Source
        ));
    }

    #[test]
    fn notes_name_the_command_for_steps_that_need_a_password() {
        // hyprpm escalates, so the screen must show the command rather than
        // pretend it can run it.
        let mut s = screen();
        s.state = State::NotAdded;
        assert!(
            s.notes().iter().any(|n| n.contains("hyprpm add")),
            "{:?}",
            s.notes()
        );
        s.state = State::Disabled;
        assert!(
            s.notes().iter().any(|n| n.contains("hyprpm enable")),
            "{:?}",
            s.notes()
        );
    }

    #[test]
    fn a_bound_chord_is_reported_and_an_unbound_one_prompts() {
        let mut s = screen();
        assert!(s.notes().iter().any(|n| n.contains("SUPER+GRAVE")));
        s.bind = None;
        assert!(s.notes().iter().any(|n| n.contains("press b")));
    }
}

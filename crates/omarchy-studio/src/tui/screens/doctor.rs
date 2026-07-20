//! Doctor screen (spec 07, roadmap 0.5.6): the health view. System facts,
//! capability probes, and the update-survival picture (hooks + drift) in one
//! place, with install/remove for the lifecycle hooks a keypress away.
//!
//! Read-only except the two hook actions, which the App performs (IO stays
//! out of screens); `reload` re-probes everything.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use studio_core::cmd::CommandRunner;
use studio_core::hooks::{self, HookState};
use studio_core::omarchy::{Capabilities, OmarchyPaths};
use studio_core::snapshot::SnapshotStore;

use crate::tui::theme::Skin;

pub enum DoctorAction {
    None,
    /// Install (or repair) both lifecycle hooks.
    InstallHooks,
    /// Remove both lifecycle hooks.
    RemoveHooks,
    /// Re-run every probe.
    Refresh,
}

pub struct DoctorScreen {
    caps: Capabilities,
    theme: String,
    system_path: String,
    hooks: Vec<(&'static str, HookState)>,
    drift: Vec<PathBuf>,
    clobbers: Vec<PathBuf>,
    store_ok: bool,
    /// TUI-side probe result (kitty/sixel/half-blocks) — set by the App,
    /// which owns the [`crate::tui::imagecell::ImageCell`].
    graphics: Option<String>,
    /// What the active theme does and doesn't cover.
    coverage: Vec<studio_core::modules::coverage::Row>,
}

impl DoctorScreen {
    pub fn load(paths: &OmarchyPaths, runner: &dyn CommandRunner) -> Self {
        let caps = Capabilities::probe(paths, runner);
        let theme = paths
            .current_theme_name()
            .unwrap_or_else(|_| "unknown".into());
        let hooks = hooks::status(paths);
        let (drift, clobbers, store_ok) = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(studio_core::cmd::RealRunner),
        ) {
            Ok(store) => {
                let r = hooks::update_report(&store);
                (r.drift, r.clobbers, true)
            }
            Err(_) => (Vec::new(), Vec::new(), false),
        };
        // Read the live theme dir rather than resolve the slug: that's what
        // Omarchy's symlinks actually point at.
        let coverage = studio_core::modules::coverage::report(paths, &paths.current_theme_dir());
        Self {
            caps,
            theme,
            system_path: paths.system.display().to_string(),
            hooks,
            drift,
            clobbers,
            store_ok,
            graphics: None,
            coverage,
        }
    }

    pub fn reload(&mut self, paths: &OmarchyPaths, runner: &dyn CommandRunner) {
        let graphics = self.graphics.take();
        *self = Self::load(paths, runner);
        self.graphics = graphics;
    }

    pub fn set_graphics(&mut self, label: &str) {
        self.graphics = Some(label.to_string());
    }

    pub fn handle(&mut self, key: KeyEvent) -> DoctorAction {
        match key.code {
            KeyCode::Char('i') => DoctorAction::InstallHooks,
            KeyCode::Char('x') if self.hooks.iter().any(|(_, s)| *s != HookState::Missing) => {
                DoctorAction::RemoveHooks
            }
            KeyCode::Char('r') => DoctorAction::Refresh,
            _ => DoctorAction::None,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let good = |b: bool| if b { "✓" } else { "✗" };
        let style_of = |b: bool, skin: &Skin| if b { skin.ok() } else { skin.error() };
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(Span::styled("  System", skin.dim())));
        let fact = |label: &str, value: String, skin: &Skin| {
            Line::from(vec![
                Span::styled(format!("    {label:<14}"), skin.dim()),
                Span::styled(value, skin.body()),
            ])
        };
        lines.push(fact(
            "omarchy",
            format!("{} at {}", self.caps.omarchy_version, self.system_path),
            skin,
        ));
        // An Omarchy Studio hasn't been tested against is worth saying out
        // loud here — this is the screen people open when something looks
        // wrong. It's a warning, never a refusal (see `version_fit`), so it
        // reads as context rather than an error.
        if let Some(warning) = studio_core::version_fit(&self.caps.omarchy_version).warning() {
            // Wrap by hand: Paragraph's own wrap restarts continuation lines at
            // column zero, which breaks them out of this indented fact list.
            let width = area.width.saturating_sub(8).max(20) as usize;
            for (i, line) in wrap_words(&warning, width).into_iter().enumerate() {
                let lead = if i == 0 { "    ! " } else { "      " };
                lines.push(Line::from(vec![
                    Span::styled(lead, skin.warn()),
                    Span::styled(line, skin.warn()),
                ]));
            }
        }
        let hypr = if self.caps.hyprland_version.is_empty() {
            "unavailable (not in a Hyprland session?)".into()
        } else {
            self.caps.hyprland_version.clone()
        };
        lines.push(fact("hyprland", hypr, skin));
        lines.push(fact("theme", self.theme.clone(), skin));
        // A theme that doesn't ship a non-templated file leaves Omarchy's
        // symlink dangling, and the app fails with an error that never
        // mentions themes (nvim is the loud one). Most community themes are
        // colours-only, so name the casualties here rather than let someone
        // debug it from the app's side.
        for row in self.coverage.iter().filter(|r| r.is_breakage()) {
            lines.push(Line::from(vec![
                Span::styled("    ! ", skin.warn()),
                Span::styled(format!("{:<14} ", row.app), skin.body()),
                Span::styled(row.consequence.to_string(), skin.dim()),
            ]));
        }
        if let Some(g) = &self.graphics {
            lines.push(fact("previews", g.clone(), skin));
        }
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled("  Capabilities", skin.dim())));
        let caps = [
            (
                self.caps.has_configerrors,
                "config verification (hyprctl configerrors)",
            ),
            (self.caps.has_templates, "theme template engine"),
            (self.caps.has_hooks, "lifecycle hooks (omarchy-hook)"),
            (self.caps.tui_float_rule, "stock floating-TUI window rule"),
            (self.caps.video_wallpapers, "video wallpapers (mpvpaper)"),
        ];
        for (ok, label) in caps {
            lines.push(Line::from(vec![
                Span::styled(format!("    {} ", good(ok)), style_of(ok, skin)),
                Span::styled(label, skin.body()),
            ]));
        }
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled("  Update survival", skin.dim())));
        for (event, state) in &self.hooks {
            let (mark, note, ok) = match state {
                HookState::Installed => ("✓", "installed".to_string(), true),
                HookState::Missing => ("✗", "not installed — i to install".to_string(), false),
                HookState::Modified => (
                    "✗",
                    "MODIFIED, not our script — i repairs".to_string(),
                    false,
                ),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("    {mark} "), style_of(ok, skin)),
                Span::styled(format!("{event:<14} hook  "), skin.body()),
                Span::styled(note, if ok { skin.dim() } else { skin.warn() }),
            ]));
        }
        if !self.store_ok {
            lines.push(Line::from(Span::styled(
                "    ✗ snapshot store unavailable — drift unknown",
                skin.error(),
            )));
        } else if self.drift.is_empty() && self.clobbers.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("    ✓ ", skin.ok()),
                Span::styled("tracked files match the last snapshot", skin.body()),
            ]));
        } else {
            for f in &self.drift {
                lines.push(Line::from(vec![
                    Span::styled("    ~ ", skin.warn()),
                    Span::styled(format!("{}", f.display()), skin.body()),
                    Span::styled("  hand-edited since our snapshot", skin.warn()),
                ]));
            }
            for f in &self.clobbers {
                lines.push(Line::from(vec![
                    Span::styled("    ! ", skin.error()),
                    Span::styled(format!("{}", f.display()), skin.body()),
                    Span::styled("  replaced by an update (.bak left)", skin.error()),
                ]));
            }
            lines.push(Line::from(Span::styled(
                "      review from Snapshots, or `omarchy-studio snapshot restore <id>`",
                skin.dim(),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  i install/repair hooks   x remove hooks   r re-run checks",
            skin.dim(),
        )));

        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }
}

/// Break `text` into lines of at most `width` columns, on word boundaries.
///
/// Only needed where a wrapped line has to keep an indent (see the version
/// warning): everywhere else `Paragraph`'s own wrap is the right tool.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_word_boundaries_within_the_width() {
        let lines = wrap_words("the quick brown fox jumps", 11);
        assert_eq!(lines, vec!["the quick", "brown fox", "jumps"]);
        assert!(lines.iter().all(|l| l.chars().count() <= 11));
    }

    #[test]
    fn a_word_longer_than_the_width_still_gets_its_own_line() {
        // Better to overflow one line than to drop the word entirely.
        assert_eq!(
            wrap_words("hi supercalifragilistic", 5),
            vec!["hi", "supercalifragilistic"]
        );
    }

    #[test]
    fn empty_text_produces_no_lines() {
        assert!(wrap_words("   ", 10).is_empty());
    }

    #[test]
    fn every_version_fit_warning_survives_wrapping_intact() {
        // The warning is the whole point of the row — wrapping must not lose
        // or reorder any of it.
        for v in ["4.0.0", "banana", ""] {
            let warning = studio_core::version_fit(v)
                .warning()
                .expect("untested version must warn");
            let wrapped = wrap_words(&warning, 40).join(" ");
            assert_eq!(
                wrapped,
                warning.split_whitespace().collect::<Vec<_>>().join(" ")
            );
        }
    }
}

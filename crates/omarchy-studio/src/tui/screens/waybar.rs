//! Waybar screen (spec 05 §4, roadmap 0.4.3).
//!
//! The status bar, laid out as its three lanes (left / center / right). Move a
//! module within its lane or across lanes, add one from the catalog, or remove
//! it — all edited through the comment-preserving JSONC editor. Save restarts
//! Waybar, snapshot-backed for undo. No JSON in sight.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use std::path::PathBuf;

use studio_core::cmd::CommandRunner;
use studio_core::modules::targets::{self, Source};
use studio_core::modules::waybar::{label_for, CustomSpec, WaybarConfig, LANES, MODULES};
use studio_core::omarchy::OmarchyPaths;

use crate::tui::theme::Skin;

pub enum WaybarAction {
    None,
    /// Save the config and restart Waybar (App snapshots first).
    Apply,
    /// Point Studio at a different config target (roadmap 0.8.2): `Some(path)`
    /// sets an override, `None` resets to the Omarchy default. App persists it.
    Retarget(Option<PathBuf>),
}

/// A candidate config.jsonc the target picker can switch to.
struct TargetPick {
    path: PathBuf,
    source: Source,
    detail: String,
}

/// The custom-module wizard's text fields (roadmap 0.4.5).
const WIZARD_LABELS: [&str; 5] = [
    "Name",
    "Command to run",
    "Refresh (seconds, blank = once)",
    "Display format",
    "On click (optional)",
];

/// A little form for scaffolding a `custom/*` module in plain language.
struct Wizard {
    fields: [String; 5],
    focus: usize,
    /// Lane the new module lands in (index into LANES).
    lane: usize,
    error: Option<String>,
}

impl Wizard {
    fn new(lane: usize) -> Self {
        let mut fields: [String; 5] = Default::default();
        fields[3] = "{}".to_string(); // sensible default format
        Self {
            fields,
            focus: 0,
            lane,
            error: None,
        }
    }

    fn spec(&self) -> CustomSpec {
        let interval = self.fields[2].trim().parse::<u32>().ok();
        let on_click = {
            let s = self.fields[4].trim();
            (!s.is_empty()).then(|| s.to_string())
        };
        let format = {
            let f = self.fields[3].trim();
            if f.is_empty() {
                "{}".into()
            } else {
                f.to_string()
            }
        };
        CustomSpec {
            name: self.fields[0].trim().to_string(),
            exec: self.fields[1].trim().to_string(),
            interval,
            format,
            on_click,
        }
    }
}

pub struct WaybarScreen {
    cfg: Option<WaybarConfig>,
    error: Option<String>,
    /// Flat cursor over (lane index, module index) pairs.
    cursor: usize,
    dirty: bool,
    /// Catalog picker overlay (add), Some(selected catalog index) while open.
    picker: Option<usize>,
    /// Custom-module wizard overlay, open when adding a `custom/*` module.
    wizard: Option<Wizard>,
    /// Which config.jsonc we're editing and why (roadmap 0.8.2).
    target_source: Source,
    /// Candidate config targets (override / detected / default) for the picker.
    candidates: Vec<TargetPick>,
    /// Target picker overlay, `Some(selected candidate index)` while open.
    target_picker: Option<usize>,
}

impl WaybarScreen {
    pub fn load(paths: &OmarchyPaths) -> Self {
        let (cfg, error) = match WaybarConfig::load(paths) {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(friendly(&e))),
        };
        let (target_source, candidates) = Self::resolve_targets(paths);
        Self {
            cfg,
            error,
            cursor: 0,
            dirty: false,
            picker: None,
            wizard: None,
            target_source,
            candidates,
            target_picker: None,
        }
    }

    /// The active config target's source plus every candidate the picker offers
    /// (override, running instances, Omarchy default) — deduped by path.
    fn resolve_targets(paths: &OmarchyPaths) -> (Source, Vec<TargetPick>) {
        let overrides =
            targets::Overrides::load(&studio_core::studio_config_dir().join("config.toml"));
        let instances = targets::waybar_instances();
        let default = targets::default_for("waybar.config", paths).unwrap_or_default();
        let resolved = targets::resolve_waybar_config(default.clone(), &overrides, &instances);

        let mut candidates: Vec<TargetPick> = Vec::new();
        if let Some(p) = overrides.get("waybar.config") {
            candidates.push(TargetPick {
                path: p,
                source: Source::Override,
                detail: "your saved override".into(),
            });
        }
        for w in &instances {
            if let Some(p) = &w.config {
                if !candidates.iter().any(|c| &c.path == p) {
                    candidates.push(TargetPick {
                        path: p.clone(),
                        source: Source::Detected,
                        detail: format!("running now · pid {}", w.pid),
                    });
                }
            }
        }
        if !candidates.iter().any(|c| c.path == default) {
            candidates.push(TargetPick {
                path: default,
                source: Source::Default,
                detail: "Omarchy default".into(),
            });
        }
        (resolved.source, candidates)
    }

    pub fn reload(&mut self, paths: &OmarchyPaths) {
        let keep = self.cursor;
        *self = Self::load(paths);
        self.cursor = keep;
    }

    pub fn is_modal(&self) -> bool {
        self.picker.is_some() || self.wizard.is_some() || self.target_picker.is_some()
    }

    /// Index of the currently-active target among the candidates.
    fn active_target(&self) -> usize {
        let current = self.cfg.as_ref().map(|c| c.path());
        self.candidates
            .iter()
            .position(|c| Some(c.path.as_path()) == current)
            .unwrap_or(0)
    }

    /// The (lane, module-index) pairs in display order.
    fn flat(&self) -> Vec<(usize, usize)> {
        let Some(cfg) = &self.cfg else { return vec![] };
        let mut v = Vec::new();
        for (li, (lane, _)) in LANES.iter().zip(cfg.lanes()).enumerate() {
            let _ = lane;
            let count = cfg.lane(LANES[li]).len();
            for mi in 0..count {
                v.push((li, mi));
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

    pub fn handle(&mut self, key: KeyEvent) -> WaybarAction {
        if self.wizard.is_some() {
            return self.handle_wizard(key);
        }
        if self.picker.is_some() {
            return self.handle_picker(key);
        }
        if self.target_picker.is_some() {
            return self.handle_target_picker(key);
        }
        if self.cfg.is_none() {
            // Even with no readable config, let the user retarget to a good one.
            if matches!(key.code, KeyCode::Char('f')) && self.candidates.len() > 1 {
                self.target_picker = Some(self.active_target());
            }
            return WaybarAction::None;
        }
        let n = self.flat().len();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            // Shift+↑/↓ reorders within a lane; ←/→ (or < >) moves between lanes.
            KeyCode::Down if shift => self.move_within(1),
            KeyCode::Up if shift => self.move_within(-1),
            KeyCode::Right | KeyCode::Char('>') => self.move_lane(1),
            KeyCode::Left | KeyCode::Char('<') => self.move_lane(-1),
            KeyCode::Down if n > 0 => self.cursor = (self.cursor + 1).min(n - 1),
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = n.saturating_sub(1),
            KeyCode::Char('a') => self.picker = Some(0),
            KeyCode::Char('d') | KeyCode::Char('x') => self.remove_selected(),
            KeyCode::Char('f') if self.candidates.len() > 1 => {
                self.target_picker = Some(self.active_target())
            }
            KeyCode::Char('s') if self.dirty => return WaybarAction::Apply,
            _ => {}
        }
        WaybarAction::None
    }

    fn handle_target_picker(&mut self, key: KeyEvent) -> WaybarAction {
        let Some(sel) = self.target_picker.as_mut() else {
            return WaybarAction::None;
        };
        match key.code {
            KeyCode::Esc => self.target_picker = None,
            KeyCode::Down => *sel = (*sel + 1).min(self.candidates.len().saturating_sub(1)),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Enter => {
                let pick = &self.candidates[*sel];
                // Choosing the default clears the override; anything else sets it.
                let action = match pick.source {
                    Source::Default => WaybarAction::Retarget(None),
                    _ => WaybarAction::Retarget(Some(pick.path.clone())),
                };
                self.target_picker = None;
                return action;
            }
            _ => {}
        }
        WaybarAction::None
    }

    fn selected(&self) -> Option<(usize, usize)> {
        self.flat().get(self.cursor).copied()
    }

    /// Move the selected module up/down within its own lane.
    fn move_within(&mut self, dir: i64) {
        let Some((li, mi)) = self.selected() else {
            return;
        };
        let lane = LANES[li];
        let mut ids = self.cfg.as_ref().unwrap().lane(lane);
        let target = mi as i64 + dir;
        if target < 0 || target as usize >= ids.len() {
            return;
        }
        ids.swap(mi, target as usize);
        if self.cfg.as_mut().unwrap().reorder(lane, &ids).is_ok() {
            self.dirty = true;
            // follow the moved item
            self.cursor = (self.cursor as i64 + dir).max(0) as usize;
            self.clamp_cursor();
        }
    }

    /// Move the selected module to the adjacent lane (append there).
    fn move_lane(&mut self, dir: i64) {
        let Some((li, mi)) = self.selected() else {
            return;
        };
        let target_lane = li as i64 + dir;
        if target_lane < 0 || target_lane as usize >= LANES.len() {
            return;
        }
        let from = LANES[li];
        let to = LANES[target_lane as usize];
        let id = self.cfg.as_ref().unwrap().lane(from)[mi].clone();
        let cfg = self.cfg.as_mut().unwrap();
        if cfg.remove(from, &id).is_ok() && cfg.add(to, &id).is_ok() {
            self.dirty = true;
        }
        self.clamp_cursor();
    }

    fn remove_selected(&mut self) {
        let Some((li, mi)) = self.selected() else {
            return;
        };
        let lane = LANES[li];
        let id = self.cfg.as_ref().unwrap().lane(lane)[mi].clone();
        if self.cfg.as_mut().unwrap().remove(lane, &id).is_ok() {
            self.dirty = true;
            self.clamp_cursor();
        }
    }

    fn handle_picker(&mut self, key: KeyEvent) -> WaybarAction {
        let Some(sel) = self.picker.as_mut() else {
            return WaybarAction::None;
        };
        match key.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Down => *sel = (*sel + 1).min(MODULES.len() - 1),
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Enter => {
                let module = MODULES[*sel];
                // add to the lane the cursor is currently in (default left)
                let li = self.selected().map(|(li, _)| li).unwrap_or(0);
                self.picker = None;
                // A custom/group catalog entry opens the wizard rather than
                // dropping a placeholder; a concrete module is added directly.
                if module.id.ends_with("/*") {
                    self.wizard = Some(Wizard::new(li));
                } else if self.cfg.as_mut().unwrap().add(LANES[li], module.id).is_ok() {
                    self.dirty = true;
                }
            }
            _ => {}
        }
        WaybarAction::None
    }

    fn handle_wizard(&mut self, key: KeyEvent) -> WaybarAction {
        let Some(wiz) = self.wizard.as_mut() else {
            return WaybarAction::None;
        };
        match key.code {
            KeyCode::Esc => self.wizard = None,
            KeyCode::Tab | KeyCode::Down => wiz.focus = (wiz.focus + 1) % WIZARD_LABELS.len(),
            KeyCode::BackTab | KeyCode::Up => {
                wiz.focus = (wiz.focus + WIZARD_LABELS.len() - 1) % WIZARD_LABELS.len()
            }
            KeyCode::Backspace => {
                wiz.fields[wiz.focus].pop();
            }
            KeyCode::Char(c) => wiz.fields[wiz.focus].push(c),
            KeyCode::Enter => {
                let spec = wiz.spec();
                if spec.name.is_empty() {
                    wiz.error = Some("give the module a name".into());
                    wiz.focus = 0;
                } else if spec.exec.is_empty() {
                    wiz.error = Some("what command should it run?".into());
                    wiz.focus = 1;
                } else {
                    let lane = LANES[wiz.lane];
                    match self.cfg.as_mut().unwrap().add_custom_module(&spec, lane) {
                        Ok(_) => {
                            self.dirty = true;
                            self.wizard = None;
                        }
                        Err(e) => self.wizard.as_mut().unwrap().error = Some(friendly(&e)),
                    }
                }
            }
            _ => {}
        }
        WaybarAction::None
    }

    // IO delegated from the App. Applies with the crash watchdog: if the edit
    // stops Waybar, the previous config is restored automatically.
    pub fn commit(
        &self,
        store: &studio_core::snapshot::SnapshotStore,
        runner: &dyn CommandRunner,
    ) -> studio_core::error::Result<studio_core::modules::waybar::ApplyOutcome> {
        use studio_core::modules::waybar::ApplyOutcome;
        match &self.cfg {
            // Give Waybar a moment to come up (or crash) before checking.
            Some(c) => c.apply_watched(store, runner, std::time::Duration::from_millis(900)),
            None => Ok(ApplyOutcome::Applied),
        }
    }

    pub fn config_path(&self) -> Option<std::path::PathBuf> {
        self.cfg.as_ref().map(|c| c.path().to_path_buf())
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        if let Some(msg) = &self.error {
            let mut lines = vec![
                Line::from(Span::styled(
                    "Can't read your Waybar config",
                    skin.accent_bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(msg.clone(), skin.body())),
            ];
            if self.candidates.len() > 1 {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Press f to point Studio at a different config file.",
                    skin.dim(),
                )));
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
            if let Some(sel) = self.target_picker {
                self.render_target_picker(f, area, skin, sel);
            }
            return;
        }
        let Some(cfg) = &self.cfg else { return };

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(area);
        self.render_target_header(f, rows[0], skin, cfg.path());

        let mut items: Vec<ListItem> = Vec::new();
        let mut flat_i = 0usize;
        for (li, lane) in LANES.iter().enumerate() {
            let title = match li {
                0 => "Left",
                1 => "Center",
                _ => "Right",
            };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {title}"),
                skin.dim(),
            ))));
            let ids = cfg.lane(lane);
            if ids.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "      (empty)",
                    skin.dim(),
                ))));
            }
            for id in ids {
                let selected = flat_i == self.cursor;
                let style = if selected {
                    skin.selection()
                } else {
                    skin.body()
                };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(if selected { "  ▸ " } else { "    " }, skin.accent_bold()),
                    Span::styled(format!("{:<24}", label_for(&id)), style),
                    Span::styled(id, skin.dim()),
                ])));
                flat_i += 1;
            }
        }
        f.render_widget(List::new(items), rows[1]);
        self.render_footer(f, rows[2], skin);

        if let Some(sel) = self.picker {
            self.render_picker(f, area, skin, sel);
        }
        if let Some(wiz) = &self.wizard {
            self.render_wizard(f, area, skin, wiz);
        }
        if let Some(sel) = self.target_picker {
            self.render_target_picker(f, area, skin, sel);
        }
    }

    /// One-line header naming the config.jsonc we're editing and why.
    fn render_target_header(&self, f: &mut Frame, area: Rect, skin: &Skin, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let source_style = match self.target_source {
            Source::Override => skin.accent_bold(),
            Source::Detected => skin.accent_bold(),
            Source::Default => skin.dim(),
        };
        let mut spans = vec![
            Span::styled("  Editing ", skin.dim()),
            Span::styled(name, skin.body()),
            Span::styled(" · ", skin.dim()),
            Span::styled(self.target_source.label(), source_style),
        ];
        if self.candidates.len() > 1 {
            spans.push(Span::styled("   f change", skin.dim()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_target_picker(&self, f: &mut Frame, area: Rect, skin: &Skin, sel: usize) {
        let rect = crate::tui::ui::centered_rect(area, 70, self.candidates.len() as u16 + 4);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Which config? — Enter to switch, Esc to cancel ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let items: Vec<ListItem> = self
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let on = i == sel;
                let marker = if on { "▸ " } else { "  " };
                let name_style = if on { skin.selection() } else { skin.body() };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{marker}{:<9}", c.source.label()),
                        skin.accent_bold(),
                    ),
                    Span::styled(c.path.display().to_string(), name_style),
                    Span::styled(format!("  ({})", c.detail), skin.dim()),
                ]))
            })
            .collect();
        f.render_widget(List::new(items), inner);
    }

    fn render_wizard(&self, f: &mut Frame, area: Rect, skin: &Skin, wiz: &Wizard) {
        let rect = crate::tui::ui::centered_rect(area, 60, 15);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(format!(" New custom module → {} ", LANES[wiz.lane]));
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        let mut lines: Vec<Line> = Vec::new();
        for (i, label) in WIZARD_LABELS.iter().enumerate() {
            let on = i == wiz.focus;
            let marker = if on { "▸ " } else { "  " };
            let val = &wiz.fields[i];
            let shown = if on {
                format!("{val}▏") // cursor caret
            } else {
                val.clone()
            };
            let label_style = if on { skin.accent_bold() } else { skin.dim() };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker}{label}: "), label_style),
                Span::styled(shown, skin.body()),
            ]));
        }
        lines.push(Line::from(""));
        if let Some(err) = &wiz.error {
            lines.push(Line::from(Span::styled(format!("  {err}"), skin.warn())));
        }
        lines.push(Line::from(Span::styled(
            "  Tab next · Enter create · Esc cancel",
            skin.dim(),
        )));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_footer(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let dirty = if self.dirty {
            Span::styled("  ·  unsaved — s to apply & restart Waybar", skin.warn())
        } else {
            Span::styled("", skin.dim())
        };
        let p = Paragraph::new(vec![
            Line::from(vec![Span::styled("Bar layout", skin.dim()), dirty]),
            Line::from(Span::styled(
                "shift+↑↓ reorder   ←→ move across   a add   d remove   f target   s apply",
                skin.dim(),
            )),
        ]);
        f.render_widget(p, area);
    }

    fn render_picker(&self, f: &mut Frame, area: Rect, skin: &Skin, sel: usize) {
        let rect = crate::tui::ui::centered_rect(area, 56, 16);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(skin.accent_bold())
            .title(" Add a module — Enter to add, Esc to cancel ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let view = (inner.height as usize).max(1);
        let scroll = sel.saturating_sub(view.saturating_sub(1));
        let items: Vec<ListItem> = MODULES
            .iter()
            .enumerate()
            .skip(scroll)
            .take(view)
            .map(|(i, m)| {
                let on = i == sel;
                let name = if on {
                    Span::styled(format!("▸ {}", m.label), skin.selection())
                } else {
                    Span::styled(format!("  {}", m.label), skin.body())
                };
                ListItem::new(Line::from(vec![
                    name,
                    Span::styled(format!("  {}", m.summary), skin.dim()),
                ]))
            })
            .collect();
        f.render_widget(List::new(items), inner);
    }
}

fn friendly(e: &studio_core::StudioError) -> String {
    match e {
        studio_core::StudioError::External { detail, .. } => detail.clone(),
        other => format!("{other:?}"),
    }
}

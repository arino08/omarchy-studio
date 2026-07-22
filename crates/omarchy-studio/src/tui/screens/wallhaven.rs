//! In-TUI wallhaven browser (spec 04 §6) — `w` in the Wallpapers screen.
//!
//! All network traffic runs on one worker thread that owns the [`Client`]
//! (and with it the rate budget); the UI sends [`Job`]s and drains
//! [`Outcome`]s, so a slow search never blocks a keystroke and the event
//! loop only polls while something is actually in flight. The worker exits
//! when the browser closes (its job sender drops).
//!
//! Thumbnails go through the shared on-disk [`ThumbCache`]: the UI checks
//! the cache directly for hits and only enqueues a fetch on a miss, so
//! revisiting results is instant and offline browsing of cached thumbs
//! keeps working (spec 04 §6 degradation).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};
use ratatui::Frame;

use studio_core::modules::wallhaven::{
    load_api_key, nearest_color, Client, Purity, Query, SearchPage, Sorting, ThumbCache, Wallpaper,
};

use super::super::imagecell::ImageCell;
use super::super::theme::Skin;

/// Ratio filter cycle (`r`).
const RATIOS: [(Option<&str>, &str); 5] = [
    (None, "any"),
    (Some("16x9"), "16:9"),
    (Some("16x10"), "16:10"),
    (Some("21x9"), "ultrawide"),
    (Some("9x16"), "portrait"),
];

enum Job {
    Search(Query),
    Thumb(Box<Wallpaper>),
    Download(Box<Wallpaper>, PathBuf, Then),
}

/// What the App should do with a finished download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// Just land it in backgrounds/<theme>/ (`d`).
    Keep,
    /// Set it as the wallpaper (`enter`).
    Set,
    /// Open the theme-from-wallpaper wizard on it (`t`).
    Craft,
}

enum Outcome {
    Page(Result<SearchPage, String>),
    /// id → cache path; `None` means the fetch failed (shrug, text fallback).
    Thumb(String, Option<PathBuf>),
    Downloaded(Result<(PathBuf, Then), String>),
}

pub enum BrowserAction {
    None,
    Close,
}

fn brief(e: studio_core::StudioError) -> String {
    match e {
        studio_core::StudioError::External { detail, .. } => detail,
        other => format!("{other:?}"),
    }
}

fn worker(jobs: Receiver<Job>, out: Sender<Outcome>, cache: Option<ThumbCache>) {
    let mut client = Client::online();
    for job in jobs {
        let outcome = match job {
            Job::Search(q) => Outcome::Page(client.search(&q).map_err(brief)),
            Job::Thumb(wp) => {
                let path = cache.as_ref().and_then(|c| client.thumb(c, &wp).ok());
                Outcome::Thumb(wp.id.clone(), path)
            }
            Job::Download(wp, dir, then) => {
                Outcome::Downloaded(client.download(&wp, &dir).map(|p| (p, then)).map_err(brief))
            }
        };
        if out.send(outcome).is_err() {
            return; // browser closed mid-request
        }
    }
}

pub struct WallhavenBrowser {
    query: Query,
    /// `/`: the search text being edited, replacing `query.q` on Enter.
    input: Option<String>,
    /// `k`: the API key being typed. Rendered masked — a shoulder-surfable
    /// credential shouldn't sit on screen in plain text.
    key_input: Option<String>,
    page: Option<SearchPage>,
    selected: usize,
    ratio_idx: usize,
    /// Accent hex of the active theme, for the `c` color-match toggle.
    accent: Option<String>,
    has_key: bool,
    download_dir: PathBuf,
    /// Thumb currently shown: (wallpaper id, cache path).
    thumb: Option<(String, PathBuf)>,
    thumb_inflight: Option<String>,
    searching: bool,
    downloading: bool,
    error: Option<String>,
    cache: Option<ThumbCache>,
    jobs: Sender<Job>,
    results: Receiver<Outcome>,
}

impl WallhavenBrowser {
    /// Open with a toplist search already running. `accent` is the theme's
    /// accent hex (drives `c`), `atleast` a minimum resolution like
    /// `2560x1440` (from the monitor, when known).
    pub fn open(accent: Option<String>, atleast: Option<String>, download_dir: PathBuf) -> Self {
        let (jobs_tx, jobs_rx) = channel();
        let (out_tx, out_rx) = channel();
        let worker_cache = ThumbCache::open_default().ok();
        std::thread::spawn(move || worker(jobs_rx, out_tx, worker_cache));
        let mut b = Self::with_channels(jobs_tx, out_rx, accent, atleast, download_dir);
        b.cache = ThumbCache::open_default().ok();
        b.search();
        b
    }

    /// Everything but the worker thread — what tests drive directly.
    fn with_channels(
        jobs: Sender<Job>,
        results: Receiver<Outcome>,
        accent: Option<String>,
        atleast: Option<String>,
        download_dir: PathBuf,
    ) -> Self {
        let query = Query {
            sorting: Sorting::Toplist,
            atleast,
            ..Query::default()
        };
        Self {
            query,
            input: None,
            key_input: None,
            page: None,
            selected: 0,
            ratio_idx: 0,
            accent,
            has_key: load_api_key().is_some(),
            download_dir,
            thumb: None,
            thumb_inflight: None,
            searching: false,
            downloading: false,
            error: None,
            cache: None,
            jobs,
            results,
        }
    }

    /// Is anything in flight? Drives the event loop's poll-vs-block choice.
    pub fn busy(&self) -> bool {
        self.searching || self.downloading || self.thumb_inflight.is_some()
    }

    fn search(&mut self) {
        self.query.page = 1;
        self.request_page();
    }

    fn request_page(&mut self) {
        self.error = None;
        self.searching = true;
        self.selected = 0;
        let _ = self.jobs.send(Job::Search(self.query.clone()));
    }

    fn selected_wp(&self) -> Option<&Wallpaper> {
        self.page.as_ref().and_then(|p| p.data.get(self.selected))
    }

    /// Make sure the selected result's thumbnail is on its way: cache hit is
    /// used immediately, a miss becomes a worker job (one at a time).
    fn ensure_thumb(&mut self) {
        let Some(wp) = self.selected_wp().cloned() else {
            return;
        };
        if self.thumb.as_ref().is_some_and(|(id, _)| *id == wp.id) {
            return;
        }
        if let Some(hit) = self.cache.as_ref().and_then(|c| c.get(&wp.id)) {
            self.thumb = Some((wp.id, hit));
            return;
        }
        if self.thumb_inflight.as_deref() != Some(&wp.id) {
            self.thumb_inflight = Some(wp.id.clone());
            let _ = self.jobs.send(Job::Thumb(Box::new(wp)));
        }
    }

    /// Drain worker outcomes. Returns a finished download for the App to act
    /// on (set / craft / toast); everything else lands in browser state.
    pub fn drain(&mut self) -> Option<(PathBuf, Then)> {
        let mut done = None;
        loop {
            match self.results.try_recv() {
                Ok(Outcome::Page(Ok(page))) => {
                    self.searching = false;
                    self.error = if page.data.is_empty() {
                        Some(format!("no results for `{}`", self.query.q))
                    } else {
                        None
                    };
                    self.page = Some(page);
                    self.selected = 0;
                    self.ensure_thumb();
                }
                Ok(Outcome::Page(Err(e))) => {
                    self.searching = false;
                    self.error = Some(offline_banner(&e));
                }
                Ok(Outcome::Thumb(id, path)) => {
                    if self.thumb_inflight.as_deref() == Some(&id) {
                        self.thumb_inflight = None;
                    }
                    if let Some(p) = path {
                        // Only adopt it if the user is still on that result.
                        if self.selected_wp().map(|w| w.id.as_str()) == Some(id.as_str()) {
                            self.thumb = Some((id, p));
                        }
                    }
                    // Selection may have moved while this one was in flight.
                    self.ensure_thumb();
                }
                Ok(Outcome::Downloaded(Ok(hit))) => {
                    self.downloading = false;
                    done = Some(hit);
                }
                Ok(Outcome::Downloaded(Err(e))) => {
                    self.downloading = false;
                    self.error = Some(offline_banner(&e));
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        done
    }

    pub fn handle(&mut self, key: KeyEvent) -> BrowserAction {
        // API-key entry owns every key while open.
        if let Some(buf) = self.key_input.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let entered = self.key_input.take().unwrap_or_default();
                    match studio_core::modules::wallhaven::save_api_key(&entered) {
                        Ok(()) => {
                            self.has_key = !entered.trim().is_empty();
                            // Re-search so the purity filter takes effect now
                            // rather than on the next launch.
                            self.error = None;
                            self.search();
                        }
                        Err(e) => self.error = Some(format!("couldn't save the key: {e:?}")),
                    }
                }
                KeyCode::Esc => self.key_input = None,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return BrowserAction::None;
        }

        // Search text entry owns every key while open.
        if let Some(buf) = self.input.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    self.query.q = self.input.take().unwrap_or_default();
                    self.search();
                }
                KeyCode::Esc => self.input = None,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return BrowserAction::None;
        }

        let n = self.page.as_ref().map_or(0, |p| p.data.len());
        if crate::tui::ui::list_nav(key.code, &mut self.selected, n) {
            self.ensure_thumb();
            return BrowserAction::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return BrowserAction::Close,
            KeyCode::Char('/') => self.input = Some(self.query.q.clone()),
            // Start from empty rather than the stored key: this is a paste
            // target, and echoing a saved credential back serves no one.
            KeyCode::Char('k') => self.key_input = Some(String::new()),
            KeyCode::Char('n') => {
                if let Some(p) = &self.page {
                    if p.meta.current_page < p.meta.last_page {
                        self.query.page = p.meta.current_page + 1;
                        self.request_page();
                    }
                }
            }
            KeyCode::Char('p') => {
                if let Some(p) = &self.page {
                    if p.meta.current_page > 1 {
                        self.query.page = p.meta.current_page - 1;
                        self.request_page();
                    }
                }
            }
            KeyCode::Char('s') => {
                let i = Sorting::ALL
                    .iter()
                    .position(|s| *s == self.query.sorting)
                    .unwrap_or(0);
                self.query.sorting = Sorting::ALL[(i + 1) % Sorting::ALL.len()];
                self.search();
            }
            KeyCode::Char('r') => {
                self.ratio_idx = (self.ratio_idx + 1) % RATIOS.len();
                self.query.ratios = RATIOS[self.ratio_idx].0.map(str::to_string);
                self.search();
            }
            KeyCode::Char('c') => {
                if self.query.color.is_some() {
                    self.query.color = None;
                    self.search();
                } else if let Some(picker) = self.accent.as_deref().and_then(nearest_color) {
                    self.query.color = Some(picker.to_string());
                    self.search();
                }
            }
            KeyCode::Char('P') if self.has_key => {
                self.query.purity = match self.query.purity {
                    Purity::Sfw => Purity::Sketchy,
                    Purity::Sketchy => Purity::Nsfw,
                    Purity::Nsfw => Purity::Sfw,
                };
                self.search();
            }
            KeyCode::Enter => self.download(Then::Set),
            KeyCode::Char('d') => self.download(Then::Keep),
            KeyCode::Char('t') => self.download(Then::Craft),
            _ => {}
        }
        BrowserAction::None
    }

    fn download(&mut self, then: Then) {
        if self.downloading {
            return;
        }
        let Some(wp) = self.selected_wp().cloned() else {
            return;
        };
        self.downloading = true;
        self.error = None;
        let _ = self
            .jobs
            .send(Job::Download(Box::new(wp), self.download_dir.clone(), then));
    }

    fn status(&self) -> String {
        if self.searching {
            return "searching…".into();
        }
        if self.downloading {
            return "downloading… (full-size images take a moment)".into();
        }
        match &self.page {
            Some(p) => format!(
                "page {}/{} · {} walls",
                p.meta.current_page,
                p.meta.last_page.max(1),
                p.meta.total
            ),
            None => String::new(),
        }
    }

    fn filter_line(&self, skin: &Skin) -> Line<'static> {
        let mut spans = vec![Span::styled(" sort ", skin.dim())];
        spans.push(Span::styled(
            self.query.sorting.label().to_string(),
            skin.body(),
        ));
        spans.push(Span::styled("  ratio ", skin.dim()));
        spans.push(Span::styled(
            RATIOS[self.ratio_idx].1.to_string(),
            skin.body(),
        ));
        spans.push(Span::styled("  color ", skin.dim()));
        match &self.query.color {
            Some(c) => {
                spans.push(swatch(c));
                spans.push(Span::styled(" accent", skin.body()));
            }
            None => spans.push(Span::styled("off", skin.body())),
        }
        spans.push(Span::styled("  purity ", skin.dim()));
        if self.has_key {
            spans.push(Span::styled(
                self.query.purity.label().to_string(),
                skin.body(),
            ));
        } else {
            // Full pointer lives in the README; keep the line inside 110 cols.
            spans.push(Span::styled("SFW (add api_key for more)", skin.dim()));
        }
        Line::from(spans)
    }

    pub fn render(&self, f: &mut Frame, area: Rect, skin: &Skin, images: &mut ImageCell) {
        f.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(skin.accent_bold())
            .title(Line::from(vec![
                Span::styled(" ✦ ", skin.accent_bold()),
                Span::styled("wallhaven ", skin.body()),
            ]))
            .title_top(
                Line::from(Span::styled(format!(" {} ", self.status()), skin.dim()))
                    .right_aligned(),
            )
            .title_bottom(
                Line::from(Span::styled(
                    " / search · ⏎ set · d save · t theme · s/r/c/P filters · k api key · esc ",
                    skin.dim(),
                ))
                .right_aligned(),
            )
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // search line
                Constraint::Length(1), // filters
                Constraint::Length(1), // spacer / error banner
                Constraint::Min(1),    // results
            ])
            .split(inner);

        // Search line: key entry while open (same one-line field — showing
        // both at once would be noise), else the input field or the query.
        let q_line = match (&self.key_input, &self.input) {
            (Some(buf), _) => Line::from(vec![
                Span::styled(" api key ", skin.accent_bold()),
                // Masked: it's a credential, and this browser is the sort of
                // thing people screen-share.
                Span::styled("•".repeat(buf.chars().count()), skin.body()),
                Span::styled("▏", skin.accent_bold()),
                Span::styled("  paste, then ⏎ · esc cancels", skin.dim()),
            ]),
            (None, Some(buf)) => Line::from(vec![
                Span::styled(" search ", skin.accent_bold()),
                Span::styled(buf.clone(), skin.body()),
                Span::styled("▏", skin.accent_bold()),
            ]),
            (None, None) => Line::from(vec![
                Span::styled(" search ", skin.dim()),
                Span::styled(
                    if self.query.q.is_empty() {
                        "(toplist — / to type a query)".to_string()
                    } else {
                        self.query.q.clone()
                    },
                    skin.body(),
                ),
            ]),
        };
        f.render_widget(Paragraph::new(q_line), rows[0]);
        f.render_widget(Paragraph::new(self.filter_line(skin)), rows[1]);
        if let Some(err) = &self.error {
            f.render_widget(
                Paragraph::new(Span::styled(format!(" {err}"), skin.error())),
                rows[2],
            );
        }

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(44), Constraint::Min(20)])
            .split(rows[3]);
        self.render_list(f, cols[0], skin);
        self.render_preview(f, cols[1], skin, images);
    }

    fn render_list(&self, f: &mut Frame, area: Rect, skin: &Skin) {
        let Some(page) = &self.page else {
            return;
        };
        // Keep the selection inside the viewport.
        let h = area.height as usize;
        let offset = (self.selected + 1).saturating_sub(h);
        let items: Vec<ListItem> = page
            .data
            .iter()
            .enumerate()
            .skip(offset)
            .take(h)
            .map(|(i, wp)| {
                let sel = i == self.selected;
                let mut spans = vec![
                    Span::styled(if sel { "▌" } else { " " }.to_string(), skin.accent_bold()),
                    Span::styled(
                        format!("{:<8}", wp.id),
                        if sel { skin.accent_bold() } else { skin.body() },
                    ),
                    Span::styled(format!("{:<11}", wp.resolution), skin.dim()),
                ];
                for c in wp.colors.iter().take(5) {
                    spans.push(swatch(c));
                }
                let item = ListItem::new(Line::from(spans));
                if sel {
                    item.style(ratatui::style::Style::default().bg(skin.sel_bg))
                } else {
                    item
                }
            })
            .collect();
        f.render_widget(List::new(items), area);
    }

    fn render_preview(&self, f: &mut Frame, area: Rect, skin: &Skin, images: &mut ImageCell) {
        let Some(wp) = self.selected_wp() else { return };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(area);
        let mut drew = false;
        if let Some((id, path)) = &self.thumb {
            if *id == wp.id {
                drew = images.render(f, rows[0], path);
            }
        }
        if !drew {
            let note = if self.thumb_inflight.as_deref() == Some(&wp.id) {
                "fetching preview…"
            } else {
                "no preview — enter still sets it"
            };
            f.render_widget(
                Paragraph::new(Span::styled(note, skin.dim())).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(skin.border()),
                ),
                rows[0],
            );
        }
        let mb = wp.file_size as f64 / (1024.0 * 1024.0);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(format!("{} ", wp.resolution), skin.body()),
                    Span::styled(
                        format!("· {} · {:.1} MB · {}", wp.category, mb, wp.purity),
                        skin.dim(),
                    ),
                ]),
                Line::from(Span::styled(wp.url.clone(), skin.dim())),
            ]),
            rows[1],
        );
    }
}

/// "offline" phrasing per spec 04 §6 — local features keep working.
fn offline_banner(detail: &str) -> String {
    format!("wallhaven unreachable ({detail}) — local features unaffected")
}

/// Two-cell color swatch.
fn swatch(hex: &str) -> Span<'static> {
    let c = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    Span::styled(
        "  ",
        ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(
            (c >> 16) as u8,
            (c >> 8) as u8,
            c as u8,
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::modules::wallhaven::{Meta, Thumbs};

    fn wall(id: &str) -> Wallpaper {
        Wallpaper {
            id: id.into(),
            url: format!("https://wallhaven.cc/w/{id}"),
            purity: "sfw".into(),
            category: "general".into(),
            dimension_x: 3840,
            dimension_y: 2160,
            resolution: "3840x2160".into(),
            ratio: "1.78".into(),
            file_size: 1024 * 1024,
            file_type: "image/jpeg".into(),
            colors: vec!["#0a9058".into()],
            path: format!("https://w.wallhaven.cc/full/{id}.jpg"),
            thumbs: Thumbs {
                large: format!("https://th.wallhaven.cc/lg/{id}.jpg"),
                original: String::new(),
                small: String::new(),
            },
        }
    }

    fn page(ids: &[&str], current: u32, last: u32) -> SearchPage {
        SearchPage {
            data: ids.iter().map(|i| wall(i)).collect(),
            meta: Meta {
                current_page: current,
                last_page: last,
                total: ids.len() as u64,
            },
        }
    }

    fn browser() -> (WallhavenBrowser, Receiver<Job>, Sender<Outcome>) {
        let (jobs_tx, jobs_rx) = channel();
        let (out_tx, out_rx) = channel();
        let b = WallhavenBrowser::with_channels(
            jobs_tx,
            out_rx,
            Some("#0a9058".into()),
            None,
            PathBuf::from("/tmp/nowhere"),
        );
        (b, jobs_rx, out_tx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, ratatui::crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn page_arrival_resets_selection_and_reports_status() {
        let (mut b, _jobs, out) = browser();
        b.searching = true;
        b.selected = 7;
        out.send(Outcome::Page(Ok(page(&["aaa", "bbb"], 1, 5))))
            .unwrap();
        assert!(b.drain().is_none());
        assert!(!b.busy() || b.thumb_inflight.is_some()); // only thumb may remain
        assert_eq!(b.selected, 0);
        assert_eq!(b.status(), "page 1/5 · 2 walls");
    }

    #[test]
    fn paging_respects_bounds() {
        let (mut b, jobs, out) = browser();
        out.send(Outcome::Page(Ok(page(&["aaa"], 1, 1)))).unwrap();
        b.drain();
        while jobs.try_recv().is_ok() {} // discard thumb request
        b.handle(key(KeyCode::Char('n'))); // last page already
        b.handle(key(KeyCode::Char('p'))); // first page already
        assert!(jobs.try_recv().is_err(), "no request should have been sent");
    }

    #[test]
    fn color_toggle_maps_accent_through_the_picker_palette() {
        let (mut b, _jobs, _out) = browser();
        b.handle(key(KeyCode::Char('c')));
        assert_eq!(b.query.color.as_deref(), Some("66cccc")); // jade → teal slot
        b.handle(key(KeyCode::Char('c')));
        assert_eq!(b.query.color, None);
    }

    #[test]
    fn purity_stays_sfw_without_api_key() {
        let (mut b, _jobs, _out) = browser();
        b.has_key = false;
        b.handle(key(KeyCode::Char('P')));
        assert_eq!(b.query.purity, Purity::Sfw);
    }

    #[test]
    fn k_opens_key_entry_and_esc_discards_it() {
        let (mut b, _jobs, _out) = browser();
        assert!(b.key_input.is_none());
        b.handle(key(KeyCode::Char('k')));
        // Starts empty rather than echoing a saved credential back.
        assert_eq!(b.key_input.as_deref(), Some(""));
        for c in "abc".chars() {
            b.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(b.key_input.as_deref(), Some("abc"));
        b.handle(key(KeyCode::Backspace));
        assert_eq!(b.key_input.as_deref(), Some("ab"));
        b.handle(key(KeyCode::Esc));
        assert!(b.key_input.is_none(), "esc must not save");
    }

    #[test]
    fn key_entry_swallows_keys_that_would_otherwise_act() {
        // `d` downloads and `q` closes; while typing a key they're just text.
        let (mut b, _jobs, _out) = browser();
        b.handle(key(KeyCode::Char('k')));
        assert!(matches!(
            b.handle(key(KeyCode::Char('q'))),
            BrowserAction::None
        ));
        b.handle(key(KeyCode::Char('d')));
        assert_eq!(b.key_input.as_deref(), Some("qd"));
    }

    #[test]
    fn download_outcome_reaches_the_app() {
        let (mut b, _jobs, out) = browser();
        out.send(Outcome::Page(Ok(page(&["aaa"], 1, 1)))).unwrap();
        b.drain();
        b.handle(key(KeyCode::Enter));
        assert!(b.downloading);
        out.send(Outcome::Downloaded(Ok((
            PathBuf::from("/x/wallhaven-aaa.jpg"),
            Then::Set,
        ))))
        .unwrap();
        let (path, then) = b.drain().expect("finished download surfaces");
        assert_eq!(then, Then::Set);
        assert_eq!(path, PathBuf::from("/x/wallhaven-aaa.jpg"));
        assert!(!b.downloading);
    }

    #[test]
    fn search_input_owns_keys_until_enter() {
        let (mut b, jobs, _out) = browser();
        b.handle(key(KeyCode::Char('/')));
        for c in "jade".chars() {
            b.handle(key(KeyCode::Char(c)));
        }
        // `q` must type (then backspace away), not close, while editing
        assert!(matches!(
            b.handle(key(KeyCode::Char('q'))),
            BrowserAction::None
        ));
        b.handle(key(KeyCode::Backspace));
        b.handle(key(KeyCode::Enter));
        assert_eq!(b.query.q, "jade");
        assert!(b.searching);
        assert!(matches!(jobs.try_recv(), Ok(Job::Search(_))));
    }
}

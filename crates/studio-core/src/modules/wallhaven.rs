//! Wallhaven client (spec 04 §6, roadmap 0.6.3).
//!
//! Blocking `ureq` client meant to live on a worker thread (spec 00 D3 — no
//! async runtime). Everything network-shaped goes through the [`Fetch`] trait
//! so tests run entirely on vendored fixtures:
//!
//! - [`Query`] builds the `/search` parameter set: free text, category
//!   bitmask, purity (SFW-only unless an API key is present — spec FR11.6),
//!   sorting, minimum resolution, ratio, and one of wallhaven's fixed picker
//!   colors ("match my accent" via [`nearest_color`]).
//! - [`RateBudget`] enforces the anonymous 45 req/min budget client-side and
//!   backs off exponentially on 429.
//! - [`ThumbCache`] is an mtime-LRU file cache under
//!   `~/.cache/omarchy-studio/wallhaven/`, 200 MB cap (spec 01 §7).
//!
//! The optional API key lives in Studio's own `config.toml`
//! (`[wallhaven] api_key = "…"`), never anywhere else (spec 01 §7).

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::Result;
use crate::modules::color::Color;
use crate::StudioError;

pub const BASE_URL: &str = "https://wallhaven.cc/api/v1";

/// Anonymous API budget: 45 requests per minute.
const RATE_LIMIT: usize = 45;
const RATE_SPAN: Duration = Duration::from_secs(60);

/// Thumbnail cache cap (spec 01 §7).
pub const CACHE_CAP_BYTES: u64 = 200 * 1024 * 1024;

/// Refuse to buffer bodies past this — full wallpapers top out well below.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------- fetching

#[derive(Debug)]
pub enum FetchError {
    /// Non-2xx response.
    Status(u16),
    /// DNS/TLS/socket trouble — the "offline banner" case.
    Network(String),
}

/// The one seam between this module and the network.
pub trait Fetch: Send {
    fn get(&self, url: &str) -> std::result::Result<Vec<u8>, FetchError>;
}

/// Real fetcher. One agent = one connection pool + consistent UA.
pub struct HttpFetch {
    agent: ureq::Agent,
}

impl HttpFetch {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("omarchy-studio/", env!("CARGO_PKG_VERSION")))
                .build(),
        }
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for HttpFetch {
    fn get(&self, url: &str) -> std::result::Result<Vec<u8>, FetchError> {
        match self.agent.get(url).call() {
            Ok(resp) => {
                let mut body = Vec::new();
                resp.into_reader()
                    .take(MAX_BODY_BYTES)
                    .read_to_end(&mut body)
                    .map_err(|e| FetchError::Network(e.to_string()))?;
                Ok(body)
            }
            Err(ureq::Error::Status(code, _)) => Err(FetchError::Status(code)),
            Err(e) => Err(FetchError::Network(e.to_string())),
        }
    }
}

// ------------------------------------------------------------------ query

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Purity {
    #[default]
    Sfw,
    /// SFW + sketchy — needs an API key.
    Sketchy,
    /// Everything — needs an API key.
    Nsfw,
}

impl Purity {
    fn mask(self) -> &'static str {
        match self {
            Purity::Sfw => "100",
            Purity::Sketchy => "110",
            Purity::Nsfw => "111",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Purity::Sfw => "SFW",
            Purity::Sketchy => "+ sketchy",
            Purity::Nsfw => "everything",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sorting {
    #[default]
    Relevance,
    Toplist,
    Random,
    Views,
    DateAdded,
    Favorites,
}

impl Sorting {
    fn param(self) -> &'static str {
        match self {
            Sorting::Relevance => "relevance",
            Sorting::Toplist => "toplist",
            Sorting::Random => "random",
            Sorting::Views => "views",
            Sorting::DateAdded => "date_added",
            Sorting::Favorites => "favorites",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sorting::Relevance => "relevance",
            Sorting::Toplist => "toplist",
            Sorting::Random => "random",
            Sorting::Views => "most viewed",
            Sorting::DateAdded => "newest",
            Sorting::Favorites => "most faved",
        }
    }

    pub const ALL: [Sorting; 6] = [
        Sorting::Relevance,
        Sorting::Toplist,
        Sorting::Random,
        Sorting::Views,
        Sorting::DateAdded,
        Sorting::Favorites,
    ];
}

/// One `/search` request, before pagination.
#[derive(Debug, Clone)]
pub struct Query {
    pub q: String,
    pub general: bool,
    pub anime: bool,
    pub people: bool,
    pub purity: Purity,
    pub sorting: Sorting,
    /// Minimum resolution, e.g. `2560x1440` (auto-filled from the monitor).
    pub atleast: Option<String>,
    /// e.g. `16x9`.
    pub ratios: Option<String>,
    /// One of [`COLORS`] — use [`nearest_color`] to map a palette slot.
    pub color: Option<String>,
    pub page: u32,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            q: String::new(),
            general: true,
            anime: true,
            people: false,
            purity: Purity::default(),
            sorting: Sorting::default(),
            atleast: None,
            ratios: None,
            color: None,
            page: 1,
        }
    }
}

impl Query {
    /// Full parameter string. Purity above SFW silently degrades to SFW when
    /// no API key is present — the *frontend* is responsible for greying the
    /// selector with the "requires API key" note (FR11.6); the client just
    /// guarantees we never send a request wallhaven would reject.
    pub fn params(&self, api_key: Option<&str>) -> String {
        let mask = |on: bool| if on { '1' } else { '0' };
        let purity = if api_key.is_some() {
            self.purity
        } else {
            Purity::Sfw
        };
        let mut s = format!(
            "q={}&categories={}{}{}&purity={}&sorting={}&page={}",
            urlencode(&self.q),
            mask(self.general),
            mask(self.anime),
            mask(self.people),
            purity.mask(),
            self.sorting.param(),
            self.page,
        );
        if let Some(atleast) = &self.atleast {
            s.push_str(&format!("&atleast={}", urlencode(atleast)));
        }
        if let Some(ratios) = &self.ratios {
            s.push_str(&format!("&ratios={}", urlencode(ratios)));
        }
        if let Some(color) = &self.color {
            s.push_str(&format!("&colors={}", urlencode(color)));
        }
        if let Some(key) = api_key {
            s.push_str(&format!("&apikey={}", urlencode(key)));
        }
        s
    }
}

/// RFC 3986 percent-encoding, keeping only unreserved characters.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Wallhaven's fixed color-picker palette (the only values `colors=` accepts).
pub const COLORS: [&str; 29] = [
    "660000", "990000", "cc0000", "cc3333", "ea4c88", "993399", "663399", "333399", "0066cc",
    "0099cc", "66cccc", "77cc33", "669900", "336600", "666600", "999900", "cccc33", "ffff00",
    "ffcc33", "ff9900", "ff6600", "cc6633", "996633", "663300", "000000", "999999", "cccccc",
    "ffffff", "424153",
];

/// Nearest picker color to an arbitrary hex — "match my accent".
///
/// Hue-first, like extraction's ANSI matching: raw RGB distance would map a
/// vivid jade to the grey-navy `424153`; users asking for "walls that match
/// my accent" mean the *hue*. Greys (either side) only compete on lightness.
pub fn nearest_color(hex: &str) -> Option<&'static str> {
    // greyness = chroma, not HSL saturation — near-white off-whites have huge
    // HSL "saturation" because the denominator collapses at the extremes
    let chroma = |c: &Color| {
        let max = c.r.max(c.g).max(c.b);
        let min = c.r.min(c.g).min(c.b);
        (max - min) as f32 / 255.0
    };
    let want = Color::parse(hex).ok()?;
    let (wh, ws, wl) = want.to_hsl();
    let grey = chroma(&want) < 0.15;
    COLORS
        .iter()
        .copied()
        .filter_map(|c| {
            let cand = Color::parse(c).expect("COLORS entries are valid hex");
            let (ch, cs, cl) = cand.to_hsl();
            if grey != (chroma(&cand) < 0.15) {
                return None; // greys never match colors, colors never match greys
            }
            let score = if grey {
                (wl - cl).abs()
            } else {
                let dh = (wh - ch).abs();
                let dh = dh.min(360.0 - dh) / 180.0;
                // hue dominates; saturation/lightness only break near-ties
                8.0 * dh * dh + (wl - cl) * (wl - cl) + 0.5 * (ws - cs) * (ws - cs)
            };
            Some((score, c))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| c)
}

// ----------------------------------------------------------------- models

#[derive(Debug, Clone, Deserialize)]
pub struct Thumbs {
    pub large: String,
    pub original: String,
    pub small: String,
}

/// One search hit, as wallhaven serves it.
#[derive(Debug, Clone, Deserialize)]
pub struct Wallpaper {
    pub id: String,
    /// The wallhaven *page* for this wallpaper.
    pub url: String,
    pub purity: String,
    pub category: String,
    pub dimension_x: u32,
    pub dimension_y: u32,
    pub resolution: String,
    pub ratio: String,
    pub file_size: u64,
    /// MIME, e.g. `image/jpeg`.
    pub file_type: String,
    #[serde(default)]
    pub colors: Vec<String>,
    /// The full-size image URL.
    pub path: String,
    pub thumbs: Thumbs,
}

impl Wallpaper {
    /// File extension of the full-size image (`jpg`, `png`, …).
    pub fn ext(&self) -> &str {
        self.path.rsplit('.').next().unwrap_or("jpg")
    }

    /// Download name per spec 04 §6: `wallhaven-<id>.<ext>`.
    pub fn file_name(&self) -> String {
        format!("wallhaven-{}.{}", self.id, self.ext())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub current_page: u32,
    pub last_page: u32,
    pub total: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchPage {
    pub data: Vec<Wallpaper>,
    pub meta: Meta,
}

// ------------------------------------------------------------ rate budget

/// Client-side sliding-window budget + exponential 429 backoff.
///
/// Pure bookkeeping over injected `Instant`s — the [`Client`] does the actual
/// sleeping, tests never wait.
#[derive(Debug)]
pub struct RateBudget {
    window: VecDeque<Instant>,
    limit: usize,
    span: Duration,
    backoff_until: Option<Instant>,
    strikes: u32,
}

impl Default for RateBudget {
    fn default() -> Self {
        Self::new(RATE_LIMIT, RATE_SPAN)
    }
}

impl RateBudget {
    pub fn new(limit: usize, span: Duration) -> Self {
        Self {
            window: VecDeque::new(),
            limit,
            span,
            backoff_until: None,
            strikes: 0,
        }
    }

    /// How long the next request must wait to stay inside the budget.
    pub fn delay(&mut self, now: Instant) -> Duration {
        while let Some(&front) = self.window.front() {
            if now.duration_since(front) >= self.span {
                self.window.pop_front();
            } else {
                break;
            }
        }
        let window_wait = if self.window.len() < self.limit {
            Duration::ZERO
        } else {
            // window is non-empty here: limit >= 1
            (*self.window.front().unwrap() + self.span).saturating_duration_since(now)
        };
        let backoff_wait = self
            .backoff_until
            .map(|t| t.saturating_duration_since(now))
            .unwrap_or(Duration::ZERO);
        window_wait.max(backoff_wait)
    }

    /// Book a request that is firing now.
    pub fn record(&mut self, now: Instant) {
        self.window.push_back(now);
    }

    /// A response came back fine — forget any backoff.
    pub fn success(&mut self) {
        self.strikes = 0;
        self.backoff_until = None;
    }

    /// Server said 429: 2 s, 4 s, 8 s … capped at 60 s.
    pub fn throttled(&mut self, now: Instant) {
        self.strikes = (self.strikes + 1).min(6);
        let secs = (2u64 << (self.strikes - 1)).min(60);
        self.backoff_until = Some(now + Duration::from_secs(secs));
    }
}

// ------------------------------------------------------------ thumb cache

/// Filesystem LRU keyed on wallpaper id; recency = file mtime.
pub struct ThumbCache {
    dir: PathBuf,
    cap_bytes: u64,
}

impl ThumbCache {
    pub fn new(dir: PathBuf, cap_bytes: u64) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, cap_bytes })
    }

    /// The real cache: `~/.cache/omarchy-studio/wallhaven/`, 200 MB.
    pub fn open_default() -> Result<Self> {
        Self::new(crate::studio_cache_dir().join("wallhaven"), CACHE_CAP_BYTES)
    }

    fn file_for(&self, id: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{id}.{ext}"))
    }

    /// Cache hit → path, with recency bumped so it survives eviction longest.
    pub fn get(&self, id: &str) -> Option<PathBuf> {
        let prefix = format!("{id}.");
        let hit = std::fs::read_dir(&self.dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
            })?;
        if let Ok(f) = std::fs::File::open(&hit) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        Some(hit)
    }

    pub fn put(&self, id: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.file_for(id, ext);
        std::fs::write(&path, bytes)?;
        self.evict()?;
        Ok(path)
    }

    /// Drop oldest-touched files until the cache fits the cap.
    fn evict(&self) -> Result<()> {
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&self.dir)?.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_file() {
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                files.push((mtime, meta.len(), entry.path()));
            }
        }
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        files.sort(); // oldest mtime first
        for (_, len, path) in files {
            if total <= self.cap_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- api key

/// Read `[wallhaven] api_key` out of a Studio `config.toml`.
pub fn api_key_from(config_toml: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_toml).ok()?;
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    doc.get("wallhaven")?
        .get("api_key")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The real key location: `~/.config/omarchy-studio/config.toml` (spec 01 §7).
pub fn load_api_key() -> Option<String> {
    api_key_from(&crate::studio_config_dir().join("config.toml"))
}

/// Write `[wallhaven] api_key` into a Studio `config.toml`, preserving every
/// other setting in the file (`[targets]`, `[themesync]`, …) — toml_edit keeps
/// formatting and comments, so hand-written config survives a write from the
/// TUI. An empty key removes the entry rather than storing `""`.
pub fn save_api_key_to(config_toml: &Path, key: &str) -> Result<()> {
    let text = std::fs::read_to_string(config_toml).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| StudioError::ParseFailed {
        file: config_toml.to_path_buf(),
        line: None,
        hint: format!("Studio's own config.toml isn't valid TOML: {e}"),
    })?;
    let key = key.trim();
    if key.is_empty() {
        // The section may be a `[wallhaven]` table or an inline
        // `wallhaven = { … }`; clear the key from whichever this file uses.
        match doc.get_mut("wallhaven") {
            Some(toml_edit::Item::Table(t)) => {
                t.remove("api_key");
            }
            Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => {
                t.remove("api_key");
            }
            _ => {}
        }
    } else {
        // Seed a real `[wallhaven]` section when the file has none: indexing
        // straight into a missing key produces an *inline* table
        // (`wallhaven = { api_key = "…" }`), which reads back fine but doesn't
        // match the section style the rest of this config uses.
        if doc.get("wallhaven").is_none() {
            doc["wallhaven"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        doc["wallhaven"]["api_key"] = toml_edit::value(key);
    }
    if let Some(parent) = config_toml.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::configfs::atomic_write(config_toml, &doc.to_string())?;
    // The key is a credential: keep it out of other users' reach. Best-effort,
    // since a filesystem without unix permissions is not a reason to fail.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config_toml, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Save the key to the real config location.
pub fn save_api_key(key: &str) -> Result<()> {
    save_api_key_to(&crate::studio_config_dir().join("config.toml"), key)
}

// ------------------------------------------------------------------ client

pub struct Client {
    fetch: Box<dyn Fetch>,
    budget: RateBudget,
    base: String,
    pub api_key: Option<String>,
}

impl Client {
    pub fn new(fetch: Box<dyn Fetch>, api_key: Option<String>) -> Self {
        Self {
            fetch,
            budget: RateBudget::default(),
            base: BASE_URL.to_string(),
            api_key,
        }
    }

    /// Real client: HTTP fetcher + whatever key `config.toml` holds.
    pub fn online() -> Self {
        Self::new(Box::new(HttpFetch::new()), load_api_key())
    }

    pub fn search(&mut self, query: &Query) -> Result<SearchPage> {
        let url = format!(
            "{}/search?{}",
            self.base,
            query.params(self.api_key.as_deref())
        );
        let body = self.request(&url)?;
        serde_json::from_slice(&body).map_err(|e| StudioError::External {
            cmd: "wallhaven".into(),
            detail: format!("unexpected response shape: {e}"),
        })
    }

    /// Thumbnail through the LRU cache (`thumbs.large`).
    pub fn thumb(&mut self, cache: &ThumbCache, wp: &Wallpaper) -> Result<PathBuf> {
        if let Some(hit) = cache.get(&wp.id) {
            return Ok(hit);
        }
        let ext = wp.thumbs.large.rsplit('.').next().unwrap_or("jpg");
        let bytes = self.request(&wp.thumbs.large)?;
        cache.put(&wp.id, ext, &bytes)
    }

    /// Full-size image → `<dir>/wallhaven-<id>.<ext>`. Already-downloaded
    /// files are reused, not re-fetched.
    pub fn download(&mut self, wp: &Wallpaper, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let dest = dir.join(wp.file_name());
        if dest.is_file() {
            return Ok(dest);
        }
        let bytes = self.request(&wp.path)?;
        std::fs::write(&dest, bytes)?;
        Ok(dest)
    }

    /// Budgeted GET. Sleeps to stay inside the rate budget (we run on a
    /// worker thread — spec 00 D3); a 429 arms the backoff and surfaces as an
    /// error so the frontend can show "retrying shortly" instead of hanging.
    fn request(&mut self, url: &str) -> Result<Vec<u8>> {
        let wait = self.budget.delay(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        self.budget.record(Instant::now());
        match self.fetch.get(url) {
            Ok(body) => {
                self.budget.success();
                Ok(body)
            }
            Err(FetchError::Status(429)) => {
                self.budget.throttled(Instant::now());
                Err(StudioError::External {
                    cmd: "wallhaven".into(),
                    detail: "rate limited (429) — backing off, retry in a moment".into(),
                })
            }
            Err(FetchError::Status(code)) => Err(StudioError::External {
                cmd: "wallhaven".into(),
                detail: format!("HTTP {code} from {url}"),
            }),
            Err(FetchError::Network(e)) => Err(StudioError::External {
                cmd: "wallhaven".into(),
                detail: format!("wallhaven unreachable — check your connection; local features unaffected ({e})"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const FIXTURE: &str = include_str!("../../tests/fixtures/wallhaven/search.json");

    /// Serves canned bodies and records every URL it was asked for.
    struct StubFetch {
        responses: Mutex<Vec<std::result::Result<Vec<u8>, FetchError>>>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl StubFetch {
        fn new(
            responses: Vec<std::result::Result<Vec<u8>, FetchError>>,
        ) -> (Self, Arc<Mutex<Vec<String>>>) {
            let seen = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    responses: Mutex::new(responses),
                    seen: seen.clone(),
                },
                seen,
            )
        }
    }

    impl Fetch for StubFetch {
        fn get(&self, url: &str) -> std::result::Result<Vec<u8>, FetchError> {
            self.seen.lock().unwrap().push(url.to_string());
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-wh-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn query_params_gate_purity_on_api_key() {
        let mut q = Query {
            q: "jade city".into(),
            people: true,
            purity: Purity::Sketchy,
            sorting: Sorting::Toplist,
            atleast: Some("2560x1440".into()),
            ratios: Some("16x9".into()),
            color: Some("336600".into()),
            page: 2,
            ..Query::default()
        };
        // anonymous: sketchy silently degrades to SFW-only
        assert_eq!(
            q.params(None),
            "q=jade%20city&categories=111&purity=100&sorting=toplist&page=2\
             &atleast=2560x1440&ratios=16x9&colors=336600"
        );
        // with a key the requested tier goes through, and the key is appended
        let with_key = q.params(Some("k3y"));
        assert!(with_key.contains("purity=110"), "{with_key}");
        assert!(with_key.ends_with("&apikey=k3y"), "{with_key}");

        q.anime = false;
        assert!(q.params(None).contains("categories=101"));
    }

    #[test]
    fn nearest_color_maps_palette_slots_to_picker() {
        assert_eq!(nearest_color("#cc0000"), Some("cc0000"));
        assert_eq!(nearest_color("#f8f8f2"), Some("ffffff"));
        // osaka jade is a green-cyan — must land on teal, never the grey-navy
        // that raw RGB distance would pick
        assert_eq!(nearest_color("#0a9058"), Some("66cccc"));
        assert_eq!(nearest_color("not-a-color"), None);
    }

    #[test]
    fn fixture_response_parses() {
        let page: SearchPage = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(page.meta.current_page, 1);
        assert_eq!(page.meta.last_page, 47);
        assert_eq!(page.data.len(), 2);
        let wp = &page.data[0];
        assert_eq!(wp.id, "94x38z");
        assert_eq!(wp.resolution, "3840x2160");
        assert_eq!(wp.ext(), "jpg");
        assert_eq!(wp.file_name(), "wallhaven-94x38z.jpg");
        assert!(wp.thumbs.large.contains("th.wallhaven.cc"));
    }

    #[test]
    fn rate_budget_window_and_backoff() {
        let start = Instant::now();
        let mut b = RateBudget::new(3, Duration::from_secs(60));

        assert_eq!(b.delay(start), Duration::ZERO);
        for _ in 0..3 {
            b.record(start);
        }
        // window full: 4th request must wait for the window to roll over
        assert_eq!(
            b.delay(start + Duration::from_secs(1)),
            Duration::from_secs(59)
        );
        // …and is free again once the span has passed
        assert_eq!(b.delay(start + Duration::from_secs(61)), Duration::ZERO);

        // 429s: exponential 2/4/8…, reset by a success
        b.throttled(start);
        assert_eq!(b.delay(start), Duration::from_secs(2));
        b.throttled(start);
        assert_eq!(b.delay(start), Duration::from_secs(4));
        for _ in 0..10 {
            b.throttled(start);
        }
        assert_eq!(
            b.delay(start),
            Duration::from_secs(60),
            "backoff caps at 60s"
        );
        b.success();
        assert_eq!(b.delay(start + Duration::from_secs(61)), Duration::ZERO);
    }

    #[test]
    fn thumb_cache_lru_evicts_oldest() {
        let cache = ThumbCache::new(tmp_dir("cache"), 10).unwrap();
        let a = cache.put("aaa", "jpg", b"12345").unwrap();
        let b = cache.put("bbb", "jpg", b"12345").unwrap();
        assert!(a.is_file() && b.is_file());

        // make `a` clearly older, then touch it via get() so `b` becomes LRU
        let old = std::time::SystemTime::now() - Duration::from_secs(120);
        std::fs::File::open(&a).unwrap().set_modified(old).unwrap();
        assert_eq!(cache.get("aaa"), Some(a.clone()));

        // 5 more bytes blow the 10-byte cap → oldest-touched (`b`) is evicted
        let c = cache.put("ccc", "jpg", b"12345").unwrap();
        assert!(c.is_file());
        assert!(cache.get("bbb").is_none(), "LRU entry evicted");
        assert!(
            cache.get("aaa").is_some(),
            "recently-touched entry survives"
        );
    }

    #[test]
    fn client_search_thumb_download_on_fixtures() {
        let (stub, seen) = StubFetch::new(vec![
            Ok(FIXTURE.as_bytes().to_vec()),
            Ok(b"thumbbytes".to_vec()),
            Ok(b"fullbytes".to_vec()),
        ]);
        let mut client = Client::new(Box::new(stub), None);

        let page = client.search(&Query::default()).unwrap();
        assert_eq!(page.data.len(), 2);
        assert!(seen.lock().unwrap()[0]
            .starts_with("https://wallhaven.cc/api/v1/search?q=&categories=110&purity=100"));

        let cache = ThumbCache::new(tmp_dir("thumbs"), 1024).unwrap();
        let thumb = client.thumb(&cache, &page.data[0]).unwrap();
        assert_eq!(std::fs::read(&thumb).unwrap(), b"thumbbytes");
        // second call is a cache hit — no request
        let requests_before = seen.lock().unwrap().len();
        client.thumb(&cache, &page.data[0]).unwrap();
        assert_eq!(seen.lock().unwrap().len(), requests_before);

        let dir = tmp_dir("dl");
        let dest = client.download(&page.data[0], &dir).unwrap();
        assert_eq!(dest, dir.join("wallhaven-94x38z.jpg"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"fullbytes");
        // existing file short-circuits — still no extra request
        let requests_before = seen.lock().unwrap().len();
        client.download(&page.data[0], &dir).unwrap();
        assert_eq!(seen.lock().unwrap().len(), requests_before);
    }

    #[test]
    fn client_429_arms_backoff_and_offline_is_friendly() {
        let (stub, _) = StubFetch::new(vec![
            Err(FetchError::Status(429)),
            Err(FetchError::Network("dns error".into())),
        ]);
        let mut client = Client::new(Box::new(stub), None);

        let err = client.search(&Query::default()).unwrap_err();
        assert!(format!("{err:?}").contains("rate limited"));
        assert!(
            client.budget.delay(Instant::now()) > Duration::ZERO,
            "429 must arm the backoff"
        );
        client.budget.success(); // disarm so the next call doesn't sleep

        let err = client.search(&Query::default()).unwrap_err();
        assert!(format!("{err:?}").contains("wallhaven unreachable"));
    }

    #[test]
    fn api_key_read_from_config_toml() {
        let dir = tmp_dir("key");
        let cfg = dir.join("config.toml");
        assert_eq!(api_key_from(&cfg), None, "missing file is fine");
        std::fs::write(&cfg, "[wallhaven]\napi_key = \"abc123\"\n").unwrap();
        assert_eq!(api_key_from(&cfg), Some("abc123".into()));
        std::fs::write(&cfg, "[wallhaven]\napi_key = \"\"\n").unwrap();
        assert_eq!(api_key_from(&cfg), None, "empty key = no key");
    }

    #[test]
    fn saving_a_key_round_trips_and_keeps_other_settings() {
        let dir = tmp_dir("key-save");
        let cfg = dir.join("config.toml");

        // Writes into a file that doesn't exist yet, as a real section rather
        // than the inline `wallhaven = { .. }` that indexing alone produces.
        save_api_key_to(&cfg, "abc123").unwrap();
        assert_eq!(api_key_from(&cfg), Some("abc123".into()));
        assert!(
            std::fs::read_to_string(&cfg)
                .unwrap()
                .contains("[wallhaven]"),
            "expected a [wallhaven] section"
        );

        // Someone else's settings must survive a key write — the config is
        // shared with [targets] and [themesync].
        let text = std::fs::read_to_string(&cfg).unwrap();
        std::fs::write(
            &cfg,
            format!("# my notes\n[themesync]\ntools = [\"fzf\"]\n\n{text}"),
        )
        .unwrap();
        save_api_key_to(&cfg, "  xyz789  ").unwrap();
        let after = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(api_key_from(&cfg), Some("xyz789".into()), "trimmed");
        assert!(after.contains("# my notes"), "{after}");
        assert!(after.contains("tools = [\"fzf\"]"), "{after}");
    }

    #[test]
    fn saving_an_empty_key_clears_it() {
        let dir = tmp_dir("key-clear");
        let cfg = dir.join("config.toml");
        save_api_key_to(&cfg, "abc123").unwrap();
        save_api_key_to(&cfg, "   ").unwrap();
        assert_eq!(api_key_from(&cfg), None);
        // Cleared, not stored as an empty string that would be sent as a key.
        assert!(!std::fs::read_to_string(&cfg).unwrap().contains("api_key"));
    }

    #[test]
    fn a_broken_config_is_reported_not_overwritten() {
        let dir = tmp_dir("key-broken");
        let cfg = dir.join("config.toml");
        std::fs::write(&cfg, "this is not [ valid toml\n").unwrap();
        assert!(save_api_key_to(&cfg, "abc123").is_err());
        // The user's file is left exactly as it was for them to fix.
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            "this is not [ valid toml\n"
        );
    }

    /// Live smoke test — network, excluded from CI. Run manually with
    /// `cargo test -p studio-core wallhaven::tests::live -- --ignored`.
    #[test]
    #[ignore = "hits the real wallhaven API"]
    fn live_search_smoke() {
        let mut client = Client::online();
        let page = client
            .search(&Query {
                q: "landscape".into(),
                ..Query::default()
            })
            .unwrap();
        assert!(!page.data.is_empty());
        assert!(page.data[0].path.starts_with("https://"));
    }
}

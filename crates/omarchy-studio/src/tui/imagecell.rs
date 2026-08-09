//! In-terminal image previews (spec 07 §4, roadmap 0.6.4).
//!
//! One [`ImageCell`] lives on the App and is shared by every screen that
//! previews an image. The terminal's graphics protocol is probed once at
//! startup (kitty → sixel → half-block cells; half-blocks work everywhere,
//! so a "no graphics" terminal still gets a coarse preview). When even the
//! probe fails — not a tty, hostile terminal — screens fall back to text and
//! the `o`-opens-in-imv path (spec 06 §4 degradation table).
//!
//! The probe is deliberately *not* `Picker::from_query_stdio()`: that spawns
//! a thread which reads the tty for query responses, and when the terminal
//! doesn't answer everything (tmux) the thread never exits and steals
//! keystrokes from crossterm for the rest of the session. Environment
//! sniffing + the `TIOCGWINSZ` pixel size cover the terminals Omarchy users
//! actually run, with zero stdin reads.
//!
//! Decoding happens on one long-lived worker thread so navigation never
//! blocks; a screen renders its own text fallback until the image is ready.
//!
//! Decoded images are kept in a small path-keyed cache rather than a single
//! "current" slot, because one frame legitimately draws more than one image:
//! the wallhaven and community browsers are modals over `cols[1]`, so
//! `draw_panel` renders the Wallpapers preview and the browser then renders
//! its thumbnail — same `ImageCell`, same frame, two paths (tui/mod.rs). With
//! one slot those two evict each other every frame, so neither ever settles
//! and the pane flickers between them with no decode ever finishing.
//!
//! The cache is deliberately tiny: an entry holds the full-resolution image
//! so the protocol can re-resize, and wallpapers are routinely 4K.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use ratatui::layout::Rect;
use ratatui::Frame;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

/// Decoded images held at once. Three covers the deepest real stack — the
/// Wallpapers preview under a browser modal, plus one in hand while another
/// decodes — without holding several 4K images resident.
const CACHE_CAP: usize = 3;

/// Decodes queued at once. Beyond this a `render` skips the request and asks
/// again next frame, which is what bounds a held-down `j`: the backlog stays
/// short and the retry naturally asks for whatever is selected *now* rather
/// than working through everything scrolled past.
const MAX_IN_FLIGHT: usize = 3;

/// Paths remembered as undecodable. Bounded so a directory of broken files
/// can't grow this without limit.
const FAILED_CAP: usize = 32;

/// What a [`ImageCell::render`] call managed to put on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// The image is on screen; the caller draws nothing.
    Drawn,
    /// Decode is in flight — a later frame will draw it. Callers that show a
    /// message should say "loading", not "broken".
    Decoding,
    /// The file didn't decode and won't be retried.
    Failed,
}

pub struct ImageCell {
    picker: Picker,
    /// Decoded protocols, least-recently-drawn first.
    cache: Vec<(PathBuf, StatefulProtocol)>,
    /// Paths that didn't decode — don't retry them every frame.
    failed: Vec<PathBuf>,
    /// Paths the worker still owes us an answer for.
    pending: Vec<PathBuf>,
    decoder: Decoder,
}

/// Handle to the decode worker: paths out, decoded images back.
struct Decoder {
    tx: mpsc::Sender<PathBuf>,
    rx: mpsc::Receiver<(PathBuf, Option<image::DynamicImage>)>,
}

impl Decoder {
    fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, res_rx) = mpsc::channel();
        thread::spawn(move || {
            // Every request gets an answer, in order. Nothing is coalesced
            // away: callers dedupe by path and won't re-ask for one already
            // in flight, so a dropped request would strand that path forever.
            // Backlog is bounded by MAX_IN_FLIGHT on the sending side.
            while let Ok(path) = req_rx.recv() {
                let img = image::open(&path).ok();
                if res_tx.send((path, img)).is_err() {
                    break;
                }
            }
        });
        Self {
            tx: req_tx,
            rx: res_rx,
        }
    }
}

/// Graphics protocol from the environment, never from a tty round-trip.
/// Conservative: inside tmux always half-blocks (passthrough is off by
/// default and detection would need the racy stdin query).
fn detect_protocol() -> ProtocolType {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let term = var("TERM");
    if var("TMUX").is_empty() {
        if !var("KITTY_WINDOW_ID").is_empty() || term.contains("kitty") || term.contains("ghostty")
        {
            return ProtocolType::Kitty;
        }
        if var("TERM_PROGRAM") == "WezTerm" {
            return ProtocolType::Iterm2;
        }
        if term.contains("foot") {
            return ProtocolType::Sixel;
        }
    }
    ProtocolType::Halfblocks
}

impl ImageCell {
    /// Probe once at startup. Cell pixel size comes from the window-size
    /// ioctl where the terminal reports it; otherwise a common 8×16 guess —
    /// only aspect correction depends on it.
    pub fn probe() -> Self {
        let font_size = match ratatui::crossterm::terminal::window_size() {
            Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => {
                (ws.width / ws.columns, ws.height / ws.rows)
            }
            _ => (8, 16),
        };
        let mut picker = Picker::from_fontsize(font_size);
        picker.set_protocol_type(detect_protocol());
        Self {
            picker,
            cache: Vec::new(),
            failed: Vec::new(),
            pending: Vec::new(),
            decoder: Decoder::spawn(),
        }
    }

    /// Human name for the Doctor screen's capability line.
    pub fn protocol_label(&self) -> &'static str {
        match self.picker.protocol_type() {
            ProtocolType::Kitty => "kitty graphics",
            ProtocolType::Iterm2 => "iTerm2 graphics",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Halfblocks => "half-block cells",
        }
    }

    /// Draw `path` scaled into `area`. When nothing was drawn the caller
    /// renders its own fallback, and the variant says which one: a file still
    /// decoding is not a file that failed to decode. Never draws some *other*
    /// path's image — two screens share this cell within one frame, so a stale
    /// draw would put one screen's wallpaper in the other's pane.
    pub fn render(&mut self, f: &mut Frame, area: Rect, path: &Path) -> Preview {
        self.poll();

        if let Some(i) = self.cache.iter().position(|(p, _)| p == path) {
            // Move to the back: least-recently-drawn is evicted first, and
            // the modal underneath redraws every frame, so it stays resident.
            let entry = self.cache.remove(i);
            self.cache.push(entry);
            let (_, protocol) = self.cache.last_mut().expect("just pushed");
            f.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(None)),
                area,
                protocol,
            );
            return Preview::Drawn;
        }

        if self.failed.iter().any(|p| p == path) {
            return Preview::Failed;
        }
        self.request(path);
        Preview::Decoding
    }

    /// Queue a decode unless it's already queued or the backlog is full.
    fn request(&mut self, path: &Path) {
        if self.pending.iter().any(|p| p == path) || self.pending.len() >= MAX_IN_FLIGHT {
            return;
        }
        let owned = path.to_path_buf();
        if self.decoder.tx.send(owned.clone()).is_err() {
            // Worker died; nothing will ever arrive for this path.
            self.remember_failed(owned);
            return;
        }
        self.pending.push(owned);
    }

    /// Drain finished decodes. The event loop calls this too: a decode kicked
    /// off on a screen the user then left still has to clear `pending`, or the
    /// loop keeps polling instead of going back to a blocking read.
    pub fn poll(&mut self) {
        while let Ok((path, img)) = self.decoder.rx.try_recv() {
            self.pending.retain(|p| p != &path);
            match img {
                Some(img) => {
                    let protocol = self.picker.new_resize_protocol(img);
                    self.cache.retain(|(p, _)| p != &path);
                    if self.cache.len() >= CACHE_CAP {
                        self.cache.remove(0);
                    }
                    self.cache.push((path, protocol));
                }
                None => self.remember_failed(path),
            }
        }
    }

    fn remember_failed(&mut self, path: PathBuf) {
        if self.failed.iter().any(|p| p == &path) {
            return;
        }
        if self.failed.len() >= FAILED_CAP {
            self.failed.remove(0);
        }
        self.failed.push(path);
    }

    /// True while a decode is in flight — the event loop polls instead of
    /// blocking so the preview appears as soon as it lands.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "omarchy-studio-imagecell-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn self_cached(cell: &ImageCell, path: &Path) -> bool {
        cell.cache.iter().any(|(p, _)| p == path)
    }

    /// Write a tiny valid PNG so the worker has something real to decode.
    fn png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        image::RgbImage::new(4, 4).save(&path).unwrap();
        path
    }

    #[test]
    fn worker_decodes_and_reports_the_path_it_decoded() {
        let dir = scratch("ok");
        let path = png(&dir, "wall.png");
        let decoder = Decoder::spawn();
        decoder.tx.send(path.clone()).unwrap();
        let (got, img) = decoder.rx.recv().unwrap();
        assert_eq!(got, path);
        assert!(img.is_some());
    }

    #[test]
    fn undecodable_file_comes_back_as_none_rather_than_hanging() {
        let dir = scratch("bad");
        let path = dir.join("not-an-image.png");
        std::fs::write(&path, b"definitely not a png").unwrap();
        let decoder = Decoder::spawn();
        decoder.tx.send(path.clone()).unwrap();
        let (got, img) = decoder.rx.recv().unwrap();
        assert_eq!(got, path);
        assert!(img.is_none());
    }

    /// Every request is answered, in order. `render` won't re-ask for a path
    /// already in `pending`, so a request the worker dropped would leave that
    /// path pending forever and its screen stuck on the text fallback.
    #[test]
    fn every_queued_request_is_answered_in_order() {
        let dir = scratch("queue");
        let queued: Vec<PathBuf> = ["a.png", "b.png", "c.png", "d.png", "z.png"]
            .iter()
            .map(|n| png(&dir, n))
            .collect();

        let decoder = Decoder::spawn();
        for path in &queued {
            decoder.tx.send(path.clone()).unwrap();
        }

        for want in &queued {
            let (got, img) = decoder.rx.recv().unwrap();
            assert_eq!(&got, want);
            assert!(img.is_some(), "{got:?} should have decoded");
        }
    }

    /// Drives the frame shape that broke wallhaven previews: `draw_panel`
    /// renders the Wallpapers preview into `cols[1]`, then the browser modal
    /// renders its thumbnail into the same rect. Two paths, one cell, one
    /// frame — with a single `current` slot they evicted each other forever
    /// and the modal's pane never settled.
    #[test]
    fn two_screens_in_one_frame_both_get_their_own_image() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let dir = scratch("twoup");
        let under = png(&dir, "wallpaper.png");
        let modal = png(&dir, "thumb.png");

        let mut cell = ImageCell::probe();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();

        let mut both = false;
        for _ in 0..50 {
            let (mut a, mut b) = (Preview::Decoding, Preview::Decoding);
            term.draw(|f| {
                a = cell.render(f, Rect::new(0, 0, 40, 20), &under);
                b = cell.render(f, Rect::new(40, 0, 40, 20), &modal);
            })
            .unwrap();
            if a == Preview::Drawn && b == Preview::Drawn {
                both = true;
                break;
            }
            cell.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(both, "both panes should settle on their own image");
        // Both really are resident — a `true` here means this path's own
        // image was drawn, not whatever the other screen last decoded.
        assert!(self_cached(&cell, &under) && self_cached(&cell, &modal));

        // And they stay settled — no eviction war on subsequent frames.
        for _ in 0..5 {
            let (mut a, mut b) = (Preview::Decoding, Preview::Decoding);
            term.draw(|f| {
                a = cell.render(f, Rect::new(0, 0, 40, 20), &under);
                b = cell.render(f, Rect::new(40, 0, 40, 20), &modal);
            })
            .unwrap();
            assert_eq!(
                (a, b),
                (Preview::Drawn, Preview::Drawn),
                "an image was evicted by the other screen"
            );
        }
        assert!(!cell.has_pending(), "loop would keep polling forever");
    }

    /// A path that fails to decode is remembered, so the worker isn't asked
    /// again on every frame.
    #[test]
    fn a_failed_path_is_not_requeued_every_frame() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let dir = scratch("failed");
        let path = dir.join("broken.png");
        std::fs::write(&path, b"not an image").unwrap();

        let mut cell = ImageCell::probe();
        let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
        for _ in 0..50 {
            term.draw(|f| {
                cell.render(f, Rect::new(0, 0, 40, 12), &path);
            })
            .unwrap();
            if cell.failed.iter().any(|p| p == &path) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            cell.failed.iter().any(|p| p == &path),
            "failure not recorded"
        );

        let mut got = Preview::Drawn;
        term.draw(|f| {
            got = cell.render(f, Rect::new(0, 0, 40, 12), &path);
        })
        .unwrap();
        assert_eq!(
            got,
            Preview::Failed,
            "caller must be told it failed, not that it's still loading"
        );
        assert!(!cell.has_pending(), "a known-bad path was queued again");
    }
}

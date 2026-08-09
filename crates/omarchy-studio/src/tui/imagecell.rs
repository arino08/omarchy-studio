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
//! blocks. The UI keeps showing the previous image (or nothing) until the new
//! one is ready. The worker coalesces: holding `j` through a wallpaper list
//! queues paths faster than a 4K JPEG decodes, so it drains to the newest
//! request and drops the ones already scrolled past — one decode in flight,
//! not one per keypress.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use ratatui::layout::Rect;
use ratatui::Frame;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

pub struct ImageCell {
    picker: Picker,
    /// Protocol state for the image currently on screen.
    current: Option<(PathBuf, StatefulProtocol)>,
    /// Last path that failed to decode — don't retry every frame.
    failed: Option<PathBuf>,
    /// Path the worker is decoding right now, if any.
    pending: Option<PathBuf>,
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
            while let Ok(mut path) = req_rx.recv() {
                // Skip anything the user has already scrolled past.
                while let Ok(newer) = req_rx.try_recv() {
                    path = newer;
                }
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
            current: None,
            failed: None,
            pending: None,
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

    /// Draw `path` scaled into `area`. Returns false when the file didn't
    /// decode — caller renders its text fallback instead.
    pub fn render(&mut self, f: &mut Frame, area: Rect, path: &Path) -> bool {
        if self.failed.as_deref() == Some(path) {
            return false;
        }

        self.poll();

        // Decoded and on screen already — just draw it.
        if self.current.as_ref().map(|(p, _)| p.as_path()) == Some(path) {
            self.draw(f, area);
            return true;
        }
        // The decode we just collected may have been a failure for this path.
        if self.failed.as_deref() == Some(path) {
            return false;
        }

        // Not decoded and not already queued — ask the worker for it.
        if self.pending.as_deref() != Some(path) {
            let owned = path.to_path_buf();
            if self.decoder.tx.send(owned.clone()).is_err() {
                // Worker died; nothing will ever arrive for this path.
                self.failed = Some(owned);
                return false;
            }
            self.pending = Some(owned);
        }

        // Show the previous image (stale but instant) while the decode runs.
        self.draw(f, area);
        self.current.is_some()
    }

    /// Drain finished decodes. Results for paths the user has scrolled past
    /// are dropped — only the one still pending can become `current`. The
    /// event loop calls this too: a decode kicked off on a screen the user
    /// then left still has to clear `pending`, or the loop keeps polling
    /// instead of going back to a blocking read.
    pub fn poll(&mut self) {
        while let Ok((path, img)) = self.decoder.rx.try_recv() {
            if self.pending.as_deref() != Some(path.as_path()) {
                continue;
            }
            self.pending = None;
            match img {
                Some(img) => {
                    self.current = Some((path, self.picker.new_resize_protocol(img)));
                    self.failed = None;
                }
                None => {
                    self.failed = Some(path);
                    self.current = None;
                }
            }
        }
    }

    /// True while a decode is in flight — the event loop polls instead of
    /// blocking so the preview appears as soon as it lands.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn draw(&mut self, f: &mut Frame, area: Rect) {
        if let Some((_, protocol)) = self.current.as_mut() {
            f.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(None)),
                area,
                protocol,
            );
        }
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

    /// Holding `j` queues paths faster than they decode. Whether any given
    /// run actually coalesces is a race (4×4 PNGs decode faster than the test
    /// can queue them), so what's asserted is the contract `poll` relies on:
    /// replies never reorder, and the run ends on the newest request — a
    /// dropped intermediate is a skipped reply, never a late one.
    #[test]
    fn decode_replies_stay_in_order_and_end_on_the_newest_path() {
        let dir = scratch("coalesce");
        let queued: Vec<PathBuf> = ["a.png", "b.png", "c.png", "d.png", "z.png"]
            .iter()
            .map(|n| png(&dir, n))
            .collect();
        let last = queued.last().unwrap().clone();

        let decoder = Decoder::spawn();
        for path in &queued {
            decoder.tx.send(path.clone()).unwrap();
        }

        let mut seen = Vec::new();
        loop {
            let (path, img) = decoder.rx.recv().unwrap();
            assert!(img.is_some(), "{path:?} should have decoded");
            let done = path == last;
            seen.push(path);
            if done {
                break;
            }
        }
        assert!(seen.len() <= queued.len());
        assert_eq!(seen.last().unwrap(), &last);
        // Replies are a subsequence of the send order — the worker only ever
        // skips ahead, so nothing stale can land after something newer.
        let mut want = queued.iter();
        for got in &seen {
            assert!(want.any(|q| q == got), "{got:?} arrived out of order");
        }
    }
}

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
//! Foot is the exception to `TERM` sniffing: Omarchy's `foot.ini` sets
//! `term=xterm-256color` and foot unsets `TERM_PROGRAM`, so nothing in the
//! environment names it. It is recognised from `/proc` instead, as the process
//! that started this tty's session, which reads nothing from the terminal.
//!
//! The cell size is re-measured on every draw rather than kept from startup.
//! `foot -e` starts its child before the window is configured, so the first
//! reading can be twice the real size (13×28 against a true 6×15), and sixel
//! images sized from it overrun their pane and scroll the terminal. Half-blocks
//! ignore the cell size, which is why a stale reading never showed there.
//!
//! A sixel is written once through its top-left cell and the rest of its cells
//! are skipped, so an overlay drawn over it leaves a hole that the cell diff
//! never refills. [`ImageCell::finish_frame`] spots the overlay going away and
//! the event loop repaints the screen.
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

use ratatui::buffer::Buffer;
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
    /// Where sixels landed in the frame being drawn.
    drawn: Vec<Rect>,
    /// Something was drawn over a sixel in the last finished frame.
    covered: bool,
    /// An overlay has just left a hole in a sixel.
    exposed: bool,
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
    protocol_for(
        |k| std::env::var(k).unwrap_or_default(),
        host_terminal(Path::new("/proc")).as_deref(),
    )
}

/// The decision behind [`detect_protocol`]. `host` is the process name of the
/// terminal that owns this tty, when it could be found.
fn protocol_for(var: impl Fn(&str) -> String, host: Option<&str>) -> ProtocolType {
    let term = var("TERM");
    if var("TMUX").is_empty() {
        if !var("KITTY_WINDOW_ID").is_empty() || term.contains("kitty") || term.contains("ghostty")
        {
            return ProtocolType::Kitty;
        }
        if var("TERM_PROGRAM") == "WezTerm" {
            return ProtocolType::Iterm2;
        }
        if term.contains("foot") || host == Some("foot") {
            return ProtocolType::Sixel;
        }
    }
    ProtocolType::Halfblocks
}

/// Name of the process that started this tty's session: the terminal emulator
/// when the session leader is its direct child. A multiplexer or an editor's
/// embedded terminal starts its own sessions, so it reports itself rather than
/// the foot window around it, and that foot can't receive our sixels anyway.
fn host_terminal(proc_root: &Path) -> Option<String> {
    let stat = |pid: &str| std::fs::read_to_string(proc_root.join(pid).join("stat")).ok();
    let (_, session) = stat_fields(&stat("self")?)?;
    let (terminal, _) = stat_fields(&stat(&session.to_string())?)?;
    let comm = std::fs::read_to_string(proc_root.join(terminal.to_string()).join("comm")).ok()?;
    Some(comm.trim_end().to_string())
}

/// `(ppid, session)` from a `/proc/<pid>/stat` line. Fields are counted from
/// the last `)` because the command name may itself hold spaces and parens.
fn stat_fields(stat: &str) -> Option<(u32, u32)> {
    let (_, rest) = stat.rsplit_once(") ")?;
    let mut fields = rest.split_whitespace();
    let ppid = fields.nth(1)?.parse().ok()?;
    let session = fields.nth(1)?.parse().ok()?;
    Some((ppid, session))
}

/// Pixel size of one cell from the window-size ioctl, when the terminal reports it.
fn cell_size() -> Option<(u16, u16)> {
    match ratatui::crossterm::terminal::window_size() {
        Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => {
            Some((ws.width / ws.columns, ws.height / ws.rows))
        }
        _ => None,
    }
}

fn picker_for(font_size: (u16, u16), protocol: ProtocolType) -> Picker {
    let mut picker = Picker::from_fontsize(font_size);
    picker.set_protocol_type(protocol);
    picker
}

impl ImageCell {
    /// Probe once at startup. Cell pixel size comes from the window-size
    /// ioctl where the terminal reports it; otherwise a common 8×16 guess.
    /// [`Self::render`] keeps it current.
    pub fn probe() -> Self {
        Self {
            picker: picker_for(cell_size().unwrap_or((8, 16)), detect_protocol()),
            cache: Vec::new(),
            failed: Vec::new(),
            pending: Vec::new(),
            decoder: Decoder::spawn(),
            drawn: Vec::new(),
            covered: false,
            exposed: false,
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
        self.follow_cell_size(cell_size());
        self.poll();

        if let Some(i) = self.cache.iter().position(|(p, _)| p == path) {
            // Move to the back: least-recently-drawn is evicted first, and
            // the modal underneath redraws every frame, so it stays resident.
            let entry = self.cache.remove(i);
            self.cache.push(entry);
            let is_sixel = matches!(self.picker.protocol_type(), ProtocolType::Sixel);
            let (_, protocol) = self.cache.last_mut().expect("just pushed");
            if is_sixel {
                let size = protocol.size_for(Resize::Fit(None), area);
                self.drawn.push(Rect {
                    x: area.x,
                    y: area.y,
                    ..size
                });
            }
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

    /// Call with the finished frame, overlays included. Notes when something
    /// that covered a sixel in the last frame is gone in this one.
    pub fn finish_frame(&mut self, buf: &Buffer) {
        let covered = self.drawn.drain(..).any(|rect| {
            rect.positions()
                .skip(1)
                .any(|pos| buf.cell(pos).is_some_and(|cell| !cell.skip))
        });
        self.exposed |= self.covered && !covered;
        self.covered = covered;
    }

    /// True once after an overlay leaves a sixel: the screen needs a full
    /// repaint, because no cell in the hole differs from the last frame.
    pub fn take_exposed(&mut self) -> bool {
        std::mem::take(&mut self.exposed)
    }

    /// Adopt a new cell size. Cached protocols are bound to the old one and
    /// hand their source image to nobody, so they are dropped and re-decoded.
    /// `None` means nothing was measured, which is no reason to change.
    fn follow_cell_size(&mut self, measured: Option<(u16, u16)>) {
        let Some(size) = measured else { return };
        if size == self.picker.font_size() {
            return;
        }
        self.picker = picker_for(size, self.picker.protocol_type());
        self.cache.clear();
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

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> String + 'a {
        move |k| {
            vars.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
                .unwrap_or_default()
        }
    }

    /// Omarchy's stock foot: `TERM` says xterm, so only the tty's owner tells.
    #[test]
    fn foot_is_found_when_term_does_not_name_it() {
        let stock = [("TERM", "xterm-256color")];
        assert_eq!(protocol_for(env(&stock), Some("foot")), ProtocolType::Sixel);
        assert_eq!(
            protocol_for(env(&stock), Some("herdr")),
            ProtocolType::Halfblocks
        );
        assert_eq!(protocol_for(env(&stock), None), ProtocolType::Halfblocks);
        assert_eq!(
            protocol_for(env(&[("TERM", "foot")]), None),
            ProtocolType::Sixel
        );
    }

    #[test]
    fn tmux_stays_on_half_blocks_inside_foot() {
        let tmux = [("TERM", "foot"), ("TMUX", "/tmp/tmux-1000/default,1,0")];
        assert_eq!(
            protocol_for(env(&tmux), Some("foot")),
            ProtocolType::Halfblocks
        );
    }

    #[test]
    fn stat_fields_survive_awkward_command_names() {
        let stat = "42 (a) b (c)) S 7 42 9 34816 42 4194304";
        assert_eq!(stat_fields(stat), Some((7, 9)));
        assert_eq!(stat_fields("garbage"), None);
    }

    fn proc_entry(root: &Path, pid: &str, stat: &str, comm: &str) {
        let dir = root.join(pid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stat"), stat).unwrap();
        std::fs::write(dir.join("comm"), comm).unwrap();
    }

    /// studio (300) in a shell's session (200), the shell started by foot (100).
    #[test]
    fn host_terminal_is_the_parent_of_the_session_leader() {
        let root = scratch("proc");
        proc_entry(&root, "self", "300 (studio) S 200 300 200 0", "studio\n");
        proc_entry(&root, "200", "200 (bash) S 100 200 200 0", "bash\n");
        proc_entry(&root, "100", "100 (foot) S 1 100 100 0", "foot\n");
        assert_eq!(host_terminal(&root).as_deref(), Some("foot"));
    }

    /// A pane inside a multiplexer reports the multiplexer, never the foot
    /// window its client happens to run in.
    #[test]
    fn host_terminal_stops_at_the_multiplexer() {
        let root = scratch("proc-mux");
        proc_entry(&root, "self", "300 (studio) S 200 300 200 0", "studio\n");
        proc_entry(&root, "200", "200 (bash) S 150 200 200 0", "bash\n");
        proc_entry(&root, "150", "150 (herdr) S 1 150 150 0", "herdr\n");
        assert_eq!(host_terminal(&root).as_deref(), Some("herdr"));
    }

    #[test]
    fn host_terminal_is_none_when_the_session_leader_is_gone() {
        let root = scratch("proc-gone");
        proc_entry(&root, "self", "300 (studio) S 200 300 200 0", "studio\n");
        assert_eq!(host_terminal(&root), None);
    }

    /// `foot -e` hands the child a 13×28 reading before the window settles at
    /// 6×15. Keeping the first one drew sixels twice too large, overrunning
    /// the pane and scrolling the terminal.
    #[test]
    fn a_new_cell_size_replaces_the_startup_reading() {
        let dir = scratch("cell");
        let path = png(&dir, "wall.png");
        let mut cell = ImageCell::probe();
        cell.picker = picker_for((13, 28), ProtocolType::Sixel);
        let image = image::open(&path).unwrap();
        cell.cache
            .push((path.clone(), cell.picker.new_resize_protocol(image)));

        cell.follow_cell_size(Some((6, 15)));

        assert_eq!(cell.picker.font_size(), (6, 15));
        assert_eq!(cell.picker.protocol_type(), ProtocolType::Sixel);
        assert!(
            !self_cached(&cell, &path),
            "a protocol sized for the old cell would still be drawn"
        );
    }

    #[test]
    fn no_measurement_keeps_the_size_and_the_cache() {
        let dir = scratch("nomeasure");
        let path = png(&dir, "wall.png");
        let mut cell = ImageCell::probe();
        cell.picker = picker_for((6, 15), ProtocolType::Sixel);
        let image = image::open(&path).unwrap();
        cell.cache
            .push((path.clone(), cell.picker.new_resize_protocol(image)));

        cell.follow_cell_size(None);
        cell.follow_cell_size(Some((6, 15)));

        assert_eq!(cell.picker.font_size(), (6, 15));
        assert!(self_cached(&cell, &path));
    }

    /// Closing Help over a sixel left a torn piece of it in the image until
    /// the user changed screens: the frame after an overlay must ask for a
    /// repaint, once, and frames with nothing covering the image must not.
    #[test]
    fn an_overlay_leaving_a_sixel_asks_for_one_repaint() {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Clear;
        use ratatui::Terminal;

        let dir = scratch("overlay");
        let path = dir.join("wall.png");
        image::RgbImage::from_fn(240, 300, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        })
        .save(&path)
        .unwrap();

        let mut cell = ImageCell::probe();
        cell.picker = picker_for((6, 15), ProtocolType::Sixel);
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let pane = Rect::new(0, 0, 40, 20);
        let overlay = Rect::new(10, 5, 10, 5);

        let mut frame = |cell: &mut ImageCell, with_overlay: bool| {
            let mut state = Preview::Decoding;
            term.draw(|f| {
                state = cell.render(f, pane, &path);
                if with_overlay {
                    f.render_widget(Clear, overlay);
                }
                cell.finish_frame(f.buffer_mut());
            })
            .unwrap();
            state
        };

        for _ in 0..200 {
            if frame(&mut cell, false) == Preview::Drawn {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(frame(&mut cell, false), Preview::Drawn);
        assert!(!cell.take_exposed(), "nothing has covered the image yet");

        frame(&mut cell, true);
        frame(&mut cell, true);
        assert!(!cell.take_exposed(), "the overlay is still up");

        frame(&mut cell, false);
        assert!(cell.take_exposed(), "the hole would stay on screen");
        assert!(!cell.take_exposed(), "one repaint is enough");

        frame(&mut cell, false);
        assert!(!cell.take_exposed(), "a settled frame must not repaint");
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

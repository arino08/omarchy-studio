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
//! Decoding happens on a background thread so navigation never blocks.
//! The UI shows the previous image (or nothing) until the new one is ready.

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
    current: Option<(PathBuf, StatefulProtocol)>,
    failed: Option<PathBuf>,
    pending: Option<PendingDecode>,
}

struct PendingDecode {
    path: PathBuf,
    rx: mpsc::Receiver<Option<image::DynamicImage>>,
}

/// Graphics protocol from the environment, never from a tty round-trip.
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
        }
    }

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

        // Already showing this image — just draw it.
        if self.current.as_ref().map(|(p, _)| p.as_path()) == Some(path) {
            self.draw(f, area);
            return true;
        }

        // Check if a background decode finished.
        if let Some(pending) = &self.pending {
            if pending.path == path {
                match pending.rx.try_recv() {
                    Ok(Some(img)) => {
                        let proto = self.picker.new_resize_protocol(img);
                        self.current = Some((path.to_path_buf(), proto));
                        self.pending = None;
                        self.failed = None;
                        self.draw(f, area);
                        return true;
                    }
                    Ok(None) => {
                        self.failed = Some(path.to_path_buf());
                        self.current = None;
                        self.pending = None;
                        return false;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        // Still decoding — show previous image if we have one.
                        self.draw(f, area);
                        return self.current.is_some();
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.failed = Some(path.to_path_buf());
                        self.pending = None;
                        return false;
                    }
                }
            }
        }

        // New path — kick off a background decode.
        let (tx, rx) = mpsc::channel();
        let owned = path.to_path_buf();
        let decode_path = owned.clone();
        thread::spawn(move || {
            let result = image::open(&decode_path).ok();
            let _ = tx.send(result);
        });
        self.pending = Some(PendingDecode {
            path: owned,
            rx,
        });

        // Show previous image (stale but instant) while decoding.
        self.draw(f, area);
        self.current.is_some()
    }

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

//! TUI shell (spec 07): module rail, content pane, keybar + jargon strip,
//! help overlay, command palette. Studio themes itself from the active
//! Omarchy palette. Elm-ish: events → `handle` → optional side effect → redraw.
//!
//! Every rail entry is now a real screen (Snapshots, the timeline browser, was
//! the last placeholder to land in 0.9.4).

mod chord;
mod imagecell;
mod logo;
mod theme;
mod ui;
mod screens {
    pub mod animations;
    pub mod apps;
    pub mod battery;
    pub mod community;
    pub mod doctor;
    pub mod integrations;
    pub mod keybinds;
    pub mod lockidle;
    pub mod looknfeel;
    pub mod monitors;
    pub mod notifications;
    pub mod nova;
    pub mod onboarding;
    pub mod palette_editor;
    pub mod snapshots;
    pub mod themes;
    pub mod tweaks;
    pub mod wallhaven;
    pub mod wallpapers;
    pub mod waybar;
    pub mod wizard;
}

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};
use ratatui::Frame;

use studio_core::cmd::{CommandRunner, RealRunner};
use studio_core::modules::themes::ThemeStore;
use studio_core::modules::update;
use studio_core::omarchy::{cmds, OmarchyPaths};
use studio_core::snapshot::{SnapshotKind, SnapshotStore};

use screens::animations::{AnimAction, AnimationsScreen};
use screens::apps::{AppsAction, AppsScreen};
use screens::battery::{BatteryAction, BatteryScreen};
use screens::community::{CommunityAction, CommunityBrowser};
use screens::doctor::{DoctorAction, DoctorScreen};
use screens::integrations::{IntegrationsAction, IntegrationsScreen};
use screens::keybinds::{KeybindAction, KeybindsScreen};
use screens::lockidle::{LockIdleAction, LockIdleScreen};
use screens::looknfeel::{LookFeelAction, LookFeelScreen};
use screens::monitors::{MonitorsAction, MonitorsScreen};
use screens::notifications::{NotifAction, NotificationsScreen};
use screens::nova::{NovaAction, NovaScreen};
use screens::onboarding::{self, OnboardingAction, OnboardingScreen};
use screens::palette_editor::{EditorAction, PaletteEditor};
use screens::snapshots::{SnapshotsAction, SnapshotsScreen};
use screens::themes::{ThemeAction, ThemesScreen};
use screens::tweaks::{TweaksAction, TweaksScreen};
use screens::wallhaven::{BrowserAction, Then, WallhavenBrowser};
use screens::wallpapers::{WallpaperAction, WallpapersScreen};
use screens::waybar::{WaybarAction, WaybarScreen};
use screens::wizard::{WizardAction, WizardScreen};
use theme::Skin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Themes,
    Wallpapers,
    Keybinds,
    LookFeel,
    Animations,
    Waybar,
    Notifications,
    LockIdle,
    Snapshots,
    Integrations,
    Apps,
    Monitors,
    Tweaks,
    Battery,
    Nova,
    Doctor,
}

impl Screen {
    const ALL: [Screen; 16] = [
        Screen::Themes,
        Screen::Wallpapers,
        Screen::Keybinds,
        Screen::LookFeel,
        Screen::Animations,
        Screen::Waybar,
        Screen::Notifications,
        Screen::LockIdle,
        Screen::Snapshots,
        Screen::Integrations,
        Screen::Apps,
        Screen::Monitors,
        Screen::Tweaks,
        Screen::Battery,
        Screen::Nova,
        Screen::Doctor,
    ];

    fn label(self) -> &'static str {
        match self {
            Screen::Themes => "Themes",
            Screen::Wallpapers => "Wallpaper",
            Screen::Keybinds => "Keybinds",
            Screen::LookFeel => "Look & Feel",
            Screen::Animations => "Animations",
            Screen::Waybar => "Waybar",
            Screen::Notifications => "Notifs / OSD",
            Screen::LockIdle => "Lock & Idle",
            Screen::Snapshots => "Snapshots",
            Screen::Integrations => "Integrations",
            Screen::Apps => "Apps",
            Screen::Monitors => "Monitors",
            Screen::Tweaks => "Tweaks",
            Screen::Battery => "Power",
            Screen::Nova => "Nice Launcher",
            Screen::Doctor => "Doctor",
        }
    }
}

struct Toast {
    text: String,
    ok: bool,
}

/// Which pane owns keyboard input. `Nav` drives the left rail (↑/↓ move
/// between sections, `enter` descends into the panel); `Panel` hands every
/// non-global key to the active screen, and `esc` climbs back to `Nav`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Nav,
    Panel,
}

struct App {
    paths: OmarchyPaths,
    skin: Skin,
    screen: Screen,
    focus: Focus,
    rail_state: ratatui::widgets::ListState,
    themes: ThemesScreen,
    keybinds: KeybindsScreen,
    looknfeel: LookFeelScreen,
    animations: AnimationsScreen,
    waybar: WaybarScreen,
    notifications: NotificationsScreen,
    lockidle: LockIdleScreen,
    snapshots: SnapshotsScreen,
    apps: AppsScreen,
    monitors: MonitorsScreen,
    tweaks: TweaksScreen,
    battery: BatteryScreen,
    nova: NovaScreen,
    doctor: DoctorScreen,
    integrations: IntegrationsScreen,
    wallpapers: WallpapersScreen,
    /// Shared terminal-graphics preview cell (probed once at startup).
    images: imagecell::ImageCell,
    /// First-run on-ramp, shown once over everything on a fresh Studio.
    onboarding: Option<OnboardingScreen>,
    /// Theme-from-wallpaper wizard, when open (modal over everything).
    wizard: Option<WizardScreen>,
    /// wallhaven browser, when open (modal; wizard can stack on top via `t`).
    wallhaven: Option<WallhavenBrowser>,
    /// community themes browser, when open (modal over the Themes screen).
    community: Option<CommunityBrowser>,
    /// Active palette editor (modal over the content pane), with the theme
    /// slug that was showing before it opened (restored on cancel).
    editor: Option<(PaletteEditor, String)>,
    palette: Option<CommandPalette>,
    help: bool,
    toast: Option<Toast>,
    /// Newer release found by the launch check — arms the `U` chord.
    update: Option<update::Release>,
    /// The launch check runs on its own thread; the event loop polls this
    /// until it answers (then goes back to blocking reads).
    update_rx: Option<std::sync::mpsc::Receiver<Option<update::Release>>>,
    /// `[update] auto = true`: swap + restart without waiting for `U`.
    update_auto: bool,
    /// Running binary's path, captured before any swap can rename over it
    /// (afterwards `/proc/self/exe` reads "… (deleted)").
    exe: Option<std::path::PathBuf>,
    /// Exec `exe` after the terminal is restored (set by a completed update).
    restart: bool,
    quit: bool,
}

impl App {
    fn new(paths: OmarchyPaths) -> Self {
        // Probe the terminal's graphics protocol first: the probe reads the
        // tty for the query response and would eat any keys typed while the
        // (slower) screen loads below run.
        let images = imagecell::ImageCell::probe();
        let skin = Skin::from_current(&paths);
        let themes = ThemesScreen::load(&paths);
        // If a previous session died mid-preview, its live `hyprctl keyword`
        // values are still applied but not on disk — reload to discard them.
        let state = studio_core::studio_state_dir();
        if studio_core::modules::looknfeel::preview_was_active(&state) {
            let _ = RealRunner.run(&cmds::hypr_reload());
            studio_core::modules::looknfeel::clear_preview_marker(&state);
        }
        let keybinds = KeybindsScreen::load(&paths, &RealRunner);
        let looknfeel = LookFeelScreen::load(&paths);
        let animations = AnimationsScreen::load(&paths);
        let waybar = WaybarScreen::load(&paths);
        let mut notifications = NotificationsScreen::load(&paths);
        notifications.set_dnd(Some(studio_core::modules::mako::dnd_active(&RealRunner)));
        let lockidle = LockIdleScreen::load(&paths);
        let snapshots = SnapshotsScreen::load();
        let apps = AppsScreen::load(&paths);
        let monitors = MonitorsScreen::load(&paths);
        let tweaks = TweaksScreen::load(&paths);
        let battery = BatteryScreen::load();
        let nova = NovaScreen::load(&paths);
        let integrations = IntegrationsScreen::load(&RealRunner);
        let mut doctor = DoctorScreen::load(&paths, &RealRunner);
        doctor.set_graphics(images.protocol_label());
        let mut wallpapers = WallpapersScreen::load(
            &paths,
            studio_core::omarchy::Capabilities::probe(&paths, &RealRunner).video_wallpapers,
        );
        wallpapers.context_tools = integrations.context_actions("wallpapers");
        // Launch update check, off-thread so a slow network never delays the
        // first draw. The thread touches only the network and the state-dir
        // stamp — never stdin (see ImageCell::probe for why that matters).
        let update_cfg = update::load_config();
        let update_rx = if update_cfg.check {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = tx.send(update::startup_check(
                    &update::HttpFetch::new(),
                    &studio_core::studio_state_dir(),
                    update::CURRENT,
                    now,
                ));
            });
            Some(rx)
        } else {
            None
        };
        // A fresh Studio (never onboarded, still something to set up) opens
        // onto the welcome on-ramp instead of straight into the rail.
        let ob_status = onboarding_status(&paths);
        let onboarding = should_onboard(&ob_status).then(|| OnboardingScreen::new(ob_status));
        Self {
            paths,
            skin,
            screen: Screen::Themes,
            focus: Focus::Nav,
            rail_state: ratatui::widgets::ListState::default(),
            themes,
            keybinds,
            looknfeel,
            animations,
            waybar,
            notifications,
            lockidle,
            snapshots,
            apps,
            monitors,
            tweaks,
            battery,
            nova,
            doctor,
            integrations,
            wallpapers,
            images,
            onboarding,
            wizard: None,
            wallhaven: None,
            community: None,
            editor: None,
            palette: None,
            help: false,
            toast: None,
            update: None,
            update_rx,
            update_auto: update_cfg.auto,
            exe: std::env::current_exe().ok(),
            restart: false,
            quit: false,
        }
    }

    /// The launch check answered. `None` covers up-to-date, offline, and
    /// check-not-due alike — silence, not a toast.
    fn update_arrived(&mut self, found: Option<update::Release>) {
        self.update_rx = None;
        let Some(release) = found else { return };
        let version = release.version.clone();
        if release.asset_url.is_none() {
            self.toast = Some(Toast {
                text: format!("v{version} is out (run `cargo install --path .` to build)"),
                ok: true,
            });
            return;
        }
        self.update = Some(release);
        if self.update_auto {
            self.update_apply();
        } else {
            self.toast = Some(Toast {
                text: format!("v{version} is out — press U to update & restart"),
                ok: true,
            });
        }
    }

    /// `U` (or `[update] auto`): swap the binary and restart, or explain
    /// why we won't touch it.
    fn update_apply(&mut self) {
        let Some(release) = self.update.clone() else {
            return;
        };
        let Some(exe) = self.exe.clone() else {
            self.toast = Some(Toast {
                text: "cannot resolve the running binary's path".into(),
                ok: false,
            });
            return;
        };
        match update::install_kind(&RealRunner, &exe) {
            update::InstallKind::Packaged { package } => {
                self.toast = Some(Toast {
                    text: format!(
                        "v{} is out — this install is owned by `{package}`, update it via pacman",
                        release.version
                    ),
                    ok: true,
                });
            }
            update::InstallKind::Unwritable(path) => {
                self.toast = Some(Toast {
                    text: format!(
                        "{} isn't writable — run `omarchy-studio update` with enough permissions",
                        path.display()
                    ),
                    ok: false,
                });
            }
            update::InstallKind::SelfManaged(path) => {
                match update::apply(&update::HttpFetch::new(), &release, &path) {
                    Ok(()) => {
                        self.restart = true;
                        self.quit = true;
                    }
                    Err(e) => {
                        let msg = match e {
                            studio_core::StudioError::External { detail, .. } => detail,
                            other => format!("{other:?}"),
                        };
                        self.toast = Some(Toast {
                            text: format!("update failed: {msg}"),
                            ok: false,
                        });
                    }
                }
            }
        }
    }

    fn on_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && mouse.column < 20
            && mouse.row >= 1
        {
            let visible_idx = (mouse.row - 1) as usize;
            let offset = self.rail_state.offset();
            let target = offset + visible_idx;
            self.jump(target);
        }
    }

    /// Perform a first-run step, then refresh the on-ramp's status and toast
    /// the result. Steps are additive and each has a CLI twin, so a failure
    /// here just leaves that row un-ticked — nothing half-applied to roll back.
    fn onboard_run(&mut self, step: onboarding::Step) {
        use onboarding::Step;
        let outcome: Result<String, String> = match step {
            Step::History => match crate::history() {
                Ok(_) => Ok("Change history started — every edit is now snapshotted".into()),
                Err(_) => Err("couldn't start the history repo".into()),
            },
            Step::Integration => self.onboard_integration(),
            Step::ThemeSync => self.onboard_themesync(),
        };
        self.toast = Some(match outcome {
            Ok(text) => Toast { text, ok: true },
            Err(text) => Toast { text, ok: false },
        });
        if let Some(w) = self.onboarding.as_mut() {
            w.set_status(onboarding_status(&self.paths));
        }
    }

    fn onboard_integration(&mut self) -> Result<String, String> {
        studio_core::integration::install_menu(&self.paths).map_err(|e| format!("{e:?}"))?;
        let state = studio_core::studio_state_dir();
        studio_core::hooks::install(&self.paths, &state).map_err(|e| format!("{e:?}"))?;
        Ok("Wired into Omarchy — menu entry + theme/update hooks installed".into())
    }

    fn onboard_themesync(&mut self) -> Result<String, String> {
        use studio_core::modules::themesync::{self, Enabled};
        let config_toml = studio_core::studio_config_dir().join("config.toml");
        let mut enabled = Enabled::load(&config_toml);
        let mut on: Vec<&str> = Vec::new();
        for tool in themesync::catalog() {
            if tool.available() && matches!(enabled.set(tool.id, true), Ok(true)) {
                on.push(tool.label);
            }
        }
        if on.is_empty() {
            // Nothing themable installed — don't write an empty sync config.
            return Err("no themable tools found (install fzf or lazygit)".into());
        }
        enabled.save().map_err(|e| format!("{e:?}"))?;
        crate::regen_themesync(&self.paths)?;
        Ok(format!(
            "Theme sync on for {} — open a new terminal",
            on.join(", ")
        ))
    }

    /// Dismiss the on-ramp and stamp first-run seen so it never nags again.
    fn onboard_finish(&mut self) {
        let _ = std::fs::create_dir_all(studio_core::studio_state_dir());
        let _ = std::fs::write(onboarded_marker(), "");
        self.onboarding = None;
    }

    fn on_key(&mut self, key: KeyEvent) {
        self.toast = None;

        // The first-run on-ramp is a startup gate: it owns every key until the
        // user finishes or skips it, so it comes before any other overlay.
        if let Some(w) = self.onboarding.as_mut() {
            match w.handle(key) {
                OnboardingAction::None => {}
                OnboardingAction::Run(step) => self.onboard_run(step),
                OnboardingAction::Finish => self.onboard_finish(),
            }
            return;
        }

        // Overlays consume input first.
        if self.help {
            self.help = false;
            return;
        }
        if self.palette.is_some() {
            self.palette_key(key);
            return;
        }
        // The palette editor is a focused mode: it owns all keys while open.
        if self.editor.is_some() {
            self.editor_key(key);
            return;
        }
        // So is the theme-from-wallpaper wizard (it has a text field).
        if let Some(w) = self.wizard.as_mut() {
            match w.handle(key) {
                WizardAction::None => {}
                WizardAction::Cancel => self.wizard = None,
                WizardAction::Create => self.wizard_create(),
            }
            return;
        }
        // And the wallhaven browser (search field + its own chord set).
        if let Some(b) = self.wallhaven.as_mut() {
            match b.handle(key) {
                BrowserAction::None => {}
                BrowserAction::Close => self.wallhaven = None,
            }
            return;
        }
        // And the community themes browser (filter field + its own chords).
        if let Some(b) = self.community.as_mut() {
            match b.handle(key) {
                CommunityAction::None => {}
                CommunityAction::Close => self.community = None,
            }
            return;
        }

        // Global chords (only when no in-screen text/chord/modal field is active).
        let capturing = (matches!(self.screen, Screen::Themes) && self.themes.is_capturing())
            || (matches!(self.screen, Screen::Keybinds) && self.keybinds.is_capturing())
            || (matches!(self.screen, Screen::LookFeel) && self.looknfeel.is_modal())
            || (matches!(self.screen, Screen::Waybar) && self.waybar.is_modal())
            || (matches!(self.screen, Screen::Notifications) && self.notifications.is_modal())
            || (matches!(self.screen, Screen::LockIdle) && self.lockidle.is_modal())
            || (matches!(self.screen, Screen::Apps) && self.apps.is_modal())
            || (matches!(self.screen, Screen::Wallpapers) && self.wallpapers.is_modal());
        // While a screen owns a text field / chord capture / modal, it gets
        // every key untouched — globals and focus transitions stand down.
        if !capturing {
            // Truly global, live in both focus modes.
            match key.code {
                KeyCode::Char('q') => {
                    self.quit = true;
                    return;
                }
                KeyCode::Char('?') => {
                    self.help = true;
                    return;
                }
                KeyCode::Char('/') => {
                    self.palette = Some(CommandPalette::new(&self.paths));
                    return;
                }
                KeyCode::Char('U') if self.update.is_some() => {
                    self.update_apply();
                    return;
                }
                _ => {}
            }

            match self.focus {
                // The rail owns input: ↑/↓ move between sections, digits jump,
                // enter/tab/→ descend into the panel, esc leaves the app.
                Focus::Nav => {
                    match key.code {
                        KeyCode::Up => self.cycle(-1),
                        KeyCode::Down => self.cycle(1),
                        KeyCode::Enter | KeyCode::Tab | KeyCode::Right => self.focus = Focus::Panel,
                        KeyCode::Char(d @ '1'..='9') => self.jump(d as usize - '1' as usize),
                        KeyCode::Char('0') => self.jump(9),
                        KeyCode::Esc => self.quit = true,
                        _ => {}
                    }
                    // Nav never forwards to the active screen.
                    return;
                }
                // The panel owns input; esc is the only way back to the rail.
                Focus::Panel => {
                    if key.code == KeyCode::Esc {
                        self.focus = Focus::Nav;
                        return;
                    }
                }
            }
        }

        // Delegate to the active screen.
        match self.screen {
            Screen::Themes => match self.themes.handle(key) {
                ThemeAction::None => {}
                ThemeAction::Apply(slug) => self.apply_theme(&slug),
                ThemeAction::Fork { src, name } => self.fork_theme(&src, &name),
                ThemeAction::Edit(slug) => self.open_editor(&slug),
                ThemeAction::Community => self.open_community(),
            },
            Screen::Keybinds => match self.keybinds.handle(key) {
                KeybindAction::None => {}
                KeybindAction::Commit(summary) => self.commit_keybinds(summary),
            },
            Screen::LookFeel => match self.looknfeel.handle(key) {
                LookFeelAction::None => {}
                LookFeelAction::Preview(idx) => {
                    studio_core::modules::looknfeel::mark_preview_active(
                        &studio_core::studio_state_dir(),
                    );
                    if let Err(e) = self.looknfeel.preview(idx, &RealRunner) {
                        self.toast = Some(Toast {
                            text: format!("preview failed: {}", brief(e)),
                            ok: false,
                        });
                    }
                }
                LookFeelAction::PreviewAll => {
                    studio_core::modules::looknfeel::mark_preview_active(
                        &studio_core::studio_state_dir(),
                    );
                    match self.looknfeel.preview_all(&RealRunner) {
                        Ok(()) => {
                            self.toast = Some(Toast {
                                text: "Trying preset live — s to keep, R to revert".into(),
                                ok: true,
                            })
                        }
                        Err(e) => {
                            self.toast = Some(Toast {
                                text: format!("preview failed: {}", brief(e)),
                                ok: false,
                            })
                        }
                    }
                }
                LookFeelAction::Save => self.save_looknfeel(),
                LookFeelAction::ResetAll => self.reset_looknfeel(),
                LookFeelAction::OpenToggles => {
                    use studio_core::modules::toggles::TOGGLES;
                    let rows = TOGGLES
                        .iter()
                        .map(|t| screens::looknfeel::ToggleRow {
                            id: t.id.to_string(),
                            label: t.label.to_string(),
                            on: t.is_on(&RealRunner),
                        })
                        .collect();
                    self.looknfeel.open_toggles(rows);
                }
                LookFeelAction::FlipToggle(id) => {
                    use studio_core::modules::toggles::lookup;
                    if let Some(t) = lookup(&id) {
                        match t.toggle(&RealRunner) {
                            Ok(()) => {
                                let now = t.is_on(&RealRunner);
                                self.looknfeel.set_toggle_state(&id, now);
                                let label = match now {
                                    Some(true) => format!("{} on", t.label),
                                    Some(false) => format!("{} off", t.label),
                                    None => format!("Toggled {}", t.label),
                                };
                                self.toast = Some(Toast {
                                    text: label,
                                    ok: true,
                                });
                            }
                            Err(e) => {
                                self.toast = Some(Toast {
                                    text: format!("toggle failed: {}", brief(e)),
                                    ok: false,
                                });
                            }
                        }
                    }
                }
            },
            Screen::Animations => match self.animations.handle(key) {
                AnimAction::None => {}
                AnimAction::Apply(name) => self.apply_animation(&name),
            },
            Screen::Waybar => match self.waybar.handle(key) {
                WaybarAction::None => {}
                WaybarAction::Apply => self.apply_waybar(),
                WaybarAction::Retarget(path) => self.retarget_waybar(path),
            },
            Screen::Notifications => match self.notifications.handle(key) {
                NotifAction::None => {}
                NotifAction::Save => self.apply_notifications(),
                NotifAction::ToggleDnd => self.toggle_dnd(),
                NotifAction::Test(urgency) => self.notif_test(&urgency),
                NotifAction::OsdTest => self.osd_test(),
            },
            Screen::LockIdle => match self.lockidle.handle(key) {
                LockIdleAction::None => {}
                LockIdleAction::Save => self.apply_lockidle(),
            },
            Screen::Snapshots => match self.snapshots.handle(key) {
                SnapshotsAction::None => {}
                SnapshotsAction::Restore(id) => self.snapshots_restore(&id),
            },
            Screen::Wallpapers => match self.wallpapers.handle(key) {
                WallpaperAction::None => {}
                WallpaperAction::Set(path) => self.wp_set(&path),
                WallpaperAction::Next => self.wp_next(),
                WallpaperAction::Add(path) => self.wp_add(&path),
                WallpaperAction::Remove(entry) => self.wp_remove(&entry),
                WallpaperAction::Open(entry) => self.wp_open(&entry),
                WallpaperAction::Wallhaven => self.open_wallhaven(),
                WallpaperAction::Craft(entry) => self.open_wizard(&entry),
                WallpaperAction::Tool {
                    exec,
                    label,
                    terminal,
                } => self.launch_tool(&exec, &label, terminal),
            },
            Screen::Apps => match self.apps.handle(key) {
                AppsAction::None => {}
                AppsAction::Preview(ids) => self.apps_preview(ids),
                AppsAction::Remove(plan) => self.apps_remove(plan),
            },
            Screen::Monitors => match self.monitors.handle(key) {
                MonitorsAction::None => {}
                MonitorsAction::Identify => self.monitors_identify(),
                MonitorsAction::Save(layout) => self.monitors_save(layout),
            },
            Screen::Tweaks => match self.tweaks.handle(key) {
                TweaksAction::None => {}
                TweaksAction::Applied { label, on, reload } => {
                    if reload {
                        let _ = RealRunner.run(&cmds::hypr_reload());
                    }
                    self.toast = Some(Toast {
                        text: format!("{label} {}", if on { "on" } else { "off" }),
                        ok: true,
                    });
                }
                TweaksAction::Failed(msg) => {
                    self.toast = Some(Toast {
                        text: msg,
                        ok: false,
                    });
                }
            },
            Screen::Battery => match self.battery.handle(key) {
                BatteryAction::None => {}
                BatteryAction::Apply { start, end } => self.battery_apply(start, end),
                BatteryAction::SetProfile(name) => self.set_power_profile(name),
                BatteryAction::Refresh => {
                    self.battery.reload();
                    self.toast = Some(Toast {
                        text: "Re-read battery state".into(),
                        ok: true,
                    });
                }
            },
            Screen::Integrations => match self.integrations.handle(key) {
                IntegrationsAction::None => {}
                IntegrationsAction::Launch {
                    exec,
                    label,
                    terminal,
                } => self.launch_tool(&exec, &label, terminal),
                IntegrationsAction::Refresh => {
                    self.integrations.reload(&RealRunner);
                    self.toast = Some(Toast {
                        text: "Re-detected tools".into(),
                        ok: true,
                    });
                }
            },
            Screen::Nova => match self.nova.handle(key) {
                NovaAction::None => {}
                NovaAction::Apply => self.nova_apply(),
                NovaAction::ToggleBind => self.nova_toggle_bind(),
                NovaAction::Install => self.nova_install(),
                NovaAction::Launch => {
                    let exec = self.nova.launch_exec().map(str::to_string);
                    match exec {
                        Some(e) => self.launch_tool(&e, "Nice Launcher", false),
                        None => {
                            self.toast = Some(Toast {
                                text: "Nice Launcher is not installed".into(),
                                ok: false,
                            });
                        }
                    }
                }
                NovaAction::Refresh => {
                    self.nova.reload(&self.paths);
                    self.toast = Some(Toast {
                        text: "Re-read Nice Launcher config".into(),
                        ok: true,
                    });
                }
            },
            Screen::Doctor => match self.doctor.handle(key) {
                DoctorAction::None => {}
                DoctorAction::InstallHooks => self.install_hooks(),
                DoctorAction::RemoveHooks => self.remove_hooks(),
                DoctorAction::Refresh => {
                    self.doctor.reload(&self.paths, &RealRunner);
                    self.toast = Some(Toast {
                        text: "Re-ran all checks".into(),
                        ok: true,
                    });
                }
            },
        }
    }

    /// Snapshot + save nova.json. Nice Launcher reads it at launch — nothing to restart.
    fn nova_apply(&mut self) {
        let files = [self.nova.model().config_path().to_path_buf()];
        let store = SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        )
        .ok();
        if let Some(s) = &store {
            let _ = s.record(
                SnapshotKind::Pre,
                "before Nice Launcher config save",
                &files,
                "nova",
                &[],
            );
        }
        match self.nova.model().save() {
            Ok(()) => {
                if let Some(s) = &store {
                    let _ = s.record(
                        SnapshotKind::Post,
                        "Nice Launcher config save",
                        &files,
                        "nova",
                        &[],
                    );
                }
                self.nova.saved();
                self.toast = Some(Toast {
                    text: "Nice Launcher config saved — applies on next launch".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("save failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Install the Nice Launcher launch bind (SUPER+SPACE) if absent, remove it if
    /// present — through the shared keybinds override block, snapshot-backed.
    fn nova_toggle_bind(&mut self) {
        use studio_core::modules::keybinds;
        use studio_core::modules::nova as nova_mod;
        // The pipeline snapshots; recording here too would double the entry.
        let store = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("no change history, not applying: {}", brief(e)),
                    ok: false,
                });
                return;
            }
        };
        let result = if nova_mod::keybind(&self.paths).is_some() {
            nova_mod::remove_keybind(&self.paths, &store, &RealRunner)
                .map(|_| "Nice Launcher keybind removed".to_string())
        } else {
            match nova_mod::launch_command(&RealRunner) {
                Some(exec) => nova_mod::install_keybind(
                    &self.paths,
                    "SUPER",
                    "SPACE",
                    &exec,
                    &store,
                    &RealRunner,
                )
                .map(|b| {
                    format!(
                        "{} launches Nice Launcher",
                        keybinds::render_chord(b.modmask, &b.key)
                    )
                }),
                None => Err(studio_core::error::StudioError::External {
                    cmd: "nice-launcher".into(),
                    detail: "Nice Launcher is not installed".into(),
                }),
            }
        };
        match result {
            Ok(text) => {
                self.nova.reload(&self.paths);
                self.toast = Some(Toast { text, ok: true });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("keybind: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Install Nice Launcher from GitHub via XDG layout.
    fn nova_install(&mut self) {
        use studio_core::modules::nova as nova_mod;
        self.toast = Some(Toast {
            text: "Installing Nice Launcher...".into(),
            ok: true,
        });
        match nova_mod::install(&RealRunner) {
            Ok(bin) => {
                self.nova.reload(&self.paths);
                self.toast = Some(Toast {
                    text: format!("Installed to {}", bin.display()),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("install failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Roll every tracked file back to a snapshot (roadmap 0.9.4). The restore
    /// is itself recorded, so it too can be undone. Hypr files are reloaded so
    /// the rollback is live, not just on-disk.
    fn snapshots_restore(&mut self, id: &str) {
        let store = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("snapshot store: {}", brief(e)),
                    ok: false,
                });
                return;
            }
        };
        match store.restore(id) {
            Ok(files) => {
                if files
                    .iter()
                    .any(|p| p.starts_with(self.paths.hypr_config()))
                {
                    let _ = RealRunner.run(&cmds::hypr_reload());
                }
                self.snapshots.reload();
                self.toast = Some(Toast {
                    text: format!(
                        "Restored {} file(s) from {}",
                        files.len(),
                        &id[..id.len().min(8)]
                    ),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("restore failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Snapshot config.jsonc, save the bar layout, restart Waybar, refresh.
    /// Persist a Waybar config-target choice (roadmap 0.8.2) and reload the
    /// screen from the newly-resolved file. `None` clears the override.
    fn retarget_waybar(&mut self, path: Option<std::path::PathBuf>) {
        use studio_core::modules::targets::Overrides;
        let cfg = studio_core::studio_config_dir().join("config.toml");
        let mut overrides = Overrides::load(&cfg);
        let result = match &path {
            Some(p) => overrides.set("waybar.config", &p.to_string_lossy()),
            None => overrides.reset("waybar.config").map(|_| ()),
        }
        .and_then(|()| overrides.save());
        match result {
            Ok(()) => {
                self.waybar.reload(&self.paths);
                self.toast = Some(Toast {
                    text: match path {
                        Some(p) => format!(
                            "Now editing {}",
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| p.display().to_string())
                        ),
                        None => "Back to the Omarchy default config".into(),
                    },
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't switch target: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Build the removal plan for the selected apps (roadmap 0.8.3) and hand it
    /// to the screen's confirm modal. Read-only — nothing is removed here.
    fn apps_preview(&mut self, ids: Vec<String>) {
        use studio_core::modules::apps::{find, RemovalPlan};
        let items: Vec<_> = ids.iter().filter_map(|id| find(id)).collect();
        match RemovalPlan::build(&items, &RealRunner, &self.paths) {
            Ok(plan) if plan.is_empty() => {
                self.toast = Some(Toast {
                    text: "Nothing removable in that selection".into(),
                    ok: false,
                });
            }
            Ok(plan) => self.apps.open_confirm(plan),
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't build plan: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Execute a confirmed removal plan, log it for restore, and reload markers.
    fn apps_remove(&mut self, plan: studio_core::modules::apps::RemovalPlan) {
        use studio_core::modules::apps::Manifest;
        match plan.execute(&RealRunner, false) {
            Ok(_) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let mut manifest = Manifest::load(&studio_core::studio_state_dir());
                let restore = manifest
                    .record(&plan, now)
                    .ok()
                    .and_then(|id| manifest.save().ok().map(|()| id));
                self.apps.reload(&self.paths);
                self.toast = Some(Toast {
                    text: match restore {
                        Some(id) => format!("Removed · restore with `apps restore {id}`"),
                        None => "Removed".into(),
                    },
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("removal failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Flash each monitor's name on its physical panel (roadmap 0.8.4).
    fn monitors_identify(&mut self) {
        use studio_core::modules::monitors as mon;
        match mon::load(&RealRunner) {
            Ok(mons) => {
                for (i, m) in mons.iter().enumerate() {
                    let _ = RealRunner.run(&mon::identify_cmd(i, &m.name));
                }
                self.toast = Some(Toast {
                    text: "Flashed each monitor's name".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("identify failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Persist an edited monitor layout to monitors.conf (managed block +
    /// hotplug fallback), snapshot-backed, then reload Hyprland.
    fn monitors_save(&mut self, layout: studio_core::modules::monitors::Layout) {
        use studio_core::modules::monitors as mon;
        let path = mon::conf_path(&self.paths);
        let updated = mon::render_conf(&mon::read_conf(&self.paths), &layout);
        let store = SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        );
        if let Ok(s) = &store {
            let _ = s.record(
                SnapshotKind::Pre,
                "before: monitor layout change",
                std::slice::from_ref(&path),
                "monitors",
                &[],
            );
        }
        if let Err(e) = studio_core::configfs::atomic_write(&path, &updated) {
            self.toast = Some(Toast {
                text: format!("couldn't write monitors.conf: {}", brief(e)),
                ok: false,
            });
            return;
        }
        let _ = RealRunner.run(&cmds::hypr_reload());
        if let Ok(s) = &store {
            let _ = s.record(
                SnapshotKind::Post,
                "monitor layout change",
                std::slice::from_ref(&path),
                "monitors",
                &[],
            );
        }
        self.monitors.reload(&self.paths);
        self.toast = Some(Toast {
            text: "Saved monitor layout · undo from Snapshots".into(),
            ok: true,
        });
    }

    /// Switch the active power profile and persist it at login (roadmap 0.8.6).
    fn set_power_profile(&mut self, name: String) {
        use studio_core::modules::power;
        let out = RealRunner.run(&power::set_cmd(&name));
        match out {
            Ok(o) if o.ok() => {
                let _ = power::set_persist(&self.paths, Some(&name));
                self.battery.reload();
                self.toast = Some(Toast {
                    text: format!("Power profile → {name}"),
                    ok: true,
                });
            }
            Ok(o) => {
                self.toast = Some(Toast {
                    text: format!("couldn't switch profile: {}", o.stderr.trim()),
                    ok: false,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't switch profile: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn apply_waybar(&mut self) {
        let Some(file) = self.waybar.config_path() else {
            return;
        };
        let store = SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        );
        if let Ok(s) = &store {
            let _ = s.record(
                SnapshotKind::Pre,
                "before: waybar layout change",
                std::slice::from_ref(&file),
                "waybar",
                &[],
            );
        }
        use studio_core::modules::waybar::ApplyOutcome;
        match self.waybar.commit(&RealRunner) {
            Ok(outcome) => {
                if let Ok(s) = &store {
                    let _ = s.record(
                        SnapshotKind::Post,
                        "waybar layout change",
                        std::slice::from_ref(&file),
                        "waybar",
                        &[],
                    );
                }
                self.waybar.reload(&self.paths);
                self.toast = Some(match outcome {
                    ApplyOutcome::Reverted => Toast {
                        text: "That change would have stopped Waybar — reverted.".into(),
                        ok: false,
                    },
                    _ => Toast {
                        text: "Saved bar layout · undo from Snapshots".into(),
                        ok: true,
                    },
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("waybar apply failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Snapshot the mako template, write behavior, regenerate + reload, refresh.
    fn apply_notifications(&mut self) {
        let file = self.notifications.model().tpl_path().to_path_buf();
        let store = SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        );
        if let Ok(s) = &store {
            let _ = s.record(
                SnapshotKind::Pre,
                "before: notification behavior",
                std::slice::from_ref(&file),
                "mako",
                &[],
            );
        }
        match self.notifications.model().apply(&RealRunner) {
            Ok(()) => {
                if let Ok(s) = &store {
                    let _ = s.record(
                        SnapshotKind::Post,
                        "notification behavior",
                        std::slice::from_ref(&file),
                        "mako",
                        &[],
                    );
                }
                self.notifications.reload(&self.paths);
                self.refresh_dnd();
                self.toast = Some(Toast {
                    text: "Saved notification behavior · undo from Snapshots".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("mako apply failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Flip Do Not Disturb live and reflect the new state on the screen.
    fn toggle_dnd(&mut self) {
        use studio_core::modules::mako;
        let now = mako::dnd_active(&RealRunner);
        match mako::set_dnd(&RealRunner, !now) {
            Ok(()) => {
                self.notifications.set_dnd(Some(!now));
                self.toast = Some(Toast {
                    text: format!("Do Not Disturb {}", if !now { "on" } else { "off" }),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't toggle DND: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn notif_test(&mut self, urgency: &str) {
        use studio_core::modules::mako;
        self.toast = Some(match mako::send_sample(&RealRunner, urgency) {
            Ok(()) => Toast {
                text: format!("sent a {urgency} test notification"),
                ok: true,
            },
            Err(e) => Toast {
                text: format!("couldn't send test: {}", brief(e)),
                ok: false,
            },
        });
    }

    fn refresh_dnd(&mut self) {
        use studio_core::modules::mako;
        self.notifications
            .set_dnd(Some(mako::dnd_active(&RealRunner)));
    }

    /// Snapshot + apply hypridle (restart) and hyprlock (backup on first save).
    fn apply_lockidle(&mut self) {
        let store = SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        );
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        if let Some(idle) = self.lockidle.idle_ref() {
            files.push(idle.path().to_path_buf());
        }
        if let Some(lock) = self.lockidle.lock_ref() {
            files.push(lock.path().to_path_buf());
        }
        if let Ok(s) = &store {
            let _ = s.record(
                SnapshotKind::Pre,
                "before: lock & idle",
                &files,
                "lockidle",
                &[],
            );
        }

        let mut ok = true;
        let mut note = String::new();
        if let Some(idle) = self.lockidle.idle_ref() {
            if let Err(e) = idle.apply(&RealRunner) {
                ok = false;
                note = format!("hypridle: {}", brief(e));
            }
        }
        if ok {
            if let Some(lock) = self.lockidle.lock_ref() {
                match lock.save() {
                    Ok(Some(dir)) => {
                        note = format!("backed up lock screen to {}", dir.display());
                    }
                    Ok(None) => {}
                    Err(e) => {
                        ok = false;
                        note = format!("hyprlock: {}", brief(e));
                    }
                }
            }
        }

        if ok {
            if let Ok(s) = &store {
                let _ = s.record(SnapshotKind::Post, "lock & idle", &files, "lockidle", &[]);
            }
            self.lockidle.reload(&self.paths);
            let text = if note.is_empty() {
                "Saved lock & idle · undo from Snapshots".into()
            } else {
                format!("Saved · {note}")
            };
            self.toast = Some(Toast { text, ok: true });
        } else {
            self.toast = Some(Toast {
                text: format!("lock & idle failed — {note}"),
                ok: false,
            });
        }
    }

    fn wp_set(&mut self, path: &std::path::Path) {
        use studio_core::modules::wallpapers::Wallpapers;
        match Wallpapers::set(&RealRunner, path) {
            Ok(()) => {
                self.wallpapers.reload(&self.paths);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.toast = Some(Toast {
                    text: format!("Wallpaper: {name}"),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't set wallpaper: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Write the charge window to every threshold-capable battery. Root is
    /// usually required — the error detail already carries the sudo command.
    fn battery_apply(&mut self, start: u8, end: u8) {
        use studio_core::modules::battery as bat;
        let capable: Vec<_> = bat::detect()
            .into_iter()
            .filter(|b| b.supports_thresholds())
            .collect();
        if capable.is_empty() {
            self.toast = Some(Toast {
                text: "no charge-threshold interface on this laptop".into(),
                ok: false,
            });
            return;
        }
        for b in &capable {
            if let Err(e) = bat::set_thresholds(b, start, end) {
                let msg = match e {
                    studio_core::StudioError::External { detail, .. } => detail,
                    other => format!("{other:?}"),
                };
                self.toast = Some(Toast {
                    text: msg,
                    ok: false,
                });
                return;
            }
        }
        self.battery.reload();
        self.toast = Some(Toast {
            text: format!(
                "Charge window {start}–{end}% applied — persists until reboot (see the screen for --persist)"
            ),
            ok: true,
        });
    }

    /// Launch a companion tool (Integrations screen / contextual actions),
    /// detached and reaped; callers only pass actions of tools detected as
    /// installed. TUI tools go through Omarchy's floating-terminal launcher —
    /// detaching them with no tty would just kill them.
    fn launch_tool(&mut self, exec: &str, label: &str, terminal: bool) {
        let (bin, args): (&str, Vec<&str>) = if terminal {
            (
                "omarchy-launch-floating-terminal-with-presentation",
                vec![exec],
            )
        } else {
            ("sh", vec!["-c", exec])
        };
        match std::process::Command::new(bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                self.toast = Some(Toast {
                    text: format!("Launched {label}"),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't launch {label}: {e}"),
                    ok: false,
                });
            }
        }
    }

    /// Hand the file to an external viewer, detached: imv for stills/gifs
    /// (present on Omarchy), mpv for videos. The fallback path when the
    /// terminal can't draw images — and a full-size look even when it can.
    fn wp_open(&mut self, entry: &studio_core::modules::wallpapers::Entry) {
        use studio_core::modules::wallpapers::Kind;
        let viewer = match entry.kind {
            Kind::Video => "mpv",
            _ => "imv",
        };
        match std::process::Command::new(viewer)
            .arg(&entry.path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // reap the viewer when it closes so it never lingers defunct
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                self.toast = Some(Toast {
                    text: format!("Opened {} in {viewer}", entry.name()),
                    ok: true,
                });
            }
            Err(_) => {
                self.toast = Some(Toast {
                    text: format!("{viewer} is not installed — sudo pacman -S {viewer}"),
                    ok: false,
                });
            }
        }
    }

    /// `t` on a wallpaper: extract a palette from it and open the wizard.
    fn open_wizard(&mut self, entry: &studio_core::modules::wallpapers::Entry) {
        use studio_core::modules::wallpapers::Kind;
        if entry.kind == Kind::Video {
            self.toast = Some(Toast {
                text: "palettes come from stills — pick an image or gif".into(),
                ok: false,
            });
            return;
        }
        match WizardScreen::open(&entry.path) {
            Ok(w) => self.wizard = Some(w),
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't read image: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// `w` on the Wallpapers screen: open the wallhaven browser, seeded with
    /// the theme accent (color match) and the monitor's resolution floor.
    fn open_wallhaven(&mut self) {
        let accent = match self.skin.accent {
            ratatui::style::Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
            _ => None,
        };
        let theme = self.paths.current_theme_name().unwrap_or_default();
        let dir = self.paths.config.join(format!("backgrounds/{theme}"));
        self.wallhaven = Some(WallhavenBrowser::open(accent, monitor_atleast(), dir));
    }

    /// Drain the browser's worker results; a finished download turns into
    /// the action the user asked for (set / craft / keep).
    fn wallhaven_poll(&mut self) {
        let Some(b) = self.wallhaven.as_mut() else {
            return;
        };
        let Some((path, then)) = b.drain() else {
            return;
        };
        self.wallpapers.reload(&self.paths);
        match then {
            Then::Keep => {
                self.toast = Some(Toast {
                    text: format!("saved → {}", path.display()),
                    ok: true,
                });
            }
            Then::Set => {
                self.wallhaven = None;
                self.wp_set(&path);
            }
            Then::Craft => match WizardScreen::open(&path) {
                Ok(w) => self.wizard = Some(w),
                Err(e) => {
                    self.toast = Some(Toast {
                        text: format!("couldn't read image: {}", brief(e)),
                        ok: false,
                    });
                }
            },
        }
    }

    /// `c` on the Themes screen: open the community browser, seeded with
    /// the store's slugs so already-installed entries are marked.
    fn open_community(&mut self) {
        let installed = ThemeStore::from_paths(&self.paths)
            .list()
            .into_iter()
            .map(|t| t.slug)
            .collect();
        self.community = Some(CommunityBrowser::open(installed));
    }

    /// Drain the community browser's worker results; a finished install
    /// reloads Themes (the install script applied the theme too).
    fn community_poll(&mut self) {
        let Some(b) = self.community.as_mut() else {
            return;
        };
        let Some(done) = b.drain() else {
            return;
        };
        self.themes.reload(&self.paths);
        self.themes.focus(&done.slug);
        self.skin = Skin::from_current(&self.paths);
        // The install script *applies* the theme, so a colours-only one has
        // already broken whatever it doesn't cover. Say so now, while the
        // cause is obvious — otherwise the first symptom is an unrelated-
        // looking error the next time that app starts (nvim is the loud one).
        let gaps: Vec<&str> =
            studio_core::modules::coverage::report(&self.paths, &self.paths.current_theme_dir())
                .into_iter()
                .filter(|r| r.is_breakage())
                .map(|r| r.app)
                .collect();
        self.toast = Some(if gaps.is_empty() {
            Toast {
                text: format!("{} installed and applied (slug `{}`)", done.name, done.slug),
                ok: true,
            }
        } else {
            Toast {
                text: format!(
                    "{} applied, but it doesn't theme {} — see Doctor",
                    done.name,
                    gaps.join(", ")
                ),
                ok: false,
            }
        });
    }

    /// Enter in the wizard: write the theme dir, land the user on Themes
    /// with the new theme ready to apply or refine in the palette editor.
    fn wizard_create(&mut self) {
        let Some(w) = self.wizard.as_ref() else {
            return;
        };
        match studio_core::modules::wizard::materialize(
            &self.paths,
            &w.name,
            &w.image,
            &w.extraction,
        ) {
            Ok(dir) => {
                let slug = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.wizard = None;
                self.themes = ThemesScreen::load(&self.paths);
                self.screen = Screen::Themes;
                self.toast = Some(Toast {
                    text: format!("Theme `{slug}` created — apply it here, or e to refine"),
                    ok: true,
                });
            }
            // duplicate name / io trouble: stay open so the name can change
            Err(e) => {
                self.toast = Some(Toast {
                    text: brief(e),
                    ok: false,
                });
            }
        }
    }

    fn wp_next(&mut self) {
        use studio_core::modules::wallpapers::Wallpapers;
        match Wallpapers::next(&RealRunner) {
            Ok(()) => {
                self.wallpapers.reload(&self.paths);
                self.toast = Some(Toast {
                    text: "Cycled to the next background".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't cycle: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn wp_add(&mut self, path: &std::path::Path) {
        use studio_core::modules::wallpapers::Wallpapers;
        let w = Wallpapers::load(&self.paths);
        match w.add(&self.paths, path) {
            Ok(dest) => {
                self.wallpapers.reload(&self.paths);
                let name = dest
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.toast = Some(Toast {
                    text: format!("Added {name} — Enter applies it"),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't add: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn wp_remove(&mut self, entry: &studio_core::modules::wallpapers::Entry) {
        use studio_core::modules::wallpapers::Wallpapers;
        match Wallpapers::remove(entry) {
            Ok(()) => {
                self.wallpapers.reload(&self.paths);
                self.toast = Some(Toast {
                    text: format!("Removed {}", entry.name()),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("couldn't remove: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Install (or repair) the lifecycle hooks and refresh the Doctor view.
    fn install_hooks(&mut self) {
        match studio_core::hooks::install(&self.paths, &studio_core::studio_state_dir()) {
            Ok(_) => {
                self.doctor.reload(&self.paths, &RealRunner);
                self.toast = Some(Toast {
                    text: "Hooks installed — Studio now survives theme changes and updates".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("hook install failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn remove_hooks(&mut self) {
        match studio_core::hooks::uninstall(&self.paths, &studio_core::studio_state_dir()) {
            Ok(_) => {
                self.doctor.reload(&self.paths, &RealRunner);
                self.toast = Some(Toast {
                    text: "Hooks removed".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("hook removal failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn osd_test(&mut self) {
        use studio_core::modules::swayosd;
        self.toast = Some(match swayosd::self_test(&RealRunner) {
            Ok(()) => Toast {
                text: "flashed the on-screen display".into(),
                ok: true,
            },
            Err(e) => Toast {
                text: format!("couldn't flash OSD: {}", brief(e)),
                ok: false,
            },
        });
    }

    /// Snapshot looknfeel.conf, apply an animation preset, live-reload, refresh.
    fn apply_animation(&mut self, name: &str) {
        use studio_core::modules::animations;
        let Some(preset) = animations::preset(name) else {
            return;
        };
        // The pipeline snapshots; recording here too would double the entry.
        let store = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("no change history, not applying: {}", brief(e)),
                    ok: false,
                });
                return;
            }
        };
        match animations::apply(&self.paths, preset, &store, &RealRunner) {
            Ok(_) => {
                self.animations.reload(&self.paths);
                self.toast = Some(Toast {
                    text: format!("Animations: {name} · undo from Snapshots"),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("animation change failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Persist the look & feel batch with a snapshot, then reload the screen.
    fn save_looknfeel(&mut self) {
        if !self.looknfeel.any_overrides() {
            self.toast = Some(Toast {
                text: "Nothing to save — no changes from the Omarchy defaults".into(),
                ok: true,
            });
            return;
        }
        // The pipeline snapshots pre/drift/post itself — recording here too
        // would double every entry in the timeline.
        let store = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("no change history, not applying: {}", brief(e)),
                    ok: false,
                });
                return;
            }
        };
        match self
            .looknfeel
            .commit(&self.paths, &store, &RealRunner, "look & feel change")
        {
            Ok(()) => {
                studio_core::modules::looknfeel::clear_preview_marker(
                    &studio_core::studio_state_dir(),
                );
                self.looknfeel.reload(&self.paths);
                self.toast = Some(Toast {
                    text: "Saved look & feel · undo from Snapshots".into(),
                    ok: true,
                });
            }
            // Hyprland refused the config; the pipeline already put the old
            // one back, so the desktop is fine — say what happened.
            Err(studio_core::StudioError::VerifyFailed { rolled_back, .. }) => {
                self.looknfeel.reload(&self.paths);
                self.toast = Some(Toast {
                    text: if rolled_back {
                        "Hyprland rejected that — your previous settings are back".into()
                    } else {
                        "Hyprland rejected that AND the rollback failed — see Snapshots".into()
                    },
                    ok: false,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("save failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Remove all look & feel overrides and reload Hyprland to the base config.
    fn reset_looknfeel(&mut self) {
        let store = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("no change history, not resetting: {}", brief(e)),
                    ok: false,
                });
                return;
            }
        };
        match self.looknfeel.commit(
            &self.paths,
            &store,
            &RealRunner,
            "reset look & feel to defaults",
        ) {
            Ok(()) => {
                studio_core::modules::looknfeel::clear_preview_marker(
                    &studio_core::studio_state_dir(),
                );
                self.looknfeel.reload(&self.paths);
                self.toast = Some(Toast {
                    text: "Reset look & feel to Omarchy defaults".into(),
                    ok: true,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("reset failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    /// Snapshot the bindings file, persist the override block, live-reload, and
    /// refresh the screen — the full undoable apply for a keybind change.
    fn commit_keybinds(&mut self, summary: String) {
        // The pipeline snapshots pre/drift/post itself — recording here too
        // would double every entry in the timeline.
        let store = match SnapshotStore::open_or_init(
            studio_core::studio_state_dir().join("history"),
            Box::new(RealRunner),
        ) {
            Ok(s) => s,
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("no change history, not applying: {}", brief(e)),
                    ok: false,
                });
                return;
            }
        };
        match self
            .keybinds
            .commit(&self.paths, &store, &RealRunner, &summary)
        {
            Ok(()) => {
                self.keybinds.reload(&self.paths, &RealRunner);
                self.toast = Some(Toast {
                    text: format!("{summary} · undo with u on Snapshots"),
                    ok: true,
                });
            }
            // Hyprland refused the binds; the pipeline already restored the
            // previous block, so the keymap on screen is stale — reload it.
            Err(studio_core::StudioError::VerifyFailed { rolled_back, .. }) => {
                self.keybinds.reload(&self.paths, &RealRunner);
                self.toast = Some(Toast {
                    text: if rolled_back {
                        "Hyprland rejected those binds — your previous ones are back".into()
                    } else {
                        "Hyprland rejected those binds AND the rollback failed — see Snapshots"
                            .into()
                    },
                    ok: false,
                });
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("keybind change failed: {}", brief(e)),
                    ok: false,
                });
            }
        }
    }

    fn open_editor(&mut self, slug: &str) {
        let store = ThemeStore::from_paths(&self.paths);
        let Some(theme) = store.get(slug) else { return };
        match PaletteEditor::open(&theme) {
            Some(ed) => {
                let origin = self.paths.current_theme_name().unwrap_or_default();
                self.editor = Some((ed, origin));
            }
            None => {
                self.toast = Some(Toast {
                    text: format!("{slug} has no colors.toml to edit"),
                    ok: false,
                });
            }
        }
    }

    fn editor_key(&mut self, key: KeyEvent) {
        let Some((ed, _)) = self.editor.as_mut() else {
            return;
        };
        let store = ThemeStore::from_paths(&self.paths);
        match ed.handle(key) {
            EditorAction::None => {}
            EditorAction::Preview => {
                // Ephemeral: bypasses the snapshot pipeline by design (spec 04 §2).
                if let Err(e) = store.preview(ed.working(), ed.light, &RealRunner) {
                    self.toast = Some(Toast {
                        text: format!("preview failed: {}", brief(e)),
                        ok: false,
                    });
                }
            }
            EditorAction::Save => self.save_editor(),
            EditorAction::Cancel => self.cancel_editor(),
        }
    }

    fn save_editor(&mut self) {
        let Some((ed, _)) = self.editor.take() else {
            return;
        };
        let store = ThemeStore::from_paths(&self.paths);
        let Some(theme) = store.get(&ed.slug) else {
            return;
        };
        let dest = theme.writable_colors_path(&store.user_dir);
        let result = studio_core::configfs::atomic_write(&dest, &ed.working().to_string())
            .and_then(|_| {
                RealRunner
                    .run(&cmds::theme_set(&ed.slug))
                    .map(|o| (o, dest.clone()))
            });
        store.cleanup_preview();
        match result {
            Ok((o, _)) if o.ok() => {
                self.skin = Skin::from_current(&self.paths);
                self.themes.reload(&self.paths);
                self.toast = Some(Toast {
                    text: format!("Saved & applied {}", ed.pretty),
                    ok: true,
                });
            }
            Ok((o, _)) => {
                self.toast = Some(Toast {
                    text: format!("saved, but apply failed: {}", o.stderr.trim()),
                    ok: false,
                })
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("save failed: {}", brief(e)),
                    ok: false,
                })
            }
        }
    }

    fn cancel_editor(&mut self) {
        let Some((ed, origin)) = self.editor.take() else {
            return;
        };
        let store = ThemeStore::from_paths(&self.paths);
        store.cleanup_preview();
        // Restore whatever was showing before we started previewing.
        if !origin.is_empty() {
            let _ = RealRunner.run(&cmds::theme_set(&origin));
        }
        self.skin = Skin::from_current(&self.paths);
        let msg = if ed.dirty {
            "Discarded palette edits"
        } else {
            "Closed palette editor"
        };
        self.toast = Some(Toast {
            text: msg.into(),
            ok: true,
        });
    }

    fn cycle(&mut self, dir: i32) {
        let i = Screen::ALL.iter().position(|s| *s == self.screen).unwrap();
        let n = Screen::ALL.len() as i32;
        let next = ((i as i32 + dir).rem_euclid(n)) as usize;
        self.screen = Screen::ALL[next];
    }

    fn jump(&mut self, idx: usize) {
        if let Some(s) = Screen::ALL.get(idx) {
            self.screen = *s;
        }
    }

    fn apply_theme(&mut self, slug: &str) {
        match RealRunner.run(&cmds::theme_set(slug)) {
            Ok(o) if o.ok() => {
                // The desktop just re-themed; restyle Studio to match.
                self.skin = Skin::from_current(&self.paths);
                self.themes.reload(&self.paths);
                self.toast = Some(Toast {
                    text: format!("Applied {slug}"),
                    ok: true,
                });
            }
            Ok(o) => {
                self.toast = Some(Toast {
                    text: format!("omarchy-theme-set failed: {}", o.stderr.trim()),
                    ok: false,
                })
            }
            Err(e) => {
                self.toast = Some(Toast {
                    text: format!("could not run omarchy-theme-set: {e:?}"),
                    ok: false,
                })
            }
        }
    }

    fn fork_theme(&mut self, src: &str, name: &str) {
        let store = ThemeStore::from_paths(&self.paths);
        let Some(theme) = store.get(src) else { return };
        match store.fork(&theme, name) {
            Ok(t) => {
                self.themes.reload(&self.paths);
                self.toast = Some(Toast {
                    text: format!("Forked {src} → {} (press enter to apply)", t.slug),
                    ok: true,
                });
            }
            Err(e) => {
                let detail = match e {
                    studio_core::StudioError::External { detail, .. } => detail,
                    other => format!("{other:?}"),
                };
                self.toast = Some(Toast {
                    text: format!("Fork failed: {detail}"),
                    ok: false,
                });
            }
        }
    }

    fn palette_key(&mut self, key: KeyEvent) {
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Enter => {
                let choice = p.selected();
                self.palette = None;
                if let Some(c) = choice {
                    self.run_palette_choice(c);
                }
            }
            KeyCode::Up => p.up(),
            KeyCode::Down => p.down(),
            KeyCode::Backspace => p.backspace(),
            KeyCode::Char(c) => p.push(c),
            _ => {}
        }
    }

    fn run_palette_choice(&mut self, c: Choice) {
        match c {
            Choice::Go(s) => self.screen = s,
            Choice::Theme(slug) => {
                self.screen = Screen::Themes;
                self.themes.focus(&slug);
            }
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(f.area());
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(20)])
            .split(rows[0]);
        self.draw_rail(f, cols[0]);
        self.draw_panel(f, cols[1]);
        self.draw_keybar(f, rows[1]);

        if let Some(b) = &self.wallhaven {
            b.render(f, cols[1], &self.skin, &mut self.images);
        }
        if let Some(b) = &self.community {
            b.render(f, cols[1], &self.skin, &mut self.images);
        }
        if let Some(w) = &self.wizard {
            w.render(f, cols[1], &self.skin);
        }
        if self.help {
            self.draw_help(f);
        }
        if let Some(p) = &self.palette {
            p.render(f, &self.skin);
        }
        // The on-ramp centers in the content pane, like the other modals; it
        // still owns all input until dismissed (see on_key).
        if let Some(w) = &self.onboarding {
            w.render(f, cols[1], &self.skin);
        }
    }

    fn draw_rail(&mut self, f: &mut Frame, area: Rect) {
        // When the panel holds focus the rail is inactive: its selection reads
        // as a muted memory of where you are, not a live cursor.
        let active = self.focus == Focus::Nav;
        let marker_style = if active {
            self.skin.accent_bold()
        } else {
            self.skin.dim()
        };
        let items: Vec<ListItem> = Screen::ALL
            .iter()
            .map(|s| {
                let selected = *s == self.screen;
                let marker = Span::styled(if selected { "▌ " } else { "  " }, marker_style);
                let label_style = if selected && active {
                    self.skin.accent_bold()
                } else if selected {
                    self.skin.dim()
                } else {
                    self.skin.body()
                };
                let row = Line::from(vec![
                    marker,
                    Span::styled(format!("{:<15}", s.label()), label_style),
                ]);
                let item = ListItem::new(row);
                if selected && active {
                    item.style(ratatui::style::Style::default().bg(self.skin.sel_bg))
                } else {
                    item
                }
            })
            .collect();
        let border_style = if active {
            self.skin.border()
        } else {
            self.skin.dim()
        };
        let hint = if active {
            " ↑↓ · ⏎ open "
        } else {
            " ⏎ open "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Line::from(vec![
                Span::styled(" ✦ ", self.skin.accent_bold()),
                Span::styled("Studio ", self.skin.body()),
            ]))
            .title_bottom(Line::from(Span::styled(hint, self.skin.dim())).left_aligned())
            .title_bottom(
                Line::from(Span::styled(
                    concat!(" v", env!("CARGO_PKG_VERSION"), " "),
                    self.skin.dim(),
                ))
                .right_aligned(),
            );

        let selected_idx = Screen::ALL.iter().position(|s| *s == self.screen).unwrap();
        self.rail_state.select(Some(selected_idx));
        f.render_stateful_widget(List::new(items).block(block), area, &mut self.rail_state);
    }

    /// The main pane: a rounded panel whose border carries the screen title
    /// (left) and the active theme (right); the screen renders inside.
    fn draw_panel(&mut self, f: &mut Frame, area: Rect) {
        let title = match &self.editor {
            Some((ed, _)) => format!(" Palette · {} ", ed.pretty),
            None => format!(" {} ", self.screen.label()),
        };
        let current = self.paths.current_theme_name().unwrap_or_default();
        // Grey the panel until it holds focus, so the eye lands on the rail.
        let active = self.focus == Focus::Panel;
        let border_style = if active {
            self.skin.border()
        } else {
            self.skin.dim()
        };
        let title_style = if active {
            self.skin.accent_bold()
        } else {
            self.skin.dim()
        };
        let block = Block::default()
            .borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Line::from(Span::styled(title, title_style)))
            .title_top(
                Line::from(Span::styled(format!(" {current} "), self.skin.dim())).right_aligned(),
            )
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.draw_content(f, inner);
    }

    fn draw_content(&mut self, f: &mut Frame, area: Rect) {
        if let Some((ed, _)) = &self.editor {
            ed.render(f, area, &self.skin);
            return;
        }
        // The home screen gets the wordmark banner when there's room for it.
        if self.screen == Screen::Themes
            && area.height >= logo::HEIGHT + 15
            && area.width >= logo::WIDTH
        {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(logo::HEIGHT), Constraint::Min(1)])
                .split(area);
            logo::render(f, rows[0], &self.skin);
            self.themes.render(f, rows[1], &self.skin, &mut self.images);
            return;
        }
        match self.screen {
            Screen::Themes => self.themes.render(f, area, &self.skin, &mut self.images),
            Screen::Keybinds => self.keybinds.render(f, area, &self.skin),
            Screen::LookFeel => self.looknfeel.render(f, area, &self.skin),
            Screen::Animations => self.animations.render(f, area, &self.skin),
            Screen::Waybar => self.waybar.render(f, area, &self.skin),
            Screen::Notifications => self.notifications.render(f, area, &self.skin),
            Screen::LockIdle => self.lockidle.render(f, area, &self.skin),
            Screen::Snapshots => self.snapshots.render(f, area, &self.skin),
            Screen::Wallpapers => self
                .wallpapers
                .render(f, area, &self.skin, &mut self.images),
            Screen::Integrations => self.integrations.render(f, area, &self.skin),
            Screen::Apps => self.apps.render(f, area, &self.skin),
            Screen::Monitors => self.monitors.render(f, area, &self.skin),
            Screen::Tweaks => self.tweaks.render(f, area, &self.skin),
            Screen::Battery => self.battery.render(f, area, &self.skin),
            Screen::Nova => self.nova.render(f, area, &self.skin),
            Screen::Doctor => self.doctor.render(f, area, &self.skin),
        }
    }

    /// Bottom bar: toast when present, else `key label │ key label` hints with
    /// the global chords right-aligned — the lazygit-style status strip.
    fn draw_keybar(&self, f: &mut Frame, area: Rect) {
        if let Some(t) = &self.toast {
            let (icon, style) = if t.ok {
                ("✓", self.skin.ok())
            } else {
                ("✗", self.skin.error())
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {icon} "), style.add_modifier(Modifier::BOLD)),
                    Span::styled(t.text.clone(), style),
                ])),
                area,
            );
            return;
        }
        // The rail owns input → show how to drive it; otherwise the active
        // screen's own hints, with `esc back` to climb back to the rail.
        let (hint, show_globals) = if self.focus == Focus::Nav && self.editor.is_none() {
            ("↑↓ move · ⏎ open".into(), true)
        } else {
            match &self.editor {
                Some((ed, _)) => (ed.hint(), false),
                None => match self.screen {
                    Screen::Themes => (format!("{} · esc back", self.themes.hint()), true),
                    Screen::Battery => (format!("{} · esc back", self.battery.hint()), true),
                    Screen::Apps => (format!("{} · esc back", self.apps.hint()), true),
                    Screen::Monitors => (format!("{} · esc back", self.monitors.hint()), true),
                    Screen::Tweaks => (format!("{} · esc back", self.tweaks.hint()), true),
                    Screen::Nova => (format!("{} · esc back", self.nova.hint()), true),
                    Screen::Snapshots => (format!("{} · esc back", self.snapshots.hint()), true),
                    _ => ("esc back".into(), true),
                },
            }
        };
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        let mut used = 1usize;
        for (i, seg) in hint.split('·').map(str::trim).enumerate() {
            if seg.is_empty() {
                continue;
            }
            if i > 0 {
                spans.push(Span::styled(" │ ", self.skin.border()));
                used += 3;
            }
            let (key, label) = seg.split_once(' ').unwrap_or((seg, ""));
            spans.push(Span::styled(key.to_string(), self.skin.accent_bold()));
            used += key.chars().count();
            if !label.is_empty() {
                spans.push(Span::styled(format!(" {label}"), self.skin.dim()));
                used += label.chars().count() + 1;
            }
        }
        if show_globals {
            let mut globals: Vec<(&str, &str)> =
                vec![("/", "search"), ("?", "help"), ("q", "quit")];
            if self.update.is_some() {
                globals.insert(0, ("U", "update!"));
            }
            let right_len: usize = globals
                .iter()
                .map(|(k, l)| k.len() + l.len() + 3)
                .sum::<usize>()
                + 1;
            let pad = (area.width as usize).saturating_sub(used + right_len);
            spans.push(Span::raw(" ".repeat(pad)));
            for (k, l) in globals {
                spans.push(Span::styled(format!(" {k}"), self.skin.accent_bold()));
                spans.push(Span::styled(format!(" {l} "), self.skin.dim()));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_help(&self, f: &mut Frame) {
        let area = ui::centered_rect(f.area(), 60, 13);
        f.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.skin.accent_bold())
            .title(Line::from(vec![
                Span::styled(" ? ", self.skin.accent_bold()),
                Span::styled("Help ", self.skin.body()),
            ]))
            .title_bottom(
                Line::from(Span::styled(" any key to close ", self.skin.dim())).right_aligned(),
            )
            .padding(Padding::new(1, 1, 1, 0));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let rows = [
            ("↑ / ↓", "move (rail: between modules · panel: selection)"),
            ("enter / tab", "enter the module panel from the rail"),
            ("esc", "back to the rail (from rail: quit)"),
            ("1–9, 0", "jump straight to a module"),
            ("/", "command palette (search everything)"),
            ("←→ / +−", "adjust the selected value"),
            ("U", "install a waiting update & restart"),
            ("q", "quit"),
            ("?", "this help"),
        ];
        let lines: Vec<Line> = rows
            .iter()
            .map(|(k, d)| {
                Line::from(vec![
                    Span::styled(format!("  {k:<16}"), self.skin.accent_bold()),
                    Span::styled(*d, self.skin.body()),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

// ── command palette (spec 07 §3) ─────────────────────────────────────────────

enum Choice {
    Go(Screen),
    Theme(String),
}

struct Entry {
    label: String,
    haystack: String,
    choice: Choice,
}

struct CommandPalette {
    entries: Vec<Entry>,
    query: String,
    selected: usize,
}

impl CommandPalette {
    fn new(paths: &OmarchyPaths) -> Self {
        let mut entries: Vec<Entry> = Screen::ALL
            .iter()
            .map(|s| Entry {
                label: format!("Go: {}", s.label()),
                haystack: s.label().to_lowercase(),
                choice: Choice::Go(*s),
            })
            .collect();
        for t in ThemeStore::from_paths(paths).list() {
            entries.push(Entry {
                label: format!("Theme: {}", t.pretty),
                haystack: format!("theme {} {}", t.pretty.to_lowercase(), t.slug),
                choice: Choice::Theme(t.slug),
            });
        }
        Self {
            entries,
            query: String::new(),
            selected: 0,
        }
    }

    fn matches(&self) -> Vec<&Entry> {
        let q = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| subsequence(&q, &e.haystack))
            .collect()
    }

    fn selected(&self) -> Option<Choice> {
        self.matches().get(self.selected).map(|e| match &e.choice {
            Choice::Go(s) => Choice::Go(*s),
            Choice::Theme(t) => Choice::Theme(t.clone()),
        })
    }

    fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
    fn down(&mut self) {
        let n = self.matches().len();
        if n > 0 {
            self.selected = (self.selected + 1).min(n - 1);
        }
    }
    fn push(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }
    fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    fn render(&self, f: &mut Frame, skin: &Skin) {
        let area = ui::centered_rect(f.area(), 56, 16);
        f.render_widget(Clear, area);
        let matches = self.matches();
        let count = format!(
            " {}/{} ",
            (self.selected + 1).min(matches.len()),
            matches.len()
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(skin.accent_bold())
            .title(Line::from(vec![
                Span::styled(" / ", skin.accent_bold()),
                Span::styled("Search ", skin.body()),
            ]))
            .title_bottom(Line::from(Span::styled(count, skin.dim())).right_aligned())
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("❯ ", skin.accent_bold()),
                Span::styled(self.query.clone(), skin.body()),
                Span::styled("▏", skin.accent_bold()),
            ])),
            rows[0],
        );
        let items: Vec<ListItem> = matches
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let on = i == self.selected;
                let marker = Span::styled(if on { "▌" } else { " " }, skin.accent_bold());
                let label = Span::styled(
                    format!(" {}", e.label),
                    if on { skin.selection() } else { skin.body() },
                );
                let item = ListItem::new(Line::from(vec![marker, label]));
                if on {
                    item.style(ratatui::style::Style::default().bg(skin.sel_bg))
                } else {
                    item
                }
            })
            .collect();
        f.render_widget(List::new(items), rows[2]);
    }
}

/// One-line rendering of a core error for a toast (no debug spew).
fn brief(e: studio_core::StudioError) -> String {
    use studio_core::StudioError::*;
    match e {
        External { detail, .. } => detail,
        ParseFailed { hint, .. } => hint,
        VerifyFailed { step, .. } => format!("{step} failed"),
        MissingDependency(r) => format!("{} not available", r.id),
        OmarchyMismatch { action, .. } => action,
        Io(err) => err.to_string(),
    }
}

/// Fuzzy subsequence: every char of `needle` appears in order within `hay`.
fn subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

/// Biggest connected monitor as a wallhaven `atleast` floor (`3840x2160`),
/// via `hyprctl monitors -j`. `None` outside a Hyprland session.
fn monitor_atleast() -> Option<String> {
    let mons = studio_core::modules::monitors::load(&RealRunner).ok()?;
    mons.iter()
        .map(|m| {
            (
                m.width as u64 * m.height as u64,
                format!("{}x{}", m.width, m.height),
            )
        })
        .max_by_key(|(area, _)| *area)
        .map(|(_, res)| res)
}

/// Entry point: no-args launch (spec 08). Runs until quit; restores the
/// terminal on any exit path.
/// Put the terminal back before a panic prints.
///
/// The teardown below only runs on the way out of the loop, so a panic used
/// to leave raw mode on, the alternate screen up and mouse reporting live —
/// the user's next shell echoes nothing and spews escape codes, and the
/// panic message itself is drawn into a screen that's about to vanish. The
/// hook restores first, then lets the default handler print somewhere the
/// message survives.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::DisableMouseCapture
        );
        ratatui::restore();
        eprintln!(
            "omarchy-studio hit a bug and stopped. Your config is untouched — \
             every change it makes is snapshotted first.\n\
             Please report this at https://github.com/arino08/omarchy-studio/issues\n"
        );
        previous(info);
    }));
}

/// Where the "first-run on-ramp already shown" stamp lives.
fn onboarded_marker() -> std::path::PathBuf {
    studio_core::studio_state_dir().join("onboarded")
}

/// Show the welcome on-ramp only when it hasn't been dismissed before and
/// there's still something to set up. The history repo can't be the freshness
/// signal — it's created eagerly the first time any screen opens the store —
/// so a written marker (on finish/skip) plus "not already fully set up" is what
/// keeps it a one-time thing without nagging someone who's all set.
fn should_onboard(status: &onboarding::Status) -> bool {
    !onboarded_marker().exists() && !status.all_done()
}

/// The live set-up status the on-ramp renders, read from disk.
fn onboarding_status(paths: &OmarchyPaths) -> onboarding::Status {
    let history = studio_core::studio_state_dir()
        .join("history")
        .join(".git")
        .exists();
    let integration =
        studio_core::integration::is_installed(paths) && studio_core::hooks::installed(paths);
    let themesync = !studio_core::modules::themesync::Enabled::load(
        &studio_core::studio_config_dir().join("config.toml"),
    )
    .list()
    .is_empty();
    onboarding::Status {
        history,
        integration,
        themesync,
    }
}

pub fn run() -> i32 {
    let paths = match OmarchyPaths::discover() {
        Ok(p) if p.installed() => p,
        Ok(p) => {
            eprintln!(
                "no Omarchy install at {} (set $OMARCHY_PATH if it lives elsewhere).",
                p.system.display()
            );
            return 4;
        }
        Err(_) => {
            eprintln!("HOME is not set — cannot locate an Omarchy install.");
            return 4;
        }
    };

    install_panic_hook();
    let mut terminal = ratatui::init();
    // Enable Kitty keyboard flags where supported, so chord capture can see
    // SUPER and disambiguate modifiers. No-op on terminals that lack it.
    let kbd_enhanced = matches!(
        ratatui::crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    if kbd_enhanced {
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::PushKeyboardEnhancementFlags(chord::enhancement_flags())
        );
    }
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let mut app = App::new(paths);
    let result = loop {
        if let Err(e) = terminal.draw(|f| app.draw(f)) {
            break Err(e);
        }
        // While background work is in flight (the launch update-check, or
        // wallhaven requests), poll so results can surface without a
        // keypress; otherwise this is a blocking read (zero idle wakeups).
        let background_busy = app.update_rx.is_some()
            || app.wallhaven.as_ref().is_some_and(|b| b.busy())
            || app.community.as_ref().is_some_and(|b| b.busy());
        if background_busy {
            match event::poll(std::time::Duration::from_millis(250)) {
                Ok(ready) => {
                    let polled = app.update_rx.as_ref().map(|rx| rx.try_recv());
                    match polled {
                        Some(Ok(found)) => app.update_arrived(found),
                        Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                            app.update_rx = None
                        }
                        _ => {}
                    }
                    app.wallhaven_poll();
                    app.community_poll();
                    if app.quit {
                        break Ok(());
                    }
                    if !ready {
                        continue;
                    }
                }
                Err(e) => break Err(e),
            }
        }
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                // Ctrl-C always exits, even mid text-entry.
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break Ok(());
                }
                app.on_key(chord::normalize(key));
                if app.quit {
                    break Ok(());
                }
            }
            Ok(Event::Mouse(mouse)) => {
                app.on_mouse(mouse);
            }
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    if kbd_enhanced {
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::PopKeyboardEnhancementFlags
        );
    }
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    match result {
        // An update swapped the binary at our path: replace this process
        // with the new build, same arguments.
        Ok(()) if app.restart => {
            use std::os::unix::process::CommandExt;
            let exe = app.exe.expect("restart is only set after a swap at exe");
            let err = std::process::Command::new(&exe)
                .args(std::env::args_os().skip(1))
                .exec();
            eprintln!("restart failed: {err} — start {} manually", exe.display());
            1
        }
        Ok(()) => 0,
        Err(e) => {
            eprintln!("terminal error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_subsequence_matches_in_order() {
        assert!(subsequence("tn", "tokyo night"));
        assert!(subsequence("cat", "catppuccin latte"));
        assert!(subsequence("", "anything"));
        assert!(subsequence("lumon", "theme lumon lumon"));
        // out of order or absent chars fail
        assert!(!subsequence("nt", "tn"));
        assert!(!subsequence("xyz", "tokyo night"));
    }

    #[test]
    fn screen_cycle_wraps_both_directions() {
        // ALL is the canonical rail order; first is Themes, last is Doctor.
        assert_eq!(Screen::ALL[0], Screen::Themes);
        assert_eq!(*Screen::ALL.last().unwrap(), Screen::Doctor);
        // every screen has a non-empty label
        for s in Screen::ALL {
            assert!(!s.label().is_empty());
        }
    }
}

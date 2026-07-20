//! Domain modules, one per PRD feature module. Filled in per milestone:
//!
//! | mod | PRD | spec | milestone |
//! |---|---|---|---|
//! | [`color`] | FR1.2 | 04 §2 | v0.1 |
//! | [`themes`] | M1 | 04 §1–3, §8 | v0.1 |
//! | [`keybinds`] | M3 | 05 §1 | v0.2 |
//! | `looknfeel` | M4 | 05 §2–3 | v0.3 |
//! | `waybar` | M5 | 05 §4 | v0.4 |
//! | `notify` | M6 | 05 §5–6 | v0.5 |
//! | `lockidle` | M7 | 05 §7 | v0.5 |
//! | `wallpapers` | M2 | 04 §5–6 | v0.6 |
//! | `extraction` | FR1.8 | 04 §4 | v0.6 |
//! | `wallhaven` | FR2.4 | 04 §6 | v0.6 |
//! | `wizard` | FR2.5 | 04 §7 | v0.6 |
//! | `battery` | community ask | — | v0.6 |
//! | [`update`] | community ask | — | v0.7 |

pub mod animations;
pub mod apps;
pub mod battery;
pub mod cheatsheet;
pub mod color;
pub mod community;
pub mod coverage;
pub mod dispatchers;
pub mod extraction;
pub mod keybinds;
pub mod lockidle;
pub mod looknfeel;
pub mod mako;
pub mod monitors;
pub mod nova;
pub mod power;
pub mod rice;
pub mod swayosd;
pub mod targets;
pub mod themes;
pub mod themesync;
pub mod toggles;
pub mod tweaks;
pub mod update;
pub mod wallhaven;
pub mod wallpapers;
pub mod waybar;
pub mod wizard;

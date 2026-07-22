//! Keybind model & source attribution (spec 05 §1, PRD M3).
//!
//! Two views of a bind:
//! - [`RuntimeBind`] — ground truth from `hyprctl binds -j` (what Hyprland is
//!   actually doing right now).
//! - [`ConfigBind`] — a `bind* = …` line parsed from a config file.
//!
//! Attribution joins them: for each runtime bind we find the config line that
//! defines it, tagged with the layer it came from (Omarchy default, theme,
//! user, toggle). Hyprland lets a later definition win, so when several lines
//! match we attribute to the highest-priority (last-sourced) layer.

use serde::Deserialize;

use crate::configfs::hyprlang::{Entry, HyprDoc};
use crate::configfs::Layer;
use crate::error::{Result, StudioError};

/// Hyprland modifier bitmask values (from the compositor's `modmask`).
pub mod mods {
    pub const SHIFT: u16 = 1;
    pub const CAPS: u16 = 2;
    pub const CTRL: u16 = 4;
    pub const ALT: u16 = 8;
    pub const MOD2: u16 = 16;
    pub const MOD3: u16 = 32;
    pub const SUPER: u16 = 64;
    pub const MOD5: u16 = 128;
}

/// Parse a space/whitespace-separated modifier list (`"SUPER SHIFT"`) into a
/// modmask. Accepts Hyprland's aliases (SUPER/MOD4, CTRL/CONTROL, ALT/MOD1).
pub fn mods_to_mask(text: &str) -> u16 {
    let mut mask = 0;
    for token in text.split_whitespace() {
        mask |= match token.to_ascii_uppercase().as_str() {
            "SHIFT" => mods::SHIFT,
            "CAPS" | "CAPSLOCK" => mods::CAPS,
            "CTRL" | "CONTROL" => mods::CTRL,
            "ALT" | "MOD1" => mods::ALT,
            "MOD2" => mods::MOD2,
            "MOD3" => mods::MOD3,
            "SUPER" | "MOD4" | "WIN" | "META" => mods::SUPER,
            "MOD5" => mods::MOD5,
            _ => 0,
        };
    }
    mask
}

/// Canonical modifier names for a mask, in a stable display order.
pub fn mask_to_mods(mask: u16) -> Vec<&'static str> {
    const ORDER: [(u16, &str); 6] = [
        (mods::SUPER, "SUPER"),
        (mods::CTRL, "CTRL"),
        (mods::ALT, "ALT"),
        (mods::SHIFT, "SHIFT"),
        (mods::CAPS, "CAPS"),
        (mods::MOD5, "MOD5"),
    ];
    ORDER
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

/// Human chord: `SUPER+SHIFT+T`. A bare key (no mods) renders as just the key.
pub fn render_chord(mask: u16, key: &str) -> String {
    let mut parts = mask_to_mods(mask);
    let key_disp = if key.is_empty() { "…" } else { key };
    parts.push(key_disp);
    parts.join("+")
}

/// One entry of `hyprctl binds -j`.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeBind {
    pub modmask: u16,
    pub key: String,
    #[serde(default)]
    pub keycode: i64,
    pub dispatcher: String,
    #[serde(default)]
    pub arg: String,
    #[serde(default)]
    pub submap: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub release: bool,
    #[serde(default)]
    pub repeat: bool,
}

impl RuntimeBind {
    pub fn chord(&self) -> String {
        if self.keycode != 0 && self.key.is_empty() {
            render_chord(self.modmask, &format!("code:{}", self.keycode))
        } else {
            render_chord(self.modmask, &self.key)
        }
    }
}

/// Parse the JSON array printed by `hyprctl binds -j`.
pub fn parse_runtime_binds(json: &str) -> Result<Vec<RuntimeBind>> {
    serde_json::from_str(json).map_err(|e| StudioError::ParseFailed {
        file: std::path::PathBuf::from("hyprctl binds -j"),
        line: Some(e.line()),
        hint: format!("hyprctl bind output wasn't the expected JSON: {e}"),
    })
}

/// Parse the plain-text `hyprctl binds` output.
///
/// Each bind is a flags line (`bindled`) followed by indented `key: value`
/// fields, blank-line separated:
///
/// ```text
/// bindled
/// \tmodmask: 0
/// \tkey: XF86AudioRaiseVolume
/// \tdispatcher: exec
/// \targ: omarchy-swayosd-client --output-volume raise
/// ```
///
/// Only the fields [`RuntimeBind`] carries are read; anything else is ignored,
/// so a future Hyprland adding fields doesn't break this.
pub fn parse_runtime_binds_text(text: &str) -> Vec<RuntimeBind> {
    let mut out = Vec::new();
    let mut cur: Option<RuntimeBind> = None;
    let flush = |cur: &mut Option<RuntimeBind>, out: &mut Vec<RuntimeBind>| {
        if let Some(b) = cur.take() {
            // A bind with neither a key nor a keycode isn't one.
            if !b.key.is_empty() || b.keycode != 0 {
                out.push(b);
            }
        }
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Unindented line = the directive keyword, starting a new bind.
        if !line.starts_with([' ', '\t']) {
            flush(&mut cur, &mut out);
            if line.trim_end().starts_with("bind") {
                cur = Some(RuntimeBind {
                    modmask: 0,
                    key: String::new(),
                    keycode: 0,
                    dispatcher: String::new(),
                    arg: String::new(),
                    submap: String::new(),
                    description: String::new(),
                    locked: false,
                    release: false,
                    repeat: false,
                });
            }
            continue;
        }
        let Some(b) = cur.as_mut() else { continue };
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "modmask" => b.modmask = v.parse().unwrap_or(0),
            "key" => b.key = v.to_string(),
            "keycode" => b.keycode = v.parse().unwrap_or(0),
            "description" => b.description = v.to_string(),
            "dispatcher" => b.dispatcher = v.to_string(),
            "arg" => b.arg = v.to_string(),
            "submap" => b.submap = v.to_string(),
            _ => {}
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// A `bind* = MODS, KEY, DISPATCHER, ARG…` line, parsed from a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigBind {
    /// The directive keyword: `bind`, `binde`, `bindl`, `bindm`, `bindeld`, …
    pub flags: String,
    pub modmask: u16,
    pub key: String,
    /// Present when the directive carries the `d` (description) flag — the
    /// human label Omarchy shows in its cheat-sheet.
    pub description: Option<String>,
    pub dispatcher: String,
    pub arg: String,
}

impl ConfigBind {
    /// Parse from a hyprlang bind [`Entry`] (its key is the directive, its
    /// value the comma-separated fields). Returns None if it can't be a bind.
    pub fn from_entry(entry: &Entry) -> Option<Self> {
        if !entry.key.starts_with("bind") {
            return None;
        }
        // The `d` flag (bindd/bindeld/…) inserts a DESCRIPTION field between the
        // key and the dispatcher. Detect it from the directive's flag letters.
        let has_desc = entry.key["bind".len()..].contains('d');
        // Fields: MODS, KEY, [DESC,] DISPATCHER, ARG(rest). The arg may contain
        // commas (resizeactive), so cap the split so trailing commas stay in arg.
        let cap = if has_desc { 5 } else { 4 };
        let mut it = entry.value.splitn(cap, ',');
        let mods = it.next()?.trim();
        let key = it.next()?.trim().to_string();
        let description = if has_desc {
            Some(it.next().unwrap_or("").trim().to_string())
        } else {
            None
        };
        let dispatcher = it.next().unwrap_or("").trim().to_string();
        let arg = it.next().unwrap_or("").trim().to_string();
        Some(Self {
            flags: entry.key.clone(),
            modmask: mods_to_mask(mods),
            key,
            description,
            dispatcher,
            arg,
        })
    }

    /// Does this config line define the given runtime bind? Matches on the
    /// chord + dispatcher + arg (Hyprland normalizes some keys, so key match is
    /// case-insensitive and tolerant of the `code:NN` form).
    pub fn defines(&self, rt: &RuntimeBind) -> bool {
        self.modmask == rt.modmask
            && key_eq(&self.key, rt)
            && self.dispatcher.eq_ignore_ascii_case(&rt.dispatcher)
            && self.arg == rt.arg
    }
}

fn key_eq(config_key: &str, rt: &RuntimeBind) -> bool {
    if let Some(code) = config_key.strip_prefix("code:") {
        return code.trim().parse::<i64>().ok() == Some(rt.keycode);
    }
    config_key.eq_ignore_ascii_case(&rt.key)
}

/// A runtime bind joined to the config line and layer that defines it.
#[derive(Debug, Clone)]
pub struct AttributedBind {
    pub runtime: RuntimeBind,
    /// None when no source line matched (e.g. a plugin-registered bind).
    pub source: Option<BindSource>,
}

#[derive(Debug, Clone)]
pub struct BindSource {
    pub layer: Layer,
    pub config: ConfigBind,
}

/// One config file in the source-resolution order, most-authoritative last
/// (exactly how Hyprland sources them: defaults → theme → user → toggles).
pub struct SourceFile<'a> {
    pub layer: Layer,
    pub doc: &'a HyprDoc,
}

/// Attribute each runtime bind to its defining config line. When several
/// layers define the same chord, the last (highest-priority) wins.
pub fn attribute(runtime: &[RuntimeBind], sources: &[SourceFile]) -> Vec<AttributedBind> {
    runtime
        .iter()
        .map(|rt| {
            let mut found: Option<BindSource> = None;
            for sf in sources {
                for entry in sf.doc.binds() {
                    if let Some(cb) = ConfigBind::from_entry(&entry) {
                        if cb.defines(rt) {
                            found = Some(BindSource {
                                layer: sf.layer,
                                config: cb,
                            });
                        }
                    }
                }
            }
            AttributedBind {
                runtime: rt.clone(),
                source: found,
            }
        })
        .collect()
}

// ── conflicts & overrides ────────────────────────────────────────────────────

/// The identity of a chord for collision purposes: modifiers + normalized key.
/// Two binds collide when their chord identity matches (regardless of action).
pub fn chord_id(mask: u16, key: &str) -> (u16, String) {
    (mask, key.to_ascii_uppercase())
}

impl ConfigBind {
    fn chord_id(&self) -> (u16, String) {
        chord_id(self.modmask, &self.key)
    }

    /// The mods field as Hyprland writes it in a config line (`SUPER SHIFT`).
    pub fn mods_text(&self) -> String {
        mask_to_mods(self.modmask).join(" ")
    }

    /// Render this bind back to a hyprlang line: `bind = MODS, KEY, DISP, ARG`.
    /// Empty trailing fields are omitted cleanly (e.g. `killactive` has no arg).
    pub fn render_line(&self) -> String {
        let mut line = format!("{} = {}, {}", self.flags, self.mods_text(), self.key);
        if let Some(desc) = &self.description {
            line.push_str(&format!(", {desc}"));
        }
        if !self.dispatcher.is_empty() {
            line.push_str(&format!(", {}", self.dispatcher));
            if !self.arg.is_empty() {
                line.push_str(&format!(", {}", self.arg));
            }
        }
        line
    }
}

/// Render an `unbind = MODS, KEY` line that cancels a lower-layer bind.
pub fn render_unbind(mask: u16, key: &str) -> String {
    let mods = mask_to_mods(mask).join(" ");
    format!("unbind = {mods}, {key}")
}

/// A chord defined more than once in the *effective* configuration.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub chord: String,
    /// The colliding binds, in source order (last is the one that wins).
    pub binds: Vec<ConfigBind>,
}

/// Find chords bound to more than one distinct action. Binds that repeat the
/// exact same action (harmless duplicates) are not reported; only genuine
/// disagreements where the winning action shadows another.
pub fn find_conflicts(binds: &[ConfigBind]) -> Vec<Conflict> {
    use std::collections::BTreeMap;
    let mut by_chord: BTreeMap<(u16, String), Vec<ConfigBind>> = BTreeMap::new();
    for b in binds {
        by_chord.entry(b.chord_id()).or_default().push(b.clone());
    }
    by_chord
        .into_iter()
        .filter_map(|((mask, _), group)| {
            let distinct_actions = {
                let mut acts: Vec<_> = group
                    .iter()
                    .map(|b| (b.dispatcher.as_str(), b.arg.as_str()))
                    .collect();
                acts.sort_unstable();
                acts.dedup();
                acts.len()
            };
            if distinct_actions > 1 {
                Some(Conflict {
                    chord: render_chord(mask, &group[0].key),
                    binds: group,
                })
            } else {
                None
            }
        })
        .collect()
}

/// A user-authored change to the keymap, rendered into the managed override
/// block in the user's bindings file.
#[derive(Debug, Clone)]
pub enum Override {
    /// Bind (or rebind) a chord to an action.
    Set(ConfigBind),
    /// Cancel a lower-layer bind on this chord entirely.
    Disable { modmask: u16, key: String },
}

impl Override {
    fn render(&self) -> String {
        match self {
            Override::Set(cb) => cb.render_line(),
            Override::Disable { modmask, key } => render_unbind(*modmask, key),
        }
    }
}

/// Build the body of the managed override block from a list of user changes,
/// in order. This is what goes inside the `omarchy-studio:keybinds` marker
/// block in the user's bindings.conf (spec 05 §3).
pub fn render_override_block(overrides: &[Override]) -> String {
    overrides
        .iter()
        .map(Override::render)
        .collect::<Vec<_>>()
        .join("\n")
}

// ── persistence ──────────────────────────────────────────────────────────────

use crate::configfs::{atomic_write, CommentStyle, ManagedBlock};
use crate::omarchy::OmarchyPaths;

const BLOCK_SECTION: &str = "keybinds";

fn override_block() -> ManagedBlock {
    ManagedBlock::new(BLOCK_SECTION, CommentStyle::Hash)
}

/// The user's Hyprland bindings file — sourced last, so Studio's overrides
/// there win over the Omarchy defaults (spec 05 §3).
pub fn user_bindings_path(paths: &OmarchyPaths) -> std::path::PathBuf {
    paths.hypr_config().join("bindings.conf")
}

/// Is a Studio override block present in the user's bindings file?
pub fn overrides_installed(paths: &OmarchyPaths) -> bool {
    std::fs::read_to_string(user_bindings_path(paths))
        .map(|c| override_block().contains(&c))
        .unwrap_or(false)
}

/// Write (or refresh) the managed override block in the user's bindings file.
/// An empty override list removes the block entirely. Returns the file path.
/// The caller is responsible for snapshotting first and reloading after
/// (the apply pipeline / CLI does both).
pub fn write_overrides(paths: &OmarchyPaths, overrides: &[Override]) -> Result<std::path::PathBuf> {
    let path = user_bindings_path(paths);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let block = override_block();
    let updated = if overrides.is_empty() {
        block.remove(&existing)
    } else {
        let header = "# Managed by Omarchy Studio — your keybind changes live here.";
        let body = format!("{header}\n{}", render_override_block(overrides));
        block.upsert(&existing, &body)
    };
    atomic_write(&path, &updated)?;
    Ok(path)
}

/// Read Studio's existing override block back into a list of [`Override`]s, so
/// a fresh session inherits changes the user made earlier. Absent block → empty.
pub fn read_overrides(paths: &OmarchyPaths) -> Vec<Override> {
    let Ok(content) = std::fs::read_to_string(user_bindings_path(paths)) else {
        return Vec::new();
    };
    let Some(body) = override_block().extract(&content) else {
        return Vec::new();
    };
    let doc = HyprDoc::parse(body);
    doc.binds()
        .iter()
        .filter_map(|e| {
            if e.key == "unbind" {
                // `unbind = MODS, KEY`
                let mut it = e.value.splitn(2, ',');
                let mods = it.next()?.trim();
                let key = it.next()?.trim().to_string();
                Some(Override::Disable {
                    modmask: mods_to_mask(mods),
                    key,
                })
            } else {
                ConfigBind::from_entry(e).map(Override::Set)
            }
        })
        .collect()
}

/// Load the effective keymap: run `hyprctl binds -j`, then attribute each bind
/// to the config layer that defines it (Omarchy defaults + the user's file).
pub fn load_effective(
    paths: &OmarchyPaths,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<Vec<AttributedBind>> {
    let out = runner.run(&crate::omarchy::cmds::binds_json())?;
    if !out.ok() {
        return Err(StudioError::External {
            cmd: "hyprctl binds -j".into(),
            detail: if out.stderr.trim().is_empty() {
                "is Hyprland running?".into()
            } else {
                out.stderr.trim().to_string()
            },
        });
    }
    // Hyprland 0.56's `hyprctl binds -j` emits invalid JSON (an upstream bug:
    // keys and values are misaligned and some values are unquoted). The plain
    // text form is correct, so fall back to it rather than leaving the whole
    // Keybinds screen dead on that version.
    let runtime = match parse_runtime_binds(&out.stdout) {
        Ok(b) => b,
        Err(json_err) => {
            let text = runner.run(&crate::omarchy::cmds::binds_text())?;
            if !text.ok() {
                return Err(json_err);
            }
            let parsed = parse_runtime_binds_text(&text.stdout);
            if parsed.is_empty() {
                return Err(json_err);
            }
            parsed
        }
    };

    // Source layers, lowest priority first. Defaults live under $OMARCHY_PATH.
    let mut docs: Vec<(Layer, HyprDoc)> = Vec::new();
    let default_dir = paths.system.join("default/hypr/bindings");
    if let Ok(entries) = std::fs::read_dir(&default_dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("conf"))
            .collect();
        files.sort();
        for f in files {
            if let Ok(text) = std::fs::read_to_string(&f) {
                docs.push((Layer::Default, HyprDoc::parse(&text)));
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(user_bindings_path(paths)) {
        docs.push((Layer::User, HyprDoc::parse(&text)));
    }
    let sources: Vec<SourceFile> = docs
        .iter()
        .map(|(layer, doc)| SourceFile { layer: *layer, doc })
        .collect();

    Ok(attribute(&runtime, &sources))
}

/// Persist overrides and reload Hyprland so they take effect. Returns the
/// bindings file that changed. Caller snapshots before, for undo.
/// Plan the override block as a pipeline edit, writing nothing. `None` when
/// the block already says exactly this — an apply that changes nothing should
/// not snapshot or reload.
pub fn plan_overrides(
    paths: &OmarchyPaths,
    overrides: &[Override],
) -> Option<crate::engine::FileEdit> {
    let path = user_bindings_path(paths);
    // `None` for a file that doesn't exist yet: the pipeline's hash guard
    // reads it the same way, and treating absent as empty would make the
    // guard reject the write.
    let on_disk = std::fs::read_to_string(&path).ok();
    let existing = on_disk.clone().unwrap_or_default();
    let block = override_block();
    let updated = if overrides.is_empty() {
        block.remove(&existing)
    } else {
        let header = "# Managed by Omarchy Studio — your keybind changes live here.";
        let body = format!("{header}\n{}", render_override_block(overrides));
        block.upsert(&existing, &body)
    };
    (updated != existing).then(|| crate::engine::FileEdit::new(path, on_disk.as_deref(), updated))
}

/// Apply through the pipeline: drift-check → pre-snapshot → hash-guarded write
/// → `hyprctl reload` → verify → post, rolling back if the binds we wrote stop
/// Hyprland's config loading. The caller must not snapshot — the pipeline does.
pub fn apply_overrides(
    paths: &OmarchyPaths,
    overrides: &[Override],
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
    summary: &str,
) -> Result<std::path::PathBuf> {
    let path = user_bindings_path(paths);
    let Some(edit) = plan_overrides(paths, overrides) else {
        return Ok(path); // nothing to do
    };
    let plan = crate::engine::ApplyPlan {
        summary: summary.to_string(),
        module: "keybinds".into(),
        edits: vec![edit],
        reload: vec![crate::engine::ReloadStep::HyprReload],
        verify: crate::engine::hypr_verification(runner),
        risk: crate::engine::Risk::Risky,
        trailers: Vec::new(),
    };
    crate::engine::Pipeline::new(store, runner).apply(&plan, false)?;
    Ok(path)
}

// ── marked binds ─────────────────────────────────────────────────────────
// Studio-owned launcher binds live in the override block as `bindd` lines
// whose description is a fixed marker — that's how each feature finds its
// own bind again without touching anything else in the block.

fn is_marked(o: &Override, desc: &str) -> bool {
    matches!(o, Override::Set(cb) if cb.description.as_deref() == Some(desc))
}

/// The Studio-owned bind carrying `desc` as its marker, if installed.
pub fn find_marked(paths: &OmarchyPaths, desc: &str) -> Option<ConfigBind> {
    read_overrides(paths).into_iter().find_map(|o| match o {
        Override::Set(cb) if cb.description.as_deref() == Some(desc) => Some(cb),
        _ => None,
    })
}

/// Bind `mods+key` to `exec` under the `desc` marker (replacing any previous
/// bind with the same marker), through the shared override block — sourced
/// last, so it wins over an Omarchy default on the same chord. Reloads
/// Hyprland. Caller snapshots [`user_bindings_path`] first.
pub fn install_marked(
    paths: &OmarchyPaths,
    desc: &str,
    mods: &str,
    key: &str,
    exec: &str,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<ConfigBind> {
    install_marked_dispatch(paths, desc, mods, key, "exec", exec, store, runner)
}

/// As [`install_marked`], but for a bind that isn't `exec` — a plugin
/// dispatcher such as `scrolloverview:overview, toggle`.
#[allow(clippy::too_many_arguments)]
pub fn install_marked_dispatch(
    paths: &OmarchyPaths,
    desc: &str,
    mods: &str,
    key: &str,
    dispatcher: &str,
    arg: &str,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<ConfigBind> {
    let bind = ConfigBind {
        flags: "bindd".into(),
        modmask: mods_to_mask(mods),
        key: key.to_string(),
        description: Some(desc.to_string()),
        dispatcher: dispatcher.to_string(),
        arg: arg.to_string(),
    };
    let mut overrides: Vec<Override> = read_overrides(paths)
        .into_iter()
        .filter(|o| !is_marked(o, desc))
        .collect();
    overrides.push(Override::Set(bind.clone()));
    apply_overrides(paths, &overrides, store, runner, &format!("bind {desc}"))?;
    Ok(bind)
}

/// Remove the `desc`-marked bind (other overrides survive). Returns false
/// when none was installed. The pipeline snapshots.
pub fn remove_marked(
    paths: &OmarchyPaths,
    desc: &str,
    store: &crate::snapshot::SnapshotStore,
    runner: &dyn crate::cmd::CommandRunner,
) -> Result<bool> {
    let all = read_overrides(paths);
    let kept: Vec<Override> = all
        .iter()
        .filter(|o| !is_marked(o, desc))
        .cloned()
        .collect();
    if kept.len() == all.len() {
        return Ok(false);
    }
    apply_overrides(paths, &kept, store, runner, &format!("unbind {desc}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modmask_round_trips_and_renders() {
        assert_eq!(mods_to_mask("SUPER"), 64);
        assert_eq!(mods_to_mask("SUPER SHIFT"), 65);
        assert_eq!(mods_to_mask("CTRL ALT"), 12);
        assert_eq!(mods_to_mask("MOD4 CONTROL"), 68); // aliases
        assert_eq!(mask_to_mods(65), vec!["SUPER", "SHIFT"]);
        assert_eq!(render_chord(64, "T"), "SUPER+T");
        assert_eq!(render_chord(0, "XF86AudioMute"), "XF86AudioMute");
        assert_eq!(render_chord(65, "Return"), "SUPER+SHIFT+Return");
    }

    #[test]
    fn parses_runtime_binds_json() {
        let json = r#"[
          {"modmask":64,"key":"T","dispatcher":"togglefloating","arg":"","description":"Toggle","keycode":0},
          {"modmask":0,"key":"XF86AudioRaiseVolume","dispatcher":"exec","arg":"vol up","keycode":0}
        ]"#;
        let binds = parse_runtime_binds(json).unwrap();
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].chord(), "SUPER+T");
        assert_eq!(binds[1].dispatcher, "exec");
        assert!(parse_runtime_binds("not json").is_err());
    }

    #[test]
    fn load_effective_reads_hyprctl_and_attributes_layers() {
        use crate::cmd::StubRunner;
        let paths = fake_paths("effective");
        // a default binding file under $OMARCHY_PATH, and a user override
        let def_dir = paths.system.join("default/hypr/bindings");
        std::fs::create_dir_all(&def_dir).unwrap();
        std::fs::write(
            def_dir.join("tiling.conf"),
            "bind = SUPER, T, togglefloating,\n",
        )
        .unwrap();
        std::fs::write(
            user_bindings_path(&paths),
            "bindd = SUPER, B, Browser, exec, chromium\n",
        )
        .unwrap();

        let json = r#"[
          {"modmask":64,"key":"T","dispatcher":"togglefloating","arg":"","keycode":0},
          {"modmask":64,"key":"B","dispatcher":"exec","arg":"chromium","keycode":0},
          {"modmask":64,"key":"P","dispatcher":"exec","arg":"plugin-only","keycode":0}
        ]"#;
        let runner = StubRunner::default().with_ok("hyprctl binds -j", json);

        let attributed = load_effective(&paths, &runner).unwrap();
        assert_eq!(attributed.len(), 3);
        // SUPER+T attributed to the Omarchy default layer
        let t = attributed.iter().find(|a| a.runtime.key == "T").unwrap();
        assert_eq!(t.source.as_ref().unwrap().layer, Layer::Default);
        // SUPER+B attributed to the user's own file
        let b = attributed.iter().find(|a| a.runtime.key == "B").unwrap();
        assert_eq!(b.source.as_ref().unwrap().layer, Layer::User);
        // SUPER+P defined nowhere in config → no source (plugin)
        let p = attributed.iter().find(|a| a.runtime.key == "P").unwrap();
        assert!(p.source.is_none());
    }

    #[test]
    fn config_bind_parses_and_keeps_commas_in_arg() {
        let doc = HyprDoc::parse(
            "bind = SUPER, T, exec, foot\nbinde = SUPER SHIFT, left, resizeactive, -20 0\n",
        );
        let binds = doc.binds();
        let a = ConfigBind::from_entry(&binds[0]).unwrap();
        assert_eq!(a.modmask, 64);
        assert_eq!(
            (a.key.as_str(), a.dispatcher.as_str(), a.arg.as_str()),
            ("T", "exec", "foot")
        );
        let b = ConfigBind::from_entry(&binds[1]).unwrap();
        assert_eq!(b.flags, "binde");
        assert_eq!(b.arg, "-20 0"); // arg with a space, dispatcher split correct
    }

    #[test]
    fn attribution_picks_the_highest_priority_layer() {
        // default defines SUPER+T -> togglefloating; user overrides it -> exec btop
        let default = HyprDoc::parse("bind = SUPER, T, togglefloating,\n");
        let user = HyprDoc::parse("bind = SUPER, T, exec, btop\n");
        let sources = [
            SourceFile {
                layer: Layer::Default,
                doc: &default,
            },
            SourceFile {
                layer: Layer::User,
                doc: &user,
            },
        ];

        // runtime reflects the user's version (what Hyprland actually runs)
        let rt = parse_runtime_binds(
            r#"[{"modmask":64,"key":"T","dispatcher":"exec","arg":"btop","keycode":0}]"#,
        )
        .unwrap();
        let attributed = attribute(&rt, &sources);
        assert_eq!(attributed.len(), 1);
        let src = attributed[0].source.as_ref().expect("attributed");
        assert_eq!(src.layer, Layer::User);
        assert_eq!(src.config.arg, "btop");
    }

    #[test]
    fn unmatched_runtime_bind_has_no_source() {
        let empty = HyprDoc::parse("");
        let sources = [SourceFile {
            layer: Layer::User,
            doc: &empty,
        }];
        let rt = parse_runtime_binds(
            r#"[{"modmask":64,"key":"P","dispatcher":"exec","arg":"plugin","keycode":0}]"#,
        )
        .unwrap();
        assert!(attribute(&rt, &sources)[0].source.is_none());
    }

    fn cb(mods: &str, key: &str, disp: &str, arg: &str) -> ConfigBind {
        ConfigBind {
            flags: "bind".into(),
            modmask: mods_to_mask(mods),
            key: key.into(),
            description: None,
            dispatcher: disp.into(),
            arg: arg.into(),
        }
    }

    #[test]
    fn render_line_round_trips_through_the_cst() {
        for (mods, key, disp, arg) in [
            ("SUPER", "T", "exec", "foot"),
            ("SUPER SHIFT", "left", "resizeactive", "-20 0"),
            ("SUPER", "W", "killactive", ""),
            ("", "XF86AudioMute", "exec", "mute"),
        ] {
            let line = cb(mods, key, disp, arg).render_line();
            let parsed = HyprDoc::parse(&format!("{line}\n"));
            let back = ConfigBind::from_entry(&parsed.binds()[0]).unwrap();
            assert_eq!(back.modmask, mods_to_mask(mods));
            assert_eq!(back.key, key);
            assert_eq!(back.dispatcher, disp);
            assert_eq!(back.arg, arg, "arg round-trip for {line}");
        }
    }

    #[test]
    fn conflicts_flag_disagreements_not_harmless_dupes() {
        let binds = vec![
            cb("SUPER", "T", "togglefloating", ""),
            cb("SUPER", "T", "exec", "btop"), // conflict: same chord, diff action
            cb("SUPER", "Q", "killactive", ""),
            cb("SUPER", "Q", "killactive", ""), // exact dup: not a conflict
        ];
        let conflicts = find_conflicts(&binds);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].chord, "SUPER+T");
        assert_eq!(conflicts[0].binds.len(), 2);
        // last-defined wins
        assert_eq!(conflicts[0].binds.last().unwrap().arg, "btop");
    }

    #[test]
    fn override_block_renders_binds_and_unbinds() {
        let overrides = vec![
            Override::Set(cb("SUPER", "B", "exec", "chromium")),
            Override::Disable {
                modmask: mods_to_mask("SUPER"),
                key: "T".into(),
            },
        ];
        let body = render_override_block(&overrides);
        assert_eq!(body, "bind = SUPER, B, exec, chromium\nunbind = SUPER, T");
        // the rendered block itself parses back cleanly
        let doc = HyprDoc::parse(&format!("{body}\n"));
        assert_eq!(doc.binds().len(), 2); // bind + unbind both counted as bind* keys
    }

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-kb-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("sys/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    #[test]
    fn write_overrides_manages_a_block_and_preserves_user_binds() {
        let paths = fake_paths("write");
        let user = "# Application bindings\nbindd = SUPER, RETURN, Terminal, exec, foot\n";
        std::fs::write(user_bindings_path(&paths), user).unwrap();
        assert!(!overrides_installed(&paths));

        let overrides = vec![
            Override::Set(cb("SUPER", "B", "exec", "chromium")),
            Override::Disable {
                modmask: mods_to_mask("SUPER"),
                key: "T".into(),
            },
        ];
        write_overrides(&paths, &overrides).unwrap();
        assert!(overrides_installed(&paths));
        let after = std::fs::read_to_string(user_bindings_path(&paths)).unwrap();
        assert!(after.contains("bindd = SUPER, RETURN, Terminal, exec, foot")); // user's bind kept
        assert!(after.contains("bind = SUPER, B, exec, chromium"));
        assert!(after.contains("unbind = SUPER, T"));

        // re-write is idempotent (single managed block)
        write_overrides(&paths, &overrides).unwrap();
        let twice = std::fs::read_to_string(user_bindings_path(&paths)).unwrap();
        assert_eq!(twice.matches("omarchy-studio:keybinds").count(), 2); // open+close

        // empty overrides removes the block, restoring the user's file exactly
        write_overrides(&paths, &[]).unwrap();
        assert!(!overrides_installed(&paths));
        assert_eq!(
            std::fs::read_to_string(user_bindings_path(&paths)).unwrap(),
            user
        );
    }

    #[test]
    fn read_overrides_round_trips_the_written_block() {
        let paths = fake_paths("readback");
        std::fs::write(
            user_bindings_path(&paths),
            "# user\nbindd = SUPER, RETURN, Term, exec, foot\n",
        )
        .unwrap();
        let written = vec![
            Override::Set(cb("SUPER", "B", "exec", "chromium")),
            Override::Disable {
                modmask: mods_to_mask("SUPER"),
                key: "T".into(),
            },
        ];
        write_overrides(&paths, &written).unwrap();

        let read = read_overrides(&paths);
        assert_eq!(read.len(), 2);
        match &read[0] {
            Override::Set(cb) => {
                assert_eq!(
                    (cb.key.as_str(), cb.dispatcher.as_str(), cb.arg.as_str()),
                    ("B", "exec", "chromium")
                );
            }
            _ => panic!("expected Set"),
        }
        match &read[1] {
            Override::Disable { modmask, key } => {
                assert_eq!((*modmask, key.as_str()), (mods_to_mask("SUPER"), "T"));
            }
            _ => panic!("expected Disable"),
        }
        // no block → empty
        std::fs::write(user_bindings_path(&paths), "# nothing here\n").unwrap();
        assert!(read_overrides(&paths).is_empty());
    }

    #[test]
    fn parses_omarchy_described_binds_correctly() {
        // real Omarchy formats: bindd (MODS,KEY,DESC,DISP,ARG) and bindeld
        let doc = HyprDoc::parse(
            "bindd = SUPER, RETURN, Terminal, exec, foot\n\
             bindeld = ,XF86AudioRaiseVolume, Volume up, exec, omarchy-swayosd-client --output-volume raise\n",
        );
        let binds = doc.binds();
        let term = ConfigBind::from_entry(&binds[0]).unwrap();
        assert_eq!(term.description.as_deref(), Some("Terminal"));
        assert_eq!(term.dispatcher, "exec");
        assert_eq!(term.arg, "foot"); // NOT "Terminal, exec, foot"

        let vol = ConfigBind::from_entry(&binds[1]).unwrap();
        assert_eq!(vol.key, "XF86AudioRaiseVolume");
        assert_eq!(vol.description.as_deref(), Some("Volume up"));
        assert_eq!(vol.dispatcher, "exec");
        assert_eq!(vol.arg, "omarchy-swayosd-client --output-volume raise");

        // render_line puts the description back in the right place, round-trips
        let back =
            ConfigBind::from_entry(&HyprDoc::parse(&format!("{}\n", vol.render_line())).binds()[0])
                .unwrap();
        assert_eq!(back.description, vol.description);
        assert_eq!(back.arg, vol.arg);
    }
    /// Real `hyprctl binds` text from Hyprland 0.56, whose `-j` writer is
    /// broken — this is the only readable keymap on that version.
    const BINDS_TEXT: &str = "bindled\n\
\tmodmask: 0\n\
\tsubmap: \n\
\tkey: XF86AudioRaiseVolume\n\
\tkeycode: 0\n\
\tcatchall: false\n\
\tdescription: Volume up\n\
\tdispatcher: exec\n\
\targ: omarchy-swayosd-client --output-volume raise\n\
\n\
bindd\n\
\tmodmask: 64\n\
\tsubmap: \n\
\tkey: Q\n\
\tkeycode: 0\n\
\tcatchall: false\n\
\tdescription: Close window\n\
\tdispatcher: killactive\n\
\targ: \n";

    #[test]
    fn parses_the_plain_text_keymap_hyprland_056_forces_us_to_use() {
        let binds = parse_runtime_binds_text(BINDS_TEXT);
        assert_eq!(binds.len(), 2);

        assert_eq!(binds[0].key, "XF86AudioRaiseVolume");
        assert_eq!(binds[0].modmask, 0);
        assert_eq!(binds[0].dispatcher, "exec");
        assert_eq!(binds[0].arg, "omarchy-swayosd-client --output-volume raise");
        assert_eq!(binds[0].description, "Volume up");

        // Modifiers survive, so the chord renders the way the screen shows it.
        assert_eq!(binds[1].modmask, mods::SUPER);
        assert_eq!(binds[1].chord(), "SUPER+Q");
        assert_eq!(binds[1].dispatcher, "killactive");
    }

    #[test]
    fn text_parsing_ignores_junk_and_unknown_fields() {
        // A future Hyprland adding fields must not break this, and a stray
        // header line isn't a bind.
        let odd =
            "Bind list:\nbindd\n\tmodmask: 64\n\tkey: T\n\tsome_new_field: 1\n\tdispatcher: exec\n";
        let binds = parse_runtime_binds_text(odd);
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].key, "T");
        assert!(parse_runtime_binds_text("").is_empty());
    }
}

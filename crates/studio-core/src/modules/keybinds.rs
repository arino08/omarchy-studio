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

    /// Render this bind as Omarchy 4 Lua.
    ///
    /// `o.bind(chord, description, command)` covers a command binding, which is
    /// what Studio installs and what users create. It cannot cover a bind whose
    /// action Studio can't see: in Lua mode Hyprland reports *every* bind's
    /// dispatcher as `__lua` (an opaque callback id), so a bind read back from
    /// `hyprctl binds` carries no reproducible action. Rather than emit a bind
    /// that would silently do nothing, this reports why.
    pub fn render_lua(&self) -> std::result::Result<String, String> {
        use crate::configfs::lua::Value;
        let chord = render_lua_chord(self.modmask, &self.key);
        if self.dispatcher == LUA_OPAQUE_DISPATCHER {
            return Err(format!(
                "{chord}: Studio couldn't find where this key's action is defined, so it \
                 can't move it to another chord — you can still disable a key, or bind \
                 one to a command."
            ));
        }
        let desc = self
            .description
            .clone()
            .unwrap_or_else(|| "Omarchy Studio".to_string());
        // `o.bind` takes either a command string (wrapped in exec) or a
        // dispatcher value. Only the dispatchers Studio itself installs are
        // mapped — anything else is refused rather than guessed at, with one
        // exception: `namespace:method` (a plugin dispatcher — Hyprland's own
        // registration convention, `HyprlandAPI::addDispatcherV2`) maps to a
        // call into that plugin's `hl.plugin` table, exactly the form plugin
        // READMEs document (e.g. ScrollOverview's own
        // `hl.plugin.scrolloverview.overview("toggle all")`).
        let action = match self.dispatcher.as_str() {
            "exec" => {
                if self.arg.trim().is_empty() {
                    return Err(format!("{chord}: no command to bind."));
                }
                Value::Str(self.arg.clone()).render()
            }
            "layoutmsg" => format!("hl.dsp.layout({})", Value::Str(self.arg.clone()).render()),
            // Recovered from the Lua sources by `resolve_lua_action` — already
            // an expression Omarchy wrote, so it goes out untouched.
            LUA_RAW_DISPATCHER => self.arg.clone(),
            other => {
                let Some((namespace, method)) = other.split_once(':') else {
                    return Err(format!(
                        "{chord}: Studio can't write the `{other}` dispatcher on Omarchy 4."
                    ));
                };
                let arg = if self.arg.is_empty() {
                    String::new()
                } else {
                    Value::Str(self.arg.clone()).render()
                };
                format!(
                    "function() hl.plugin{}{}({arg}) end",
                    crate::configfs::lua::render_member(namespace),
                    crate::configfs::lua::render_member(method),
                )
            }
        };
        Ok(format!(
            "o.bind({}, {}, {action})",
            Value::Str(chord).render(),
            Value::Str(desc).render(),
        ))
    }
}

/// What `hyprctl binds` reports for the dispatcher of *any* bind when Hyprland
/// runs the Lua config — the real action sits behind a callback we can't read.
pub const LUA_OPAQUE_DISPATCHER: &str = "__lua";

/// A chord as Hyprland's Lua API spells it: `SUPER + SHIFT + T`, or a bare key
/// when there are no modifiers. (hyprlang used `SUPER SHIFT, T`.)
pub fn render_lua_chord(mask: u16, key: &str) -> String {
    let mut parts = mask_to_mods(mask);
    parts.push(key);
    parts.join(" + ")
}

/// `hl.unbind("SUPER + T")` — cancels a lower-layer bind on this chord.
pub fn render_lua_unbind(mask: u16, key: &str) -> String {
    format!(
        "hl.unbind({})",
        crate::configfs::lua::Value::Str(render_lua_chord(mask, key)).render()
    )
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

    /// The Omarchy 4 form. `Err` carries a user-facing reason when the change
    /// can't be expressed in Lua — see [`ConfigBind::render_lua`].
    fn render_lua(&self) -> std::result::Result<String, String> {
        match self {
            Override::Set(cb) => cb.render_lua(),
            Override::Disable { modmask, key } => Ok(render_lua_unbind(*modmask, key)),
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

use crate::configfs::{atomic_write, ManagedBlock};
use crate::omarchy::{Dialect, OmarchyPaths};

const BLOCK_SECTION: &str = "keybinds";

/// The managed-block body for these overrides, or `None` when there are none
/// (the block should be removed). `Err` lists the changes that can't be
/// expressed in this dialect — only possible on Lua, see
/// [`ConfigBind::render_lua`].
fn override_body(
    overrides: &[Override],
    dialect: Dialect,
) -> std::result::Result<Option<String>, String> {
    if overrides.is_empty() {
        return Ok(None);
    }
    if dialect.is_lua() {
        let mut lines = Vec::with_capacity(overrides.len());
        let mut refused = Vec::new();
        for o in overrides {
            match o.render_lua() {
                Ok(line) => lines.push(line),
                Err(why) => refused.push(why),
            }
        }
        if !refused.is_empty() {
            return Err(refused.join("\n"));
        }
        return Ok(Some(format!(
            "-- Managed by Omarchy Studio — your keybind changes live here.\n{}",
            lines.join("\n")
        )));
    }
    Ok(Some(format!(
        "# Managed by Omarchy Studio — your keybind changes live here.\n{}",
        render_override_block(overrides)
    )))
}

fn override_block_for(dialect: Dialect) -> ManagedBlock {
    ManagedBlock::new(BLOCK_SECTION, dialect.comment_style())
}

/// The user's Hyprland bindings file — loaded last, so Studio's overrides
/// there win over the Omarchy defaults (spec 05 §3). `bindings.lua` on
/// Omarchy 4, `bindings.conf` before it.
pub fn user_bindings_path(paths: &OmarchyPaths) -> std::path::PathBuf {
    user_bindings_path_for(paths, Dialect::Hyprlang)
}

pub fn user_bindings_path_for(paths: &OmarchyPaths, dialect: Dialect) -> std::path::PathBuf {
    paths
        .hypr_config()
        .join(format!("bindings.{}", dialect.ext()))
}

/// Is a Studio override block present in the user's bindings file?
pub fn overrides_installed(paths: &OmarchyPaths) -> bool {
    overrides_installed_for(paths, Dialect::probe(&crate::cmd::RealRunner))
}

pub fn overrides_installed_for(paths: &OmarchyPaths, dialect: Dialect) -> bool {
    std::fs::read_to_string(user_bindings_path_for(paths, dialect))
        .map(|c| override_block_for(dialect).contains(&c))
        .unwrap_or(false)
}

/// Write (or refresh) the managed override block in the user's bindings file.
/// An empty override list removes the block entirely. Returns the file path.
/// The caller is responsible for snapshotting first and reloading after
/// (the apply pipeline / CLI does both).
pub fn write_overrides(paths: &OmarchyPaths, overrides: &[Override]) -> Result<std::path::PathBuf> {
    write_overrides_for(paths, overrides, Dialect::Hyprlang)
}

/// As [`write_overrides`], for a given dialect.
///
/// On Lua, an override Studio can't express is a hard error and *nothing* is
/// written: a partial block would quietly drop the change the user asked for,
/// which is precisely the failure mode this port exists to remove. So is an
/// existing block holding a statement Studio can't read back exactly (see
/// `lua_block_problem`) — rewriting it would mangle that statement.
pub fn write_overrides_for(
    paths: &OmarchyPaths,
    overrides: &[Override],
    dialect: Dialect,
) -> Result<std::path::PathBuf> {
    let path = user_bindings_path_for(paths, dialect);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let block = override_block_for(dialect);
    let refuse = |detail| StudioError::External {
        cmd: "keybinds".into(),
        detail,
    };
    if dialect.is_lua() {
        if let Some(why) = lua_block_problem(&path, &existing) {
            return Err(refuse(why));
        }
    }
    let body = override_body(overrides, dialect).map_err(refuse)?;
    let updated = match body {
        Some(body) => block.upsert(&existing, &body),
        None => block.remove(&existing),
    };
    atomic_write(&path, &updated)?;
    Ok(path)
}

/// Read Studio's existing override block back into a list of [`Override`]s, so
/// a fresh session inherits changes the user made earlier. Absent block → empty.
/// Reads whichever bindings file this machine's Hyprland actually loads.
pub fn read_overrides(paths: &OmarchyPaths) -> Vec<Override> {
    read_overrides_for(paths, Dialect::probe(&crate::cmd::RealRunner))
}

/// As [`read_overrides`], for a given dialect.
pub fn read_overrides_for(paths: &OmarchyPaths, dialect: Dialect) -> Vec<Override> {
    let Ok(content) = std::fs::read_to_string(user_bindings_path_for(paths, dialect)) else {
        return Vec::new();
    };
    let Some(body) = override_block_for(dialect).extract(&content) else {
        return Vec::new();
    };
    if dialect.is_lua() {
        return parse_lua_overrides(body).0;
    }
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

/// Read back the Lua override block Studio wrote — the inverse of
/// [`Override::render_lua`]. Overrides that read back cleanly come first;
/// the second list names, by 1-based line within `body`, every statement
/// that didn't. Reading is lenient (callers just get the overrides), but a
/// *write* must not proceed past a problem: re-rendering the block would
/// change or drop that statement — see [`lua_block_problem`].
fn parse_lua_overrides(body: &str) -> (Vec<Override>, Vec<(usize, String)>) {
    use crate::configfs::lua;
    let calls = match lua::parse_call_statements(body) {
        Ok(calls) => calls,
        Err((line, source)) => {
            // Statements before the unreadable one are still worth showing.
            let head: String = body.lines().take(line - 1).collect::<Vec<_>>().join("\n");
            let read = lua::parse_call_statements(&head).unwrap_or_default();
            let found = read.iter().filter_map(lua_override).collect();
            return (found, vec![(line, source)]);
        }
    };
    let mut found = Vec::new();
    let mut problems = Vec::new();
    for call in &calls {
        match lua_override(call) {
            Some(o) => found.push(o),
            None => {
                let first = call.text.lines().next().unwrap_or_default().trim();
                problems.push((call.line, first.to_string()));
            }
        }
    }
    (found, problems)
}

/// One `hl.unbind(…)` / `o.bind(…)` statement as an [`Override`] — but only
/// if rendering that override reproduces the statement token for token.
/// Anything else (an options table, a computed chord, a call Studio never
/// writes) is `None`, never an approximation.
fn lua_override(call: &crate::configfs::lua::Call) -> Option<Override> {
    use crate::configfs::lua;
    let faithful = |o: Override| {
        let rendered = o.render_lua().ok()?;
        lua::same_tokens(&call.text, &rendered).then_some(o)
    };
    match (call.callee.as_str(), call.args.as_slice()) {
        ("hl.unbind", [chord]) => {
            let (modmask, key) = split_lua_chord(&lua::as_string_literal(chord)?);
            faithful(Override::Disable { modmask, key })
        }
        ("o.bind", [chord, desc, action]) => {
            let (modmask, key) = split_lua_chord(&lua::as_string_literal(chord)?);
            let desc = lua::as_string_literal(desc)?;
            let set = |dispatcher: String, arg: String| {
                Override::Set(ConfigBind {
                    flags: "bind".into(),
                    modmask,
                    key: key.clone(),
                    description: Some(desc.clone()),
                    dispatcher,
                    arg,
                })
            };
            // What the action *is* comes from its shape: a plain string
            // literal is a command; `hl.dsp.layout("…")` is the one core
            // dispatcher Studio maps by name; `function() hl.plugin.ns.method(…)
            // end` is the plugin-dispatcher form [`ConfigBind::render_lua`]
            // emits. Anything else — including a multi-line function — is kept
            // as the verbatim expression, which always re-renders faithfully.
            let structured = if let Some(cmd) = lua::as_string_literal(action) {
                Some(("exec".to_string(), cmd))
            } else if let Some(inner) = action
                .strip_prefix("hl.dsp.layout(")
                .and_then(|a| a.strip_suffix(')'))
            {
                lua::as_string_literal(inner).map(|msg| ("layoutmsg".to_string(), msg))
            } else {
                parse_lua_plugin_call(action)
            };
            structured
                .and_then(|(d, a)| faithful(set(d, a)))
                .or_else(|| faithful(set(LUA_RAW_DISPATCHER.into(), action.clone())))
        }
        _ => None,
    }
}

/// Why the Lua keybinds block in `content` (a whole `bindings.lua`) can't be
/// rewritten safely, if it can't: the file line of each statement Studio
/// couldn't read back exactly. Re-rendering the block from what *was* read
/// would silently rewrite or drop those statements — the bug where a
/// multi-line `function() … end` bind came back as invalid `function())`.
fn lua_block_problem(path: &std::path::Path, content: &str) -> Option<String> {
    let body = override_block_for(Dialect::Lua).extract(content)?;
    let (_, problems) = parse_lua_overrides(body);
    if problems.is_empty() {
        return None;
    }
    // `extract` hands back a slice of `content`, so its offset gives the line
    // the body starts on.
    let offset = body.as_ptr() as usize - content.as_ptr() as usize;
    let first_line = content[..offset].matches('\n').count();
    let lines: Vec<String> = problems
        .iter()
        .map(|(line, source)| format!("{}:{}: {source}", path.display(), first_line + line))
        .collect();
    Some(format!(
        "Studio can't read back every line of its keybinds block, so it won't rewrite \
         the block (that would change or drop them):\n{}\nFix or remove {} by hand, \
         then try again.",
        lines.join("\n"),
        if lines.len() == 1 {
            "that line"
        } else {
            "those lines"
        },
    ))
}

/// `"SUPER + SHIFT + T"` → (mask, `T`).
fn split_lua_chord(chord: &str) -> (u16, String) {
    let mut parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let key = parts.pop().unwrap_or_default().to_string();
    (mods_to_mask(&parts.join(" ")), key)
}

/// The first quoted Lua string in `text`, unescaped.
fn first_lua_string(text: &str) -> Option<String> {
    lua_string_args(text).into_iter().next()
}

/// Every top-level quoted Lua string in an argument list, in order.
fn lua_string_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        loop {
            match chars.next() {
                None | Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some(other) => s.push(other),
                    None => break,
                },
                Some(other) => s.push(other),
            }
        }
        out.push(s);
    }
    out
}

/// The inverse of the plugin-dispatcher branch of [`ConfigBind::render_lua`]:
/// `function() hl.plugin.ns.method("arg") end` → `("ns:method", "arg")`.
/// `None` for anything else, so the caller falls back to storing the
/// expression verbatim rather than guessing.
fn parse_lua_plugin_call(expr: &str) -> Option<(String, String)> {
    let inner = expr
        .strip_prefix("function() hl.plugin")?
        .strip_suffix(" end")?
        .trim();
    let paren = inner.find('(')?;
    let members = split_lua_members(&inner[..paren])?;
    let [namespace, method] = <[String; 2]>::try_from(members).ok()?;
    let call_args = inner[paren..].strip_prefix('(')?.strip_suffix(')')?.trim();
    let arg = if call_args.is_empty() {
        String::new()
    } else {
        crate::configfs::lua::as_string_literal(call_args)?
    };
    Some((format!("{namespace}:{method}"), arg))
}

/// Split a chain of `.name` / `["name"]` table accesses — the inverse of
/// [`crate::configfs::lua::render_member`] — into its member names. `None` on
/// anything that isn't exactly that shape.
fn split_lua_members(mut s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    while !s.is_empty() {
        if let Some(rest) = s.strip_prefix('.') {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if end == 0 {
                return None;
            }
            out.push(rest[..end].to_string());
            s = &rest[end..];
        } else if let Some(rest) = s.strip_prefix('[') {
            let name = first_lua_string(rest)?;
            let close = rest.find(']')?;
            out.push(name);
            s = &rest[close + 1..];
        } else {
            return None;
        }
    }
    Some(out)
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
    plan_overrides_for(paths, overrides, Dialect::Hyprlang).unwrap_or(None)
}

/// As [`plan_overrides`], for a given dialect. `Err` when an override can't be
/// expressed — the caller surfaces it instead of writing a partial block.
pub fn plan_overrides_for(
    paths: &OmarchyPaths,
    overrides: &[Override],
    dialect: Dialect,
) -> std::result::Result<Option<crate::engine::FileEdit>, String> {
    let path = user_bindings_path_for(paths, dialect);
    // `None` for a file that doesn't exist yet: the pipeline's hash guard
    // reads it the same way, and treating absent as empty would make the
    // guard reject the write.
    let on_disk = std::fs::read_to_string(&path).ok();
    let existing = on_disk.clone().unwrap_or_default();
    if dialect.is_lua() {
        if let Some(why) = lua_block_problem(&path, &existing) {
            return Err(why);
        }
    }
    let block = override_block_for(dialect);
    let updated = match override_body(overrides, dialect)? {
        Some(body) => block.upsert(&existing, &body),
        None => block.remove(&existing),
    };
    Ok((updated != existing)
        .then(|| crate::engine::FileEdit::new(path, on_disk.as_deref(), updated)))
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
    let dialect = Dialect::probe(runner);
    let path = user_bindings_path_for(paths, dialect);
    let planned =
        plan_overrides_for(paths, overrides, dialect).map_err(|detail| StudioError::External {
            cmd: "keybinds".into(),
            detail,
        })?;
    let Some(edit) = planned else {
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
    find_marked_for(paths, desc, Dialect::probe(&crate::cmd::RealRunner))
}

/// As [`find_marked`], for a given dialect.
pub fn find_marked_for(paths: &OmarchyPaths, desc: &str, dialect: Dialect) -> Option<ConfigBind> {
    read_overrides_for(paths, dialect)
        .into_iter()
        .find_map(|o| match o {
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
    // Read through the same runner we're about to write with, so the read and
    // the write can't disagree about which dialect this machine uses.
    let mut overrides: Vec<Override> = read_overrides_for(paths, Dialect::probe(runner))
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
    // Same runner for the read and the write — see install_marked_dispatch.
    let all = read_overrides_for(paths, Dialect::probe(runner));
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
        assert!(!overrides_installed_for(&paths, Dialect::Hyprlang));

        let overrides = vec![
            Override::Set(cb("SUPER", "B", "exec", "chromium")),
            Override::Disable {
                modmask: mods_to_mask("SUPER"),
                key: "T".into(),
            },
        ];
        write_overrides(&paths, &overrides).unwrap();
        assert!(overrides_installed_for(&paths, Dialect::Hyprlang));
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
        assert!(!overrides_installed_for(&paths, Dialect::Hyprlang));
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

        let read = read_overrides_for(&paths, Dialect::Hyprlang);
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
        assert!(read_overrides_for(&paths, Dialect::Hyprlang).is_empty());
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

/// Omarchy 4: `hl.unbind` / `o.bind` in `bindings.lua`.
#[cfg(test)]
mod lua_tests {
    use super::*;

    fn fake_paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-kb-lua-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    fn exec_bind(mask: u16, key: &str, desc: &str, cmd: &str) -> ConfigBind {
        ConfigBind {
            flags: "bind".into(),
            modmask: mask,
            key: key.into(),
            description: Some(desc.into()),
            dispatcher: "exec".into(),
            arg: cmd.into(),
        }
    }

    #[test]
    fn chords_use_the_lua_plus_form() {
        assert_eq!(
            render_lua_chord(mods::SUPER | mods::SHIFT, "T"),
            "SUPER + SHIFT + T"
        );
        assert_eq!(render_lua_chord(0, "XF86AudioMute"), "XF86AudioMute");
        assert_eq!(
            render_lua_unbind(mods::SUPER, "F"),
            "hl.unbind(\"SUPER + F\")"
        );
    }

    #[test]
    fn a_command_bind_renders_as_o_bind() {
        let bind = exec_bind(mods::SUPER, "B", "Browser", "chromium");
        assert_eq!(
            bind.render_lua().unwrap(),
            "o.bind(\"SUPER + B\", \"Browser\", \"chromium\")"
        );
    }

    /// The important refusal: on Omarchy 4 every runtime bind reports `__lua`,
    /// so "move this action to another key" has nothing to copy. Studio must
    /// say so rather than write a binding that does nothing.
    #[test]
    fn refuses_to_fake_an_opaque_lua_action() {
        let mut bind = exec_bind(mods::SUPER, "W", "Close window", "");
        bind.dispatcher = LUA_OPAQUE_DISPATCHER.into();
        let err = bind.render_lua().unwrap_err();
        assert!(err.contains("SUPER + W"));
        assert!(err.contains("can't move it to another chord"));

        let paths = fake_paths("refuse");
        let err = write_overrides_for(&paths, &[Override::Set(bind)], Dialect::Lua).unwrap_err();
        assert!(format!("{err:?}").contains("SUPER + W"));
        assert!(
            !paths.hypr_config().join("bindings.lua").exists(),
            "nothing is written when part of the change can't be expressed"
        );
    }

    #[test]
    fn writes_unbind_and_bind_into_bindings_lua() {
        let paths = fake_paths("write");
        let overrides = vec![
            Override::Disable {
                modmask: mods::SUPER,
                key: "F".into(),
            },
            Override::Set(exec_bind(mods::SUPER, "B", "Browser", "chromium")),
        ];
        let path = write_overrides_for(&paths, &overrides, Dialect::Lua).unwrap();
        assert_eq!(path, paths.hypr_config().join("bindings.lua"));
        assert!(!paths.hypr_config().join("bindings.conf").exists());

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("-- >>> omarchy-studio:keybinds"));
        assert!(text.contains("hl.unbind(\"SUPER + F\")"));
        assert!(text.contains("o.bind(\"SUPER + B\", \"Browser\", \"chromium\")"));
        assert!(!text.contains("bind ="), "no hyprlang syntax");
    }

    #[test]
    fn reads_its_own_lua_block_back() {
        let paths = fake_paths("roundtrip");
        let overrides = vec![
            Override::Disable {
                modmask: mods::SUPER | mods::SHIFT,
                key: "Q".into(),
            },
            Override::Set(exec_bind(
                mods::SUPER,
                "B",
                "Browser",
                "chromium --new-window",
            )),
        ];
        write_overrides_for(&paths, &overrides, Dialect::Lua).unwrap();

        let back = read_overrides_for(&paths, Dialect::Lua);
        assert_eq!(back.len(), 2);
        match &back[0] {
            Override::Disable { modmask, key } => {
                assert_eq!(*modmask, mods::SUPER | mods::SHIFT);
                assert_eq!(key, "Q");
            }
            other => panic!("expected a disable, got {other:?}"),
        }
        match &back[1] {
            Override::Set(cb) => {
                assert_eq!(cb.modmask, mods::SUPER);
                assert_eq!(cb.key, "B");
                assert_eq!(cb.description.as_deref(), Some("Browser"));
                assert_eq!(cb.dispatcher, "exec");
                assert_eq!(cb.arg, "chromium --new-window");
            }
            other => panic!("expected a bind, got {other:?}"),
        }
    }

    #[test]
    fn clearing_leaves_the_users_own_lua_untouched() {
        let paths = fake_paths("clear");
        let path = paths.hypr_config().join("bindings.lua");
        let users =
            "-- mine\nhl.unbind(\"ALT + SPACE\")\no.bind(\"SUPER + G\", \"Games\", \"steam\")\n";
        std::fs::write(&path, users).unwrap();

        write_overrides_for(
            &paths,
            &[Override::Set(exec_bind(
                mods::SUPER,
                "B",
                "Browser",
                "chromium",
            ))],
            Dialect::Lua,
        )
        .unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("chromium"));

        write_overrides_for(&paths, &[], Dialect::Lua).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), users);
    }

    #[test]
    fn a_command_with_quotes_survives_the_round_trip() {
        let paths = fake_paths("quotes");
        let cmd = r#"sh -c 'notify-send "hi there"'"#;
        write_overrides_for(
            &paths,
            &[Override::Set(exec_bind(mods::SUPER, "N", "Note", cmd))],
            Dialect::Lua,
        )
        .unwrap();
        let back = read_overrides_for(&paths, Dialect::Lua);
        match &back[0] {
            Override::Set(cb) => assert_eq!(cb.arg, cmd),
            other => panic!("expected a bind, got {other:?}"),
        }
    }

    /// `luac -p` (syntax check only). `None` when no Lua toolchain is
    /// installed, so the suite still runs without one.
    fn luac_check(source: &str) -> Option<std::result::Result<(), String>> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for bin in ["luac", "luac5.4", "luac5.3"] {
            let spawned = Command::new(bin)
                .args(["-p", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let Ok(mut child) = spawned else { continue };
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(source.as_bytes())
                .expect("write source");
            let out = child.wait_with_output().expect("luac run");
            return Some(if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).to_string())
            });
        }
        None
    }

    fn assert_valid_lua(source: &str) {
        match luac_check(source) {
            None => eprintln!("skipping luac check: no luac on PATH"),
            Some(Ok(())) => {}
            Some(Err(e)) => panic!("wrote invalid Lua:\n{source}\n\nluac said:\n{e}"),
        }
    }

    /// The SUPER+G bind exactly as it sits in a real Omarchy 4 bindings.lua.
    const OVERVIEW_BIND: &str = "o.bind(\"SUPER + G\", \"Toggle overview\", function()\n\
                                 \thl.plugin.scrolloverview.overview(\"toggle\")\n\
                                 end)";

    /// A bindings.lua whose managed block holds `body`, below the user's own binds.
    fn bindings_with_block(paths: &OmarchyPaths, body: &str) -> std::path::PathBuf {
        let path = user_bindings_path_for(paths, Dialect::Lua);
        let text = format!(
            "local o = require(\"omarchy.bindings\")\n\
             o.bind(\"SUPER + RETURN\", \"Terminal\", \"alacritty\")\n\n{}\n",
            override_block_for(Dialect::Lua).render(body)
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The bug: any keybind write re-rendered a multi-line `function() … end`
    /// action as `function())`, which `luac -p` rejects. Every kind of write
    /// to a *different* bind must leave it — and the file — intact.
    #[test]
    fn a_multi_line_function_bind_survives_add_disable_and_remove() {
        let paths = fake_paths("multiline");
        let path = bindings_with_block(
            &paths,
            &format!(
                "-- Managed by Omarchy Studio — your keybind changes live here.\n\
                 {OVERVIEW_BIND}\n\
                 o.bind(\"SUPER + B\", \"Browser\", \"chromium\")"
            ),
        );
        assert_valid_lua(&std::fs::read_to_string(&path).unwrap());

        let check = |step: &str| {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.contains(OVERVIEW_BIND),
                "{step}: the overview bind was not kept verbatim:\n{text}"
            );
            assert!(!text.contains("function())"), "{step}: mangled:\n{text}");
            assert_valid_lua(&text);
        };

        // add
        let mut overrides = read_overrides_for(&paths, Dialect::Lua);
        assert_eq!(overrides.len(), 2, "both binds read back");
        overrides.push(Override::Set(exec_bind(mods::SUPER, "E", "Editor", "nvim")));
        write_overrides_for(&paths, &overrides, Dialect::Lua).unwrap();
        check("add");

        // disable, through the pipeline's planning path
        let mut overrides = read_overrides_for(&paths, Dialect::Lua);
        overrides.push(Override::Disable {
            modmask: mods::SUPER,
            key: "F".into(),
        });
        let edit = plan_overrides_for(&paths, &overrides, Dialect::Lua)
            .unwrap()
            .expect("a change to write");
        std::fs::write(&path, &edit.new_content).unwrap();
        check("disable");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("hl.unbind(\"SUPER + F\")"));

        // remove a different bind
        let overrides: Vec<Override> = read_overrides_for(&paths, Dialect::Lua)
            .into_iter()
            .filter(|o| !matches!(o, Override::Set(cb) if cb.key == "B"))
            .collect();
        write_overrides_for(&paths, &overrides, Dialect::Lua).unwrap();
        check("remove");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"Browser\""));
        assert!(text.contains("o.bind(\"SUPER + RETURN\", \"Terminal\", \"alacritty\")"));

        // Rewriting what was read changes nothing at all.
        let overrides = read_overrides_for(&paths, Dialect::Lua);
        assert!(plan_overrides_for(&paths, &overrides, Dialect::Lua)
            .unwrap()
            .is_none());
    }

    /// Anything in the block Studio can't reproduce exactly stops the write,
    /// naming the file line, and leaves the file untouched.
    #[test]
    fn refuses_to_rewrite_a_block_it_cannot_read_back() {
        let cases = [
            // An options table Studio's model has no room for.
            "o.bind(\"SUPER + V\", \"Paste\", \"wl-paste\", { repeating = true })",
            // Not a call at all.
            "local browser = \"chromium\"",
            // A computed chord.
            "o.bind(\"SUPER + \" .. key, \"Browser\", \"chromium\")",
            // A call Studio never writes.
            "hl.config({ general = { gaps_in = 2 } })",
            // A function that never closes.
            "o.bind(\"SUPER + G\", \"Overview\", function()",
        ];
        for (n, bad) in cases.iter().enumerate() {
            let paths = fake_paths(&format!("refuse-unreadable-{n}"));
            let path = bindings_with_block(
                &paths,
                &format!("-- Managed by Omarchy Studio\n{OVERVIEW_BIND}\n{bad}"),
            );
            let before = std::fs::read_to_string(&path).unwrap();
            // The bad statement sits on line 9: two user lines, a blank, the
            // open marker, the header comment, then the 3-line overview bind.
            let want = format!("{}:9:", path.display());

            let add = [Override::Set(exec_bind(mods::SUPER, "E", "Editor", "nvim"))];
            let err = match write_overrides_for(&paths, &add, Dialect::Lua) {
                Err(StudioError::External { detail, .. }) => detail,
                other => panic!("case {bad:?}: expected a refusal, got {other:?}"),
            };
            assert!(err.contains(&want), "case {bad:?}: {err}");
            assert!(
                err.contains(bad.lines().next().unwrap()),
                "case {bad:?}: {err}"
            );
            let err = plan_overrides_for(&paths, &add, Dialect::Lua).unwrap_err();
            assert!(err.contains(&want), "case {bad:?}: {err}");
            // Clearing the block would drop the statement too.
            assert!(write_overrides_for(&paths, &[], Dialect::Lua).is_err());

            assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        }
    }

    /// Hyprlang has no such statements and keeps its old behaviour.
    #[test]
    fn the_read_back_guard_is_lua_only() {
        let paths = fake_paths("guard-hyprlang");
        let add = [Override::Set(exec_bind(mods::SUPER, "E", "Editor", "nvim"))];
        write_overrides_for(&paths, &add, Dialect::Hyprlang).unwrap();
        write_overrides_for(&paths, &add, Dialect::Hyprlang).unwrap();
    }
}

#[cfg(test)]
mod lua_dispatcher_tests {
    use super::*;

    fn paths(tag: &str) -> OmarchyPaths {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-kb-dsp-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        }
    }

    fn bind(dispatcher: &str, arg: &str) -> ConfigBind {
        ConfigBind {
            flags: "bindd".into(),
            modmask: mods::SUPER,
            key: "LEFT".into(),
            description: Some("Scroll left".into()),
            dispatcher: dispatcher.into(),
            arg: arg.into(),
        }
    }

    /// The scrolling-layout nav binds Studio installs use `layoutmsg`; Hyprland
    /// 0.56 accepts `hl.dsp.layout("focus l")` for it (verified against the
    /// running compositor).
    #[test]
    fn layoutmsg_maps_to_hl_dsp_layout() {
        assert_eq!(
            bind("layoutmsg", "focus l").render_lua().unwrap(),
            "o.bind(\"SUPER + LEFT\", \"Scroll left\", hl.dsp.layout(\"focus l\"))"
        );
    }

    /// A dispatcher call's argument is a quoted string too, so the reader must
    /// tell `hl.dsp.layout("focus l")` from a plain command of the same text.
    #[test]
    fn a_dispatcher_bind_reads_back_as_itself_not_as_a_command() {
        let p = paths("roundtrip");
        write_overrides_for(
            &p,
            &[
                Override::Set(bind("layoutmsg", "focus l")),
                Override::Set(ConfigBind {
                    key: "B".into(),
                    description: Some("Browser".into()),
                    dispatcher: "exec".into(),
                    arg: "chromium".into(),
                    ..bind("exec", "chromium")
                }),
            ],
            Dialect::Lua,
        )
        .unwrap();

        let back = read_overrides_for(&p, Dialect::Lua);
        assert_eq!(back.len(), 2);
        match (&back[0], &back[1]) {
            (Override::Set(a), Override::Set(b)) => {
                assert_eq!(a.dispatcher, "layoutmsg");
                assert_eq!(a.arg, "focus l");
                assert_eq!(b.dispatcher, "exec");
                assert_eq!(b.arg, "chromium");
            }
            other => panic!("expected two binds, got {other:?}"),
        }
    }

    #[test]
    fn an_unmapped_dispatcher_is_refused_by_name() {
        let err = bind("movefocus", "l").render_lua().unwrap_err();
        assert!(err.contains("`movefocus`"), "names the dispatcher: {err}");
    }

    /// `namespace:method` dispatchers (Hyprland's own plugin-registration
    /// convention, e.g. `scrolloverview:overview`) map to the plugin's
    /// `hl.plugin` table — exactly the form the plugin's own README documents
    /// (`hl.plugin.scrolloverview.overview("toggle all")`).
    #[test]
    fn a_plugin_dispatcher_calls_into_hl_plugin() {
        assert_eq!(
            bind("scrolloverview:overview", "toggle")
                .render_lua()
                .unwrap(),
            "o.bind(\"SUPER + LEFT\", \"Scroll left\", \
             function() hl.plugin.scrolloverview.overview(\"toggle\") end)"
        );
    }

    /// A plugin dispatcher survives the write/read round trip as itself, not
    /// as an opaque raw expression — `current_bind`-style lookups (`niri.rs`)
    /// key off `dispatcher`, so losing this would make Studio think its own
    /// bind was never installed.
    #[test]
    fn a_plugin_dispatcher_bind_reads_back_as_itself() {
        let p = paths("plugin-roundtrip");
        write_overrides_for(
            &p,
            &[Override::Set(bind("scrolloverview:overview", "toggle"))],
            Dialect::Lua,
        )
        .unwrap();

        let back = read_overrides_for(&p, Dialect::Lua);
        match &back[0] {
            Override::Set(cb) => {
                assert_eq!(cb.dispatcher, "scrolloverview:overview");
                assert_eq!(cb.arg, "toggle");
            }
            other => panic!("expected a bind, got {other:?}"),
        }
    }

    #[test]
    fn a_plugin_dispatcher_with_no_arg_renders_and_reads_back() {
        let empty = bind("scrolloverview:overview", "");
        assert_eq!(
            empty.render_lua().unwrap(),
            "o.bind(\"SUPER + LEFT\", \"Scroll left\", \
             function() hl.plugin.scrolloverview.overview() end)"
        );
        let p = paths("plugin-empty-arg");
        write_overrides_for(&p, &[Override::Set(empty)], Dialect::Lua).unwrap();
        let back = read_overrides_for(&p, Dialect::Lua);
        match &back[0] {
            Override::Set(cb) => {
                assert_eq!(cb.dispatcher, "scrolloverview:overview");
                assert_eq!(cb.arg, "");
            }
            other => panic!("expected a bind, got {other:?}"),
        }
    }
}

// ── recovering an action from the Lua sources ────────────────────────────────
// On Omarchy 4 `hyprctl binds` reports every bind's dispatcher as `__lua`, so a
// bind read from the runtime carries no reproducible action (see
// [`LUA_OPAQUE_DISPATCHER`]). To move an action onto a different key, Studio
// finds the `o.bind(...)` that declared it and re-emits its action argument
// verbatim — the expression Omarchy itself wrote, whatever shape it has.

/// A dispatcher marker meaning "`arg` is a raw Lua expression, emit it as-is".
/// Only ever produced by [`resolve_lua_action`]; hyprlang never sees it.
pub const LUA_RAW_DISPATCHER: &str = "__lua_raw";

/// An action recovered from a Lua binding source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedAction {
    pub description: String,
    /// The action argument's Lua source, re-emittable verbatim.
    pub action: String,
}

/// The Lua files that declare bindings, most specific first: the user's own
/// overrides win over Omarchy's defaults, as they do at load time.
fn lua_bind_sources(paths: &OmarchyPaths) -> Vec<std::path::PathBuf> {
    let mut files = vec![user_bindings_path_for(paths, Dialect::Lua)];
    let dir = paths.system.join("default/hypr/bindings");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut found: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lua"))
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

/// Find the action bound to `mask`+`key` in the Lua sources.
///
/// Returns `None` when no static `o.bind` declares that chord — most notably
/// the workspace binds, which Omarchy generates in a `for` loop, so their
/// chords never appear as literals to match.
pub fn resolve_lua_action(paths: &OmarchyPaths, mask: u16, key: &str) -> Option<SourcedAction> {
    let want = chord_id(mask, key);
    for file in lua_bind_sources(paths) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if let Some(found) = scan_lua_binds(&text, want.clone()) {
            return Some(found);
        }
    }
    None
}

/// Scan one Lua source for an `o.bind` declaring `want`.
fn scan_lua_binds(text: &str, want: (u16, String)) -> Option<SourcedAction> {
    // Whole calls, not lines: an action may be a `function() … end` spanning
    // several lines, and cutting it at the first newline would emit `function()`.
    for call in crate::configfs::lua::find_calls(text, "o.bind") {
        let args = call.args;
        // chord, description, action — options are optional and ignored, since
        // they describe how the key repeats, not what it does.
        if args.len() < 3 {
            continue;
        }
        let Some(chord) = crate::configfs::lua::as_string_literal(&args[0]) else {
            continue;
        };
        let (mask, key) = split_lua_chord(&chord);
        if chord_id(mask, &key) != want {
            continue;
        }
        let description = crate::configfs::lua::as_string_literal(&args[1])
            .unwrap_or_else(|| "Omarchy Studio".to_string());
        return Some(SourcedAction {
            description,
            action: args[2].clone(),
        });
    }
    None
}

/// Build the override that moves a resolved action onto a new chord.
pub fn rebind_sourced(action: &SourcedAction, mask: u16, key: &str) -> ConfigBind {
    ConfigBind {
        flags: "bind".into(),
        modmask: mask,
        key: key.to_string(),
        description: Some(action.description.clone()),
        dispatcher: LUA_RAW_DISPATCHER.into(),
        arg: action.action.clone(),
    }
}

/// Recovering an action from the Lua sources, so a rebind is possible again.
#[cfg(test)]
mod lua_resolve_tests {
    use super::*;

    /// Verbatim lines from Omarchy 4's `default/hypr/bindings/tiling.lua`,
    /// covering the action shapes that actually occur.
    const TILING: &str = r#"
o.bind("SUPER + W", "Close window", hl.dsp.window.close())
o.bind("CTRL + ALT + DELETE", "Close all windows", "omarchy-hyprland-window-close-all")
o.bind("SUPER + T", "Toggle window floating/tiling", hl.dsp.window.float({ action = "toggle" }))
o.bind("SUPER + SHIFT + ALT + LEFT", "Move workspace to left monitor", hl.dsp.workspace.move({ monitor = "l" }))
o.bind("XF86AudioMute", "Mute", "omarchy-audio-output-volume mute-toggle", { locked = true })
"#;

    /// A multi-line action is recovered whole, not cut at its first newline.
    #[test]
    fn recovers_a_multi_line_function_action_whole() {
        let src = "local o = require(\"x\")\n\
                   o.bind(\"SUPER + G\", \"Toggle overview\", function()\n\
                   \thl.plugin.scrolloverview.overview(\"toggle\")\n\
                   end)\n";
        let found = scan_lua_binds(src, chord_id(mods::SUPER, "G")).expect("SUPER+G");
        assert_eq!(
            found.action,
            "function()\n\thl.plugin.scrolloverview.overview(\"toggle\")\nend"
        );
        // A chord mentioned only in a comment or a string is not a bind.
        let decoy = "-- o.bind(\"SUPER + Z\", \"x\", \"y\")\nlocal s = 'o.bind(\"SUPER + Z\", \"x\", \"y\")'\n";
        assert!(scan_lua_binds(decoy, chord_id(mods::SUPER, "Z")).is_none());
    }

    #[test]
    fn recovers_a_dispatcher_expression_verbatim() {
        let found = scan_lua_binds(TILING, chord_id(mods::SUPER, "W")).expect("SUPER+W");
        assert_eq!(found.description, "Close window");
        assert_eq!(found.action, "hl.dsp.window.close()");
    }

    /// The action's own table must survive intact — splitting on the comma
    /// inside `{ action = "toggle" }` would truncate it.
    #[test]
    fn recovers_an_expression_containing_a_table() {
        let found = scan_lua_binds(TILING, chord_id(mods::SUPER, "T")).expect("SUPER+T");
        assert_eq!(found.action, "hl.dsp.window.float({ action = \"toggle\" })");
    }

    /// Omarchy writes `SUPER + SHIFT + ALT`, Studio's mask order is
    /// SUPER, CTRL, ALT, SHIFT — so matching has to be on the parsed mask, not
    /// on the chord text.
    #[test]
    fn matches_regardless_of_how_the_modifiers_were_ordered() {
        let want = chord_id(mods::SUPER | mods::ALT | mods::SHIFT, "LEFT");
        let found = scan_lua_binds(TILING, want).expect("the reordered chord");
        assert_eq!(found.action, "hl.dsp.workspace.move({ monitor = \"l\" })");
    }

    #[test]
    fn recovers_a_plain_command_and_ignores_the_options_table() {
        let found = scan_lua_binds(TILING, chord_id(0, "XF86AudioMute")).expect("mute");
        assert_eq!(found.action, "\"omarchy-audio-output-volume mute-toggle\"");
        assert!(scan_lua_binds(TILING, chord_id(mods::SUPER, "NOPE")).is_none());
    }

    #[test]
    fn a_recovered_action_re_emits_verbatim_under_the_new_chord() {
        let found = scan_lua_binds(TILING, chord_id(mods::SUPER, "T")).unwrap();
        let bind = rebind_sourced(&found, mods::SUPER | mods::SHIFT, "G");
        assert_eq!(
            bind.render_lua().unwrap(),
            "o.bind(\"SUPER + SHIFT + G\", \"Toggle window floating/tiling\", \
             hl.dsp.window.float({ action = \"toggle\" }))"
        );
    }

    #[test]
    fn a_recovered_action_survives_the_write_read_round_trip() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let root = std::env::temp_dir().join(format!(
            "omarchy-studio-kb-resolve-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".config/hypr")).unwrap();
        let paths = OmarchyPaths {
            system: root.join("share/omarchy"),
            config: root.join(".config/omarchy"),
            state: root.join(".local/state/omarchy"),
        };
        let found = scan_lua_binds(TILING, chord_id(mods::SUPER, "T")).unwrap();
        let bind = rebind_sourced(&found, mods::SUPER | mods::SHIFT, "G");
        write_overrides_for(&paths, &[Override::Set(bind.clone())], Dialect::Lua).unwrap();

        let back = read_overrides_for(&paths, Dialect::Lua);
        match &back[0] {
            Override::Set(cb) => {
                assert_eq!(cb.dispatcher, LUA_RAW_DISPATCHER);
                assert_eq!(cb.arg, found.action);
                assert_eq!(cb.key, "G");
            }
            other => panic!("expected a bind, got {other:?}"),
        }
    }

    /// Omarchy generates the workspace binds in a `for` loop, so no literal
    /// chord exists to match — the documented gap.
    #[test]
    fn a_loop_generated_chord_is_honestly_unresolvable() {
        let looped = "for workspace = 1, 10 do\n  o.bind(\"SUPER + \" .. key, \"Switch\", hl.dsp.focus({}))\nend\n";
        assert!(scan_lua_binds(looped, chord_id(mods::SUPER, "code:10")).is_none());
    }

    /// Against the real Omarchy install, when there is one.
    #[test]
    fn resolves_against_the_installed_omarchy_if_present() {
        let dir = std::path::Path::new("/usr/share/omarchy/default/hypr/bindings");
        if !dir.is_dir() {
            eprintln!("skipping: no Omarchy install");
            return;
        }
        let paths = OmarchyPaths {
            system: std::path::PathBuf::from("/usr/share/omarchy"),
            config: std::path::PathBuf::from("/nonexistent"),
            state: std::path::PathBuf::from("/nonexistent"),
        };
        // SUPER+W is "Close window" in every Omarchy 4 shipped so far.
        if let Some(found) = resolve_lua_action(&paths, mods::SUPER, "W") {
            assert!(
                found.action.starts_with("hl.dsp.") || found.action.starts_with('"'),
                "unexpected action shape: {}",
                found.action
            );
            assert!(!found.description.is_empty());
        }
    }
}

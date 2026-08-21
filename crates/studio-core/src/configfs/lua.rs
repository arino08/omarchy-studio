//! Lua config emission for Omarchy 4 ("Quattro").
//!
//! Omarchy 4 moved Hyprland's config from hyprlang (`key = value` lines, see
//! [`super::hyprlang`]) to Lua: `~/.config/hypr/hyprland.lua` requires the
//! user's modules *after* Omarchy's defaults, and a later `hl.config({…})` call
//! deep-merges over an earlier one. That ordering is what Studio relies on — a
//! managed block appended to the end of a user file wins, exactly as a managed
//! block of hyprlang lines did, so the write model carries over unchanged.
//!
//! Only emission lives here. Studio never needs to *parse* arbitrary Lua: the
//! effective value of a setting is read back from `hyprctl getoption`, which is
//! authoritative regardless of which dialect produced it.
//!
//! Rendering is deterministic — keys sort, formatting is fixed — so rewriting an
//! unchanged block is a no-op and diffs stay legible.

use std::collections::BTreeMap;

/// A Lua literal.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Emitted quoted and escaped.
    Str(String),
    /// Verbatim Lua source — an already-rendered table or a call expression.
    /// The caller owns its correctness.
    Raw(String),
    /// An array-style table: `{ 1, 2 }`.
    List(Vec<Value>),
}

impl Value {
    /// Build a value of the kind a caller declares, from Studio's string form.
    /// Falls back to a quoted string when the text doesn't parse as the
    /// requested numeric/bool kind, so a malformed value is never emitted as a
    /// bare Lua token that would be a syntax error.
    pub fn int_or_str(raw: &str) -> Value {
        raw.trim()
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Str(raw.to_string()))
    }

    pub fn float_or_str(raw: &str) -> Value {
        raw.trim()
            .parse::<f64>()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::Str(raw.to_string()))
    }

    pub fn bool_or_str(raw: &str) -> Value {
        match raw.trim() {
            "true" | "1" | "yes" | "on" => Value::Bool(true),
            "false" | "0" | "no" | "off" => Value::Bool(false),
            other => Value::Str(other.to_string()),
        }
    }

    /// The Lua source for this value.
    pub fn render(&self) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => render_float(*f),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => quote(s),
            Value::Raw(s) => s.clone(),
            Value::List(items) => {
                let inner: Vec<String> = items.iter().map(|v| v.render()).collect();
                format!("{{ {} }}", inner.join(", "))
            }
        }
    }
}

/// Lua numbers have no int/float distinction, but Hyprland reads some options
/// as floats — keep the decimal point so intent survives a round trip.
fn render_float(f: f64) -> String {
    let s = format!("{f}");
    if s.contains(['.', 'e', 'E', 'n', 'i']) {
        s
    } else {
        format!("{s}.0")
    }
}

/// Quote and escape a Lua string literal.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A bare `key =` is only legal for an identifier; anything else needs
/// bracket-quoting. Hyprland's option names are all identifiers, but a theme or
/// a device name reaching this path must not produce a syntax error.
fn render_key(key: &str) -> String {
    let ident = !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !is_lua_keyword(key);
    if ident {
        key.to_string()
    } else {
        format!("[{}]", quote(key))
    }
}

fn is_lua_keyword(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    )
}

/// A tree of nested tables, built from flat dotted keys.
#[derive(Debug, Default)]
struct Tree {
    leaves: BTreeMap<String, Value>,
    branches: BTreeMap<String, Tree>,
}

impl Tree {
    fn insert(&mut self, path: &str, value: Value) {
        match path.split_once('.') {
            Some((head, rest)) => self
                .branches
                .entry(head.to_string())
                .or_default()
                .insert(rest, value),
            None => {
                self.leaves.insert(path.to_string(), value);
            }
        }
    }

    /// Render the tree's *contents* (no enclosing braces) at `depth`.
    fn render(&self, depth: usize) -> String {
        let pad = "  ".repeat(depth);
        let mut out = String::new();
        for (key, value) in &self.leaves {
            out.push_str(&format!("{pad}{} = {},\n", render_key(key), value.render()));
        }
        for (key, sub) in &self.branches {
            out.push_str(&format!("{pad}{} = {{\n", render_key(key)));
            out.push_str(&sub.render(depth + 1));
            out.push_str(&format!("{pad}}},\n"));
        }
        out
    }
}

/// Render `hl.config({ … })` from flat dotted keys, e.g. `decoration.blur.size`.
/// Returns an empty string for no entries, so a caller writing an empty managed
/// block doesn't emit a stray no-op call.
pub fn config_call(entries: &[(String, Value)]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut tree = Tree::default();
    for (key, value) in entries {
        tree.insert(key, value.clone());
    }
    format!("hl.config({{\n{}}})", tree.render(1))
}

/// Render a single-line call taking one table argument, e.g.
/// `hl.monitor({ output = "DP-2", scale = 1.5 })`. Field order is the caller's,
/// because these calls read positionally to a human (output, mode, position…).
pub fn table_call(func: &str, fields: &[(&str, Value)]) -> String {
    let inner: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{} = {}", render_key(k), v.render()))
        .collect();
    format!("{func}({{ {} }})", inner.join(", "))
}

/// Render a call taking a name and a table, e.g.
/// `hl.curve("easeOutQuint", { type = "bezier", points = { { 0.23, 1 }, … } })`.
pub fn named_table_call(func: &str, name: &str, fields: &[(&str, Value)]) -> String {
    let inner: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{} = {}", render_key(k), v.render()))
        .collect();
    format!("{func}({}, {{ {} }})", quote(name), inner.join(", "))
}

/// Read a `hl.config({…})` block back into flat dotted keys.
///
/// This is the exact inverse of [`config_call`] and understands *only* the
/// shape Studio emits — nested `ident = value,` lines. That is deliberate:
/// Studio parses its own managed block to know which settings it owns, and
/// nothing else. Anything unrecognised is skipped rather than guessed at, so a
/// hand-edited block degrades to "not overridden" instead of a wrong value.
///
/// Values come back as Studio's string form (unquoted for strings, literal text
/// for numbers and bools), matching what the hyprlang reader produces.
pub fn parse_config_call(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut path: Vec<String> = Vec::new();
    for raw in body.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        // Leaving a table: `},` or `}})` etc.
        if line.starts_with('}') {
            path.pop();
            continue;
        }
        // `hl.config({` opens the outermost table but contributes no key.
        if line.starts_with("hl.config(") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(key) = accept_key(key.trim()) else {
            continue;
        };
        let value = value.trim().trim_end_matches(',').trim();
        if value == "{" {
            path.push(key);
        } else if !key.is_empty() && !value.is_empty() {
            let mut full = path.clone();
            full.push(key);
            out.push((full.join("."), unrender_value(value)));
        }
    }
    out
}

/// Drop a trailing `-- comment`, ignoring one inside a string literal.
fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'-' if !in_str && i + 1 < b.len() && b[i + 1] == b'-' => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// Inverse of [`render_key`], rejecting anything that isn't a key *we* would
/// have emitted. Without this a single-line call like `hl.animation({ leaf = 1 })`
/// would split on its `=` and yield a nonsense key.
fn accept_key(key: &str) -> Option<String> {
    if let Some(inner) = key.strip_prefix("[\"").and_then(|k| k.strip_suffix("\"]")) {
        return Some(unrender_value(&format!("\"{inner}\"")));
    }
    let ident = !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ident.then(|| key.to_string())
}

/// Inverse of [`Value::render`] for the scalar forms: unquote and unescape a
/// string, leave numbers and bools as their literal text.
fn unrender_value(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Split a Lua argument list on its *top-level* commas, returning each argument
/// with surrounding whitespace trimmed.
///
/// `text` starts just inside the opening paren. Nested calls, tables, strings
/// and comments are stepped over, so
/// `o.bind("SUPER + T", "Float", hl.dsp.window.float({ action = "toggle" }))`
/// yields three arguments rather than splitting inside the dispatcher's table.
/// Stops at the paren that closes the list, so trailing text is ignored.
pub fn split_args(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let (mut out, mut start, mut i, mut depth) = (Vec::new(), 0usize, 0usize, 0i32);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let quote = b[i];
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        c if c == quote => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'-' if i + 1 < b.len() && b[i + 1] == b'-' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' | b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b')' | b'}' | b']' => {
                if b[i] == b')' && depth == 0 {
                    break; // the paren closing our own argument list
                }
                depth -= 1;
                i += 1;
            }
            b',' if depth == 0 => {
                out.push(text[start..i].trim().to_string());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    let tail = text[start..i.min(text.len())].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// The unescaped contents of `text` if it is a single quoted Lua string, else
/// `None` — the test for "is this argument a plain command string or a
/// dispatcher expression?".
pub fn as_string_literal(text: &str) -> Option<String> {
    let t = text.trim();
    let inner = t.strip_prefix('"').and_then(|v| v.strip_suffix('"'))?;
    // A closing quote in the middle means this is not one single literal.
    if inner.contains('"') && !inner.contains("\\\"") {
        return None;
    }
    Some(unrender_value(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, Value)]) -> Vec<(String, Value)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn nests_dotted_keys_into_tables() {
        let out = config_call(&entries(&[
            ("general.gaps_in", Value::Int(6)),
            ("general.border_size", Value::Int(1)),
            ("decoration.blur.enabled", Value::Bool(true)),
            ("decoration.blur.size", Value::Int(8)),
            ("decoration.rounding", Value::Int(4)),
        ]));
        assert_eq!(
            out,
            "hl.config({\n  \
             decoration = {\n    \
             rounding = 4,\n    \
             blur = {\n      \
             enabled = true,\n      \
             size = 8,\n    \
             },\n  \
             },\n  \
             general = {\n    \
             border_size = 1,\n    \
             gaps_in = 6,\n  \
             },\n\
             })"
        );
    }

    #[test]
    fn is_deterministic_so_rewrites_are_no_ops() {
        let pairs = entries(&[
            ("general.gaps_out", Value::Int(3)),
            ("general.gaps_in", Value::Int(2)),
        ]);
        let mut reversed = pairs.clone();
        reversed.reverse();
        assert_eq!(config_call(&pairs), config_call(&reversed));
    }

    #[test]
    fn renders_each_literal_kind() {
        assert_eq!(Value::Int(5).render(), "5");
        assert_eq!(Value::Bool(false).render(), "false");
        assert_eq!(Value::Str("dwindle".into()).render(), "\"dwindle\"");
        assert_eq!(
            Value::Raw("hl.dsp.killactive()".into()).render(),
            "hl.dsp.killactive()"
        );
        assert_eq!(
            Value::List(vec![Value::Int(1), Value::Int(1)]).render(),
            "{ 1, 1 }"
        );
    }

    #[test]
    fn floats_keep_their_decimal_point() {
        assert_eq!(Value::Float(0.15).render(), "0.15");
        assert_eq!(Value::Float(1.0).render(), "1.0");
        assert_eq!(Value::Float(3.79).render(), "3.79");
    }

    #[test]
    fn escapes_strings_and_non_identifier_keys() {
        assert_eq!(Value::Str("a\"b\\c".into()).render(), "\"a\\\"b\\\\c\"");
        // A device name with a dash can't be a bare Lua key.
        let out = config_call(&entries(&[("device", Value::Str("x".into()))]));
        assert!(out.contains("device = \"x\""));
        assert_eq!(render_key("kb-layout"), "[\"kb-layout\"]");
        assert_eq!(render_key("end"), "[\"end\"]");
        assert_eq!(render_key("2fast"), "[\"2fast\"]");
    }

    #[test]
    fn empty_entries_emit_nothing() {
        assert_eq!(config_call(&[]), "");
    }

    #[test]
    fn builds_call_forms_for_monitors_and_curves() {
        assert_eq!(
            table_call(
                "hl.monitor",
                &[
                    ("output", Value::Str("DP-2".into())),
                    ("mode", Value::Str("2560x1440@144".into())),
                    ("position", Value::Str("0x0".into())),
                    ("scale", Value::Float(1.5)),
                ]
            ),
            "hl.monitor({ output = \"DP-2\", mode = \"2560x1440@144\", position = \"0x0\", scale = 1.5 })"
        );
        assert_eq!(
            named_table_call(
                "hl.curve",
                "easeOutQuint",
                &[
                    ("type", Value::Str("bezier".into())),
                    (
                        "points",
                        Value::List(vec![
                            Value::List(vec![Value::Float(0.23), Value::Int(1)]),
                            Value::List(vec![Value::Float(0.32), Value::Int(1)]),
                        ])
                    ),
                ]
            ),
            "hl.curve(\"easeOutQuint\", { type = \"bezier\", points = { { 0.23, 1 }, { 0.32, 1 } } })"
        );
    }

    #[test]
    fn reads_back_exactly_what_it_emitted() {
        let pairs = entries(&[
            ("general.gaps_in", Value::Int(6)),
            ("general.layout", Value::Str("dwindle".into())),
            ("general.resize_on_border", Value::Bool(true)),
            ("decoration.rounding", Value::Int(8)),
            ("decoration.blur.enabled", Value::Bool(false)),
            ("decoration.dim_strength", Value::Float(0.15)),
            ("input.kb-layout", Value::Str("us,dk".into())),
        ]);
        let emitted = config_call(&pairs);
        let mut read = parse_config_call(&emitted);
        read.sort();

        let mut want: Vec<(String, String)> = vec![
            ("general.gaps_in".into(), "6".into()),
            ("general.layout".into(), "dwindle".into()),
            ("general.resize_on_border".into(), "true".into()),
            ("decoration.rounding".into(), "8".into()),
            ("decoration.blur.enabled".into(), "false".into()),
            ("decoration.dim_strength".into(), "0.15".into()),
            ("input.kb-layout".into(), "us,dk".into()),
        ];
        want.sort();
        assert_eq!(read, want);
    }

    #[test]
    fn round_trip_survives_escapes_and_nesting_depth() {
        let pairs = entries(&[
            ("a.b.c.d", Value::Str("deep".into())),
            ("misc.note", Value::Str("a\"b\\c".into())),
        ]);
        let read = parse_config_call(&config_call(&pairs));
        assert!(read.contains(&("a.b.c.d".to_string(), "deep".to_string())));
        assert!(read.contains(&("misc.note".to_string(), "a\"b\\c".to_string())));
    }

    #[test]
    fn reader_ignores_comments_and_junk_rather_than_guessing() {
        let body = "-- Managed by Omarchy Studio\nhl.config({\n  general = {\n    -- inner note\n    gaps_in = 4,\n  },\n})";
        assert_eq!(
            parse_config_call(body),
            vec![("general.gaps_in".to_string(), "4".to_string())]
        );
        // A call shape we never emit yields nothing rather than a junk key.
        assert!(parse_config_call("hl.animation({ leaf = \"x\", speed = 3 })").is_empty());
        assert!(parse_config_call("hl.monitor({ output = \"DP-2\" })").is_empty());
    }

    #[test]
    fn a_trailing_comment_does_not_swallow_a_value() {
        let body =
            "hl.config({\n  general = {\n    layout = \"dwindle\", -- not scrolling\n  },\n})";
        assert_eq!(
            parse_config_call(body),
            vec![("general.layout".to_string(), "dwindle".to_string())]
        );
    }

    #[test]
    fn splits_arguments_without_breaking_into_nested_calls() {
        assert_eq!(
            split_args(r#""SUPER + T", "Float", hl.dsp.window.float({ action = "toggle" }))"#),
            vec![
                "\"SUPER + T\"",
                "\"Float\"",
                "hl.dsp.window.float({ action = \"toggle\" })"
            ]
        );
        // A trailing options table is its own argument.
        assert_eq!(
            split_args(r#""XF86AudioMute", "Mute", "cmd", { locked = true })"#).len(),
            4
        );
        // A comma inside a string is not a separator.
        assert_eq!(
            split_args(r#""SUPER + K", "Layout", "us,dk")"#),
            vec!["\"SUPER + K\"", "\"Layout\"", "\"us,dk\""]
        );
    }

    #[test]
    fn tells_a_plain_command_from_a_dispatcher_expression() {
        assert_eq!(as_string_literal("\"chromium\""), Some("chromium".into()));
        assert_eq!(as_string_literal("hl.dsp.window.close()"), None);
        assert_eq!(as_string_literal("{ launch = \"nautilus\" }"), None);
    }

    #[test]
    fn coerces_studio_string_values_by_declared_kind() {
        assert_eq!(Value::int_or_str("6"), Value::Int(6));
        assert_eq!(Value::bool_or_str("true"), Value::Bool(true));
        assert_eq!(Value::float_or_str("0.15"), Value::Float(0.15));
        // Unparseable text degrades to a quoted string, never a bare token.
        assert_eq!(Value::int_or_str("auto"), Value::Str("auto".into()));
        assert_eq!(Value::bool_or_str("maybe"), Value::Str("maybe".into()));
    }
}

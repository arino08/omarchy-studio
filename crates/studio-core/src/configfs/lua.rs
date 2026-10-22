//! Lua config emission for Omarchy 4 ("Quattro").
//!
//! Omarchy 4 moved Hyprland's config from hyprlang (`key = value` lines, see
//! [`super::hyprlang`]) to Lua: `~/.config/hypr/hyprland.lua` requires the
//! user's modules *after* Omarchy's defaults, and a later `hl.config({…})` call
//! deep-merges over an earlier one. That ordering is what Studio relies on — a
//! managed block appended to the end of a user file wins, exactly as a managed
//! block of hyprlang lines did, so the write model carries over unchanged.
//!
//! Mostly emission lives here. Studio never needs to *evaluate* Lua: the
//! effective value of a setting is read back from `hyprctl getoption`, which is
//! authoritative regardless of which dialect produced it. What it does read is
//! lexical — its own managed blocks ([`parse_config_call`],
//! [`parse_call_statements`]) and `o.bind(…)` calls in binding sources
//! ([`find_calls`]) — with strings, comments and multi-line arguments stepped
//! over, and [`same_tokens`] to prove a re-rendered statement lost nothing.
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

/// A table member as it's *accessed*, not assigned: `.name` for a bare
/// identifier, `["name"]` otherwise. Used for calls into a namespace table
/// (`hl.plugin.scrolloverview.overview(...)`) rather than a `key = value`
/// table literal, which is what [`render_key`] is for.
pub fn render_member(name: &str) -> String {
    let key = render_key(name);
    if key.starts_with('[') {
        key
    } else {
        format!(".{key}")
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

/// Where the long bracket opening at `i` (`[[`, `[=[`, `[==[`…) ends, plus its
/// level — `None` when `i` isn't one.
fn long_bracket_open(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'=') {
        j += 1;
    }
    (b.get(j) == Some(&b'[')).then_some((j + 1, j - i - 1))
}

/// The index just past the long bracket closing at `level`, searching from
/// `from`; the end of input when it's unterminated.
fn long_bracket_close(b: &[u8], from: usize, level: usize) -> usize {
    for i in from..b.len() {
        if b[i] == b']' {
            let eq = b[i + 1..].iter().take_while(|&&c| c == b'=').count();
            if eq == level && b.get(i + 1 + level) == Some(&b']') {
                return i + 2 + level;
            }
        }
    }
    b.len()
}

/// If a string literal or comment starts at `i`, the index just past it
/// (the end of input when it's unterminated). These are the lexemes inside
/// which brackets, commas and quotes mean nothing.
fn skip_lexeme(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'"' | b'\'' => {
            let quote = b[i];
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    c if c == quote => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(b.len())
        }
        b'[' => long_bracket_open(b, i).map(|(body, level)| long_bracket_close(b, body, level)),
        b'-' if b.get(i + 1) == Some(&b'-') => {
            if let Some((body, level)) = long_bracket_open(b, i + 2) {
                return Some(long_bracket_close(b, body, level));
            }
            let mut j = i + 2;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        _ => None,
    }
}

/// The index of the `)` matching the `(` at `open`, stepping over nested
/// brackets, strings and comments — newlines included, so a multi-line
/// `function() … end` argument stays inside its call. `None` if unbalanced.
fn matching_paren(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        if let Some(next) = skip_lexeme(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return (b[i] == b')').then_some(i);
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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
        if let Some(next) = skip_lexeme(b, i) {
            i = next;
            continue;
        }
        match b[i] {
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

/// A function-call statement found in Lua source: `callee(args…)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The dotted name called, e.g. `o.bind`.
    pub callee: String,
    /// Each top-level argument's source, trimmed.
    pub args: Vec<String>,
    /// The whole call's source, from the callee to the closing paren.
    pub text: String,
    /// 1-based line the call starts on.
    pub line: usize,
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

/// Read a dotted name (`hl.plugin.x`) at `i`, returning where it ends.
fn dotted_name_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    loop {
        if !b.get(j).copied().is_some_and(is_name_start) {
            return None;
        }
        while b.get(j).copied().is_some_and(is_name_char) {
            j += 1;
        }
        if b.get(j) == Some(&b'.') {
            j += 1;
        } else {
            return Some(j);
        }
    }
}

/// The `(` after the callee name ending at `name_end`, if one follows it.
fn open_paren(b: &[u8], name_end: usize) -> Option<usize> {
    let mut open = name_end;
    while b.get(open).is_some_and(|c| c.is_ascii_whitespace()) {
        open += 1;
    }
    (b.get(open) == Some(&b'(')).then_some(open)
}

/// The call whose callee name spans `start..name_end`, if a `(` follows it.
fn call_at(text: &str, start: usize, name_end: usize) -> Option<(Call, usize)> {
    let b = text.as_bytes();
    let open = open_paren(b, name_end)?;
    let close = matching_paren(b, open)?;
    Some((
        Call {
            callee: text[start..name_end].to_string(),
            args: split_args(&text[open + 1..]),
            text: text[start..=close].to_string(),
            line: line_of(text, start),
        },
        close + 1,
    ))
}

/// Read `text` as a chunk made *only* of call statements, such as Studio's
/// managed keybinds block. Comments, whitespace and `;` between statements
/// are skipped; a statement may span any number of lines.
///
/// The second list names, as `(line, source)`, each statement that isn't a
/// plain call — an assignment, a `for` loop, an unbalanced paren — so a caller
/// can refuse rather than silently drop it. Reading resumes on the line after
/// one, except after a call whose parens never close: nothing past that can
/// be told apart.
pub fn parse_call_statements(text: &str) -> (Vec<Call>, Vec<(usize, String)>) {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() || b[i] == b';' {
            i += 1;
            continue;
        }
        if b[i] == b'-' && b.get(i + 1) == Some(&b'-') {
            i = skip_lexeme(b, i).unwrap_or(b.len());
            continue;
        }
        let name_end = dotted_name_end(b, i);
        let Some((call, next)) = name_end.and_then(|end| call_at(text, i, end)) else {
            let line = line_of(text, i);
            let source = text.lines().nth(line - 1).unwrap_or_default().trim();
            errors.push((line, source.to_string()));
            if name_end.and_then(|end| open_paren(b, end)).is_some() {
                break;
            }
            i = text[i..].find('\n').map_or(b.len(), |n| i + n + 1);
            continue;
        };
        out.push(call);
        i = next;
    }
    (out, errors)
}

/// Every call to `callee` anywhere in arbitrary Lua source — inside loops,
/// functions, whatever — skipping strings and comments. A call whose parens
/// never balance is skipped rather than truncated.
pub fn find_calls(text: &str, callee: &str) -> Vec<Call> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = skip_lexeme(b, i) {
            i = next;
            continue;
        }
        let boundary = i == 0 || !(is_name_char(b[i - 1]) || matches!(b[i - 1], b'.' | b':'));
        if boundary && is_name_start(b[i]) {
            if let Some(end) = dotted_name_end(b, i) {
                if &text[i..end] == callee {
                    if let Some((call, next)) = call_at(text, i, end) {
                        out.push(call);
                        i = next;
                        continue;
                    }
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A Lua token, as far as telling two pieces of source apart needs: strings
/// compare by *value*, so `'x'` and `"x"` are the same token, and comments
/// and whitespace vanish.
#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Str(Vec<u8>),
    Punct(u8),
}

fn tokens(text: &str) -> Vec<Token> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if let Some(next) = skip_lexeme(b, i) {
            if c != b'-' {
                out.push(Token::Str(decode_string(&b[i..next])));
            }
            i = next;
        } else if is_name_char(c) {
            let start = i;
            while i < b.len() && is_name_char(b[i]) {
                i += 1;
            }
            out.push(Token::Word(text[start..i].to_string()));
        } else {
            out.push(Token::Punct(c));
            i += 1;
        }
    }
    out
}

/// The bytes a Lua string literal (quoted or long-bracket) denotes.
fn decode_string(lit: &[u8]) -> Vec<u8> {
    if let Some((body, level)) = long_bracket_open(lit, 0) {
        let end = lit.len().saturating_sub(level + 2).max(body);
        let mut inner = &lit[body..end];
        // A newline straight after the opening bracket isn't part of the string.
        if let Some(rest) = inner
            .strip_prefix(b"\r\n")
            .or_else(|| inner.strip_prefix(b"\n"))
        {
            inner = rest;
        }
        return inner.to_vec();
    }
    let inner = &lit[1..lit.len().saturating_sub(1).max(1)];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        if inner[i] != b'\\' || i + 1 >= inner.len() {
            out.push(inner[i]);
            i += 1;
            continue;
        }
        i += 1;
        match inner[i] {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'v' => out.push(0x0b),
            b'x' => {
                let hex = std::str::from_utf8(inner.get(i + 1..i + 3).unwrap_or_default())
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(v) = hex {
                    out.push(v);
                    i += 2;
                }
            }
            b'z' => {
                while i + 1 < inner.len() && inner[i + 1].is_ascii_whitespace() {
                    i += 1;
                }
            }
            b'u' if inner.get(i + 1) == Some(&b'{') => {
                let close = inner[i..].iter().position(|&c| c == b'}').map(|p| p + i);
                let ch = close
                    .and_then(|c| std::str::from_utf8(&inner[i + 2..c]).ok())
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .and_then(char::from_u32);
                if let (Some(ch), Some(c)) = (ch, close) {
                    out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                    i = c;
                }
            }
            d if d.is_ascii_digit() => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 3 && i < inner.len() && inner[i].is_ascii_digit() {
                    v = v * 10 + u32::from(inner[i] - b'0');
                    i += 1;
                    n += 1;
                }
                out.push(v as u8);
                continue;
            }
            other => out.push(other), // \\ \" \' and an escaped newline
        }
        i += 1;
    }
    out
}

/// Do two pieces of Lua source say the same thing — the same tokens, with
/// whitespace, comments and string-quoting style ignored? This is the test
/// that a value Studio parsed and re-rendered lost nothing on the way.
pub fn same_tokens(a: &str, b: &str) -> bool {
    tokens(a) == tokens(b)
}

/// The contents of `text` if it is a single Lua string literal, else `None` —
/// the test for "is this argument a plain command string or a dispatcher
/// expression?". Concatenations (`"a" .. "b"`) and non-UTF-8 escapes are not
/// single literals.
pub fn as_string_literal(text: &str) -> Option<String> {
    match tokens(text).as_slice() {
        [Token::Str(bytes)] => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    }
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
    fn render_member_dots_an_identifier_and_brackets_the_rest() {
        assert_eq!(render_member("scrolloverview"), ".scrolloverview");
        assert_eq!(render_member("kb-layout"), "[\"kb-layout\"]");
        assert_eq!(render_member("end"), "[\"end\"]");
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
    fn reads_multi_line_call_statements_whole() {
        let body = "-- header\n\
                    o.bind(\"SUPER + G\", \"Overview\", function()\n\
                    \thl.plugin.scrolloverview.overview(\"toggle\") -- )\n\
                    end);\n\
                    hl.unbind(\"SUPER + F\")";
        let (calls, errors) = parse_call_statements(body);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].callee, "o.bind");
        assert_eq!(calls[0].line, 2);
        assert_eq!(
            calls[0].args[2],
            "function()\n\thl.plugin.scrolloverview.overview(\"toggle\") -- )\nend"
        );
        assert_eq!(calls[1].callee, "hl.unbind");
        assert_eq!(calls[1].line, 5);
    }

    #[test]
    fn a_statement_that_is_not_a_call_is_named_by_line() {
        // Every one is named, and the calls around them are still read.
        let (calls, errors) = parse_call_statements(
            "hl.unbind(\"A\")\nlocal x = 1\nhl.unbind(\"B\")\nlocal y = 2\nhl.unbind(\"C\")",
        );
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].line, 5);
        assert_eq!(
            errors,
            [
                (2, "local x = 1".to_string()),
                (4, "local y = 2".to_string())
            ]
        );
        // An unbalanced call is reported where it starts, not truncated, and
        // nothing after it is read.
        assert_eq!(
            parse_call_statements("\n\no.bind(\"A\", \"b\", function()\n  x()\n"),
            (
                Vec::new(),
                vec![(3, "o.bind(\"A\", \"b\", function()".to_string())]
            )
        );
        // A call that's then indexed or called again isn't a plain statement.
        assert_eq!(
            parse_call_statements("o.bind(\"A\", \"b\", \"c\").x = 1")
                .1
                .len(),
            1
        );
    }

    #[test]
    fn long_strings_and_comments_hide_their_brackets() {
        let (calls, _) = parse_call_statements("--[[ ) ]] f([[ ) , ]], --[==[ ( ]==] 2)");
        assert_eq!(calls[0].args, vec!["[[ ) , ]]", "--[==[ ( ]==] 2"]);
    }

    #[test]
    fn finds_calls_in_arbitrary_source_but_not_in_strings_or_comments() {
        let src = "for i = 1, 9 do\n  o.bind(\"SUPER + \" .. i, \"ws\", f(i))\nend\n\
                   -- o.bind(\"no\")\nlocal s = \"o.bind('no')\"\nfoo.o.bind(\"no\")\n\
                   o.bind(\"SUPER + G\", \"x\", function()\n  y()\nend)";
        let calls = find_calls(src, "o.bind");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].line, 2);
        assert_eq!(calls[1].line, 7);
        assert_eq!(calls[1].args[2], "function()\n  y()\nend");
    }

    #[test]
    fn token_comparison_ignores_layout_and_quote_style_only() {
        assert!(same_tokens(
            "o.bind(\"A\", 'b', function()\n\tx()\nend)",
            "o.bind( \"A\",\"b\", function() x() end ) -- note"
        ));
        assert!(same_tokens("f(\"\\65\\x42\\u{43}\")", "f(\"ABC\")"));
        assert!(same_tokens("f([[\nline]])", "f(\"line\")"));
        assert!(!same_tokens(
            "o.bind(\"A\", \"b\", function())",
            "o.bind(\"A\", \"b\", function() end)"
        ));
        assert!(!same_tokens("f(\"a\", { x = 1 })", "f(\"a\")"));
    }

    #[test]
    fn a_string_literal_is_exactly_one_string() {
        assert_eq!(as_string_literal("'single'"), Some("single".into()));
        assert_eq!(as_string_literal("\"a\\\"b\""), Some("a\"b".into()));
        assert_eq!(as_string_literal("\"a\\\"b\" .. \"c\""), None);
        assert_eq!(as_string_literal("\"a\" .. \"b\""), None);
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

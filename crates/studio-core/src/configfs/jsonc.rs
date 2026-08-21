//! JSONC editor for Waybar config (spec 03 §3, roadmap 0.4.1 spike).
//!
//! Waybar's `config.jsonc` is JSON-with-comments. We need to reorder the
//! `modules-left/center/right` arrays and tweak scalar values *without*
//! disturbing the user's comments, spacing, or trailing commas. Rather than a
//! full DOM rewrite (which would reformat everything), we parse a lightweight
//! tree of byte spans over the original source and edit by splicing only the
//! spans we touch — so every untouched byte, including every comment, survives.
//!
//! Go/no-go for the spike: (1) reorder a top-level array preserving comments,
//! (2) set a scalar value in place. Both are covered here and tested against
//! the real Omarchy waybar config.

/// A parsed value, carrying the byte span of its own text (not surrounding
/// whitespace/commas). Objects and arrays also keep their members/elements.
#[derive(Debug, Clone)]
pub enum Node {
    Object {
        span: (usize, usize),
        members: Vec<Member>,
    },
    Array {
        span: (usize, usize),
        /// Each element's value span, in source order.
        elems: Vec<(usize, usize)>,
    },
    /// A string/number/bool/null literal — its raw text is `src[span]`.
    Scalar { span: (usize, usize) },
}

#[derive(Debug, Clone)]
pub struct Member {
    pub key: String,
    pub value: Node,
}

impl Node {
    fn span(&self) -> (usize, usize) {
        match self {
            Node::Object { span, .. } | Node::Array { span, .. } | Node::Scalar { span } => *span,
        }
    }
}

/// A JSONC document: the original source plus a span tree over it.
pub struct JsoncDoc {
    src: String,
    root: Node,
}

impl JsoncDoc {
    pub fn parse(src: &str) -> Result<Self, String> {
        let bytes = src.as_bytes();
        let mut p = Parser { b: bytes, i: 0 };
        p.skip_trivia();
        let root = p.parse_value()?;
        Ok(Self {
            src: src.to_string(),
            root,
        })
    }

    /// The current (possibly edited) source.
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.src.clone()
    }

    fn slice(&self, span: (usize, usize)) -> &str {
        &self.src[span.0..span.1]
    }

    /// Resolve a dotted path (`"a.b"`) to a node. Only object keys are followed.
    fn resolve(&self, path: &str) -> Option<&Node> {
        let mut node = &self.root;
        for seg in path.split('.') {
            let Node::Object { members, .. } = node else {
                return None;
            };
            node = &members.iter().find(|m| m.key == seg)?.value;
        }
        Some(node)
    }

    /// The raw text of a scalar or the whole value at `path`.
    pub fn get(&self, path: &str) -> Option<String> {
        self.resolve(path).map(|n| self.slice(n.span()).to_string())
    }

    /// A top-level array's elements as raw strings (e.g. Waybar module names,
    /// still quoted). Returns None if the path isn't an array.
    pub fn array_items(&self, path: &str) -> Option<Vec<String>> {
        match self.resolve(path)? {
            Node::Array { elems, .. } => Some(
                elems
                    .iter()
                    .map(|s| self.slice(*s).trim().to_string())
                    .collect(),
            ),
            _ => None,
        }
    }

    /// Replace a scalar value in place, preserving all surrounding bytes.
    /// `new_raw` is the literal text to insert (include quotes for strings).
    pub fn set_scalar(&mut self, path: &str, new_raw: &str) -> Result<(), String> {
        let span = match self.resolve(path) {
            Some(Node::Scalar { span }) => *span,
            Some(_) => return Err(format!("{path} is not a scalar")),
            None => return Err(format!("{path} not found")),
        };
        self.splice(span, new_raw);
        Ok(())
    }

    /// Reorder a top-level array to `new_order` — a permutation of the array's
    /// module names (matched by their leading quoted string). Each element's
    /// full text chunk (its own whitespace and comments) is carried verbatim,
    /// so nothing but the order changes.
    pub fn reorder(&mut self, path: &str, new_order: &[String]) -> Result<(), String> {
        let span = match self.resolve(path) {
            Some(Node::Array { span, .. }) => *span,
            Some(_) => return Err(format!("{path} is not an array")),
            None => return Err(format!("{path} not found")),
        };
        let inner_span = (span.0 + 1, span.1 - 1);
        let inner = self.src[inner_span.0..inner_span.1].to_string();
        let chunks = split_top_level(&inner);
        if chunks.is_empty() {
            return Ok(());
        }
        // group chunks by their key (leading quoted string), preserving order
        let mut by_key: std::collections::HashMap<String, std::collections::VecDeque<String>> =
            Default::default();
        for c in &chunks {
            by_key.entry(chunk_key(c)).or_default().push_back(c.clone());
        }
        let mut out = String::new();
        for (i, want) in new_order.iter().enumerate() {
            let q = by_key
                .get_mut(want.trim())
                .filter(|q| !q.is_empty())
                .ok_or_else(|| format!("`{want}` is not in {path}"))?;
            let chunk = q.pop_front().unwrap();
            if i > 0 {
                out.push(',');
            }
            out.push_str(&chunk);
        }
        self.splice(inner_span, &out);
        Ok(())
    }

    /// Remove the first array element whose module name matches `id` (a quoted
    /// string like `"clock"`). No-op if absent. Preserves other elements' text.
    pub fn remove_item(&mut self, path: &str, id: &str) -> Result<(), String> {
        let span = match self.resolve(path) {
            Some(Node::Array { span, .. }) => *span,
            Some(_) => return Err(format!("{path} is not an array")),
            None => return Err(format!("{path} not found")),
        };
        let inner_span = (span.0 + 1, span.1 - 1);
        let inner = self.src[inner_span.0..inner_span.1].to_string();
        let mut chunks = split_top_level(&inner);
        let want = id.trim();
        let Some(pos) = chunks.iter().position(|c| chunk_key(c) == want) else {
            return Ok(());
        };
        chunks.remove(pos);
        let out = chunks.join(",");
        self.splice(inner_span, &out);
        Ok(())
    }

    /// Append a module `id` (quoted, e.g. `"cpu"`) to the end of a lane array,
    /// matching the existing indentation. No-op if it's already present.
    pub fn add_item(&mut self, path: &str, id: &str) -> Result<(), String> {
        let span = match self.resolve(path) {
            Some(Node::Array { span, .. }) => *span,
            Some(_) => return Err(format!("{path} is not an array")),
            None => return Err(format!("{path} not found")),
        };
        let inner_span = (span.0 + 1, span.1 - 1);
        let inner = self.src[inner_span.0..inner_span.1].to_string();
        let mut chunks = split_top_level(&inner);
        let want = id.trim();
        if chunks.iter().any(|c| chunk_key(c) == want) {
            return Ok(());
        }
        // Reuse the leading whitespace of the last chunk (its indent) for the
        // new element; fall back to a single space for an inline array.
        let indent = chunks
            .last()
            .map(|c| {
                let ws: String = c.chars().take_while(|ch| ch.is_whitespace()).collect();
                if ws.is_empty() {
                    " ".to_string()
                } else {
                    ws
                }
            })
            .unwrap_or_else(|| " ".to_string());
        chunks.push(format!("{indent}{want}"));
        let out = chunks.join(",");
        self.splice(inner_span, &out);
        Ok(())
    }

    /// Is `key` a member of the object at `path` (empty path = the root object)?
    /// Matches the key literally, so a dotted key like `style.studio` — one
    /// member, not a nested path — is found where `get` would walk into `style`.
    pub fn has_member(&self, path: &str, key: &str) -> bool {
        let node = if path.is_empty() {
            Some(&self.root)
        } else {
            self.resolve(path)
        };
        matches!(node, Some(Node::Object { members, .. })
            if members.iter().any(|m| m.key == key))
    }

    /// Remove the member `key` from the object at `path` (empty path = the root
    /// object). No-op when the key is absent. Cuts the key and value spans only,
    /// so surrounding comments — including a help block that leads the object —
    /// survive, as does every other member.
    pub fn remove_member(&mut self, path: &str, key: &str) -> Result<(), String> {
        let vspan = {
            let node = if path.is_empty() {
                &self.root
            } else {
                self.resolve(path)
                    .ok_or_else(|| format!("{path} not found"))?
            };
            let Node::Object { members, .. } = node else {
                return Err(format!("{path} is not an object"));
            };
            match members.iter().find(|m| m.key == key) {
                Some(m) => m.value.span(),
                None => return Ok(()),
            }
        };

        // Cut from the key's own quote to the end of its value. Working from
        // spans rather than comma-split chunks is what keeps a preceding comment
        // block — the help text in Omarchy's stub file — out of the removal.
        let quoted = format!("\"{key}\"");
        let Some(start) = self.src[..vspan.0].rfind(&quoted) else {
            return Ok(());
        };
        let mut start = start;
        let mut end = vspan.1;

        // Take the separating comma with it: the one that follows, or — when
        // this was the last member — the one that precedes.
        let after = self.src[end..]
            .find(|c: char| !c.is_whitespace())
            .map(|off| end + off);
        match after {
            Some(i) if self.src.as_bytes()[i] == b',' => end = i + 1,
            _ => {
                let before = self.src[..start].trim_end_matches([' ', '\t', '\r', '\n']);
                if before.ends_with(',') {
                    start = before.len() - 1;
                }
            }
        }

        // Drop the blank line the member leaves behind, but never the newline
        // that terminates whatever sits above it.
        let trimmed = self.src[..start].trim_end_matches([' ', '\t']);
        if trimmed.ends_with('\n') {
            start = trimmed.len() - 1;
        } else {
            start = trimmed.len();
        }

        self.splice((start, end), "");
        Ok(())
    }

    /// Insert (or replace) a member `"key": value` in the object at `path`
    /// (empty path = the root object). When the key already exists its value is
    /// replaced in place; otherwise the member is appended after the last one,
    /// matching its indentation. `raw_value` is literal JSON text — its own
    /// newlines are re-indented to sit under the new member.
    pub fn insert_member(&mut self, path: &str, key: &str, raw_value: &str) -> Result<(), String> {
        enum Plan {
            Replace((usize, usize)),
            /// Span of the object, plus whether it already holds a member. An
            /// object with none (a comment-only stub like Omarchy's shipped
            /// `omarchy-menu.jsonc`) must not get a separating comma — there is
            /// nothing to separate, and the comma would land inside a comment.
            Append((usize, usize), bool),
        }
        let plan = {
            let node = if path.is_empty() {
                &self.root
            } else {
                self.resolve(path)
                    .ok_or_else(|| format!("{path} not found"))?
            };
            let Node::Object { span, members } = node else {
                return Err(format!("{path} is not an object"));
            };
            match members.iter().find(|m| m.key == key) {
                Some(m) => Plan::Replace(m.value.span()),
                None => Plan::Append(*span, !members.is_empty()),
            }
        };
        match plan {
            Plan::Replace(vspan) => self.splice(vspan, raw_value),
            Plan::Append(span, has_members) => {
                let inner_span = (span.0 + 1, span.1 - 1);
                let inner = self.src[inner_span.0..inner_span.1].to_string();
                let mut chunks = split_top_level(&inner);
                // Peel the trailing whitespace off the last member (the newline
                // that sits before the closing brace) so it can follow the *new*
                // member instead — keeping the `}` on its own line.
                let mut trailing = String::new();
                if let Some(last) = chunks.last_mut() {
                    let keep = last.trim_end_matches([' ', '\t', '\r', '\n']).len();
                    trailing = last[keep..].to_string();
                    last.truncate(keep);
                }
                // Reuse the last member's leading whitespace as this one's indent
                // (falling back to a two-space block for a one-line object).
                let indent = chunks
                    .last()
                    .map(|c| {
                        let ws: String = c
                            .chars()
                            .take_while(|ch| matches!(ch, '\n' | '\r' | ' ' | '\t'))
                            .collect();
                        if ws.contains('\n') {
                            ws
                        } else {
                            "\n  ".to_string()
                        }
                    })
                    .unwrap_or_else(|| "\n  ".to_string());
                // Re-indent the value's continuation lines under the member.
                let base: String = indent.chars().filter(|c| matches!(c, ' ' | '\t')).collect();
                let value = raw_value.replace('\n', &format!("\n{base}"));
                chunks.push(format!("{indent}\"{key}\": {value}{trailing}"));
                let out = chunks.join(if has_members { "," } else { "" });
                self.splice(inner_span, &out);
            }
        }
        Ok(())
    }

    fn splice(&mut self, span: (usize, usize), replacement: &str) {
        let mut s = String::with_capacity(self.src.len());
        s.push_str(&self.src[..span.0]);
        s.push_str(replacement);
        s.push_str(&self.src[span.1..]);
        self.src = s;
        // spans are now stale; re-parse so subsequent edits are correct.
        if let Ok(re) = JsoncDoc::parse(&self.src) {
            self.root = re.root;
        }
    }
}

/// Split an array's inner text at top-level commas, respecting strings,
/// comments and nesting. Each returned chunk keeps its own whitespace/comments.
fn split_top_level(inner: &str) -> Vec<String> {
    let b = inner.as_bytes();
    let mut chunks = Vec::new();
    let (mut start, mut i, mut depth) = (0usize, 0usize, 0i32);
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b']' | b'}' => {
                depth -= 1;
                i += 1;
            }
            b',' if depth == 0 => {
                chunks.push(inner[start..i].to_string());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    let tail = &inner[start..];
    if !tail.trim().is_empty() {
        chunks.push(tail.to_string());
    }
    chunks
}

/// A chunk's identity for reordering: its first quoted string (a module name),
/// or the trimmed chunk if it has none.
fn chunk_key(chunk: &str) -> String {
    if let Some(q) = chunk.find('"') {
        if let Some(e) = chunk[q + 1..].find('"') {
            return format!("\"{}\"", &chunk[q + 1..q + 1 + e]);
        }
    }
    chunk.trim().to_string()
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_trivia(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\t' | b'\r' | b'\n' | b',' => self.i += 1,
                b'/' if self.i + 1 < self.b.len() && self.b[self.i + 1] == b'/' => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                b'/' if self.i + 1 < self.b.len() && self.b[self.i + 1] == b'*' => {
                    self.i += 2;
                    while self.i + 1 < self.b.len()
                        && !(self.b[self.i] == b'*' && self.b[self.i + 1] == b'/')
                    {
                        self.i += 1;
                    }
                    self.i += 2;
                }
                _ => break,
            }
        }
    }

    fn parse_value(&mut self) -> Result<Node, String> {
        self.skip_trivia();
        if self.i >= self.b.len() {
            return Err("unexpected end of input".into());
        }
        match self.b[self.i] {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            _ => self.parse_scalar(),
        }
    }

    fn parse_object(&mut self) -> Result<Node, String> {
        let start = self.i;
        self.i += 1; // '{'
        let mut members = Vec::new();
        loop {
            self.skip_trivia();
            if self.i >= self.b.len() {
                return Err("unterminated object".into());
            }
            if self.b[self.i] == b'}' {
                self.i += 1;
                break;
            }
            // key (a JSON string)
            let key_span = self.parse_string_span()?;
            let key = String::from_utf8_lossy(&self.b[key_span.0 + 1..key_span.1 - 1]).to_string();
            self.skip_trivia();
            if self.i >= self.b.len() || self.b[self.i] != b':' {
                return Err("expected ':' in object".into());
            }
            self.i += 1; // ':'
            let value = self.parse_value()?;
            members.push(Member { key, value });
        }
        Ok(Node::Object {
            span: (start, self.i),
            members,
        })
    }

    fn parse_array(&mut self) -> Result<Node, String> {
        let start = self.i;
        self.i += 1; // '['
        let mut elems = Vec::new();
        loop {
            self.skip_trivia();
            if self.i >= self.b.len() {
                return Err("unterminated array".into());
            }
            if self.b[self.i] == b']' {
                self.i += 1;
                break;
            }
            let v = self.parse_value()?;
            elems.push(v.span());
        }
        Ok(Node::Array {
            span: (start, self.i),
            elems,
        })
    }

    fn parse_scalar(&mut self) -> Result<Node, String> {
        if self.b[self.i] == b'"' {
            let span = self.parse_string_span()?;
            return Ok(Node::Scalar { span });
        }
        // number / bool / null: read until a structural/trivia char
        let start = self.i;
        while self.i < self.b.len()
            && !matches!(
                self.b[self.i],
                b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n' | b'/'
            )
        {
            self.i += 1;
        }
        Ok(Node::Scalar {
            span: (start, self.i),
        })
    }

    /// Parse a `"…"` string, returning its span (including the quotes).
    fn parse_string_span(&mut self) -> Result<(usize, usize), String> {
        if self.b[self.i] != b'"' {
            return Err("expected string".into());
        }
        let start = self.i;
        self.i += 1;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return Ok((start, self.i));
                }
                _ => self.i += 1,
            }
        }
        Err("unterminated string".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  // top comment
  "height": 26,
  "spacing": 0,
  "modules-left": ["custom/omarchy", "hyprland/workspaces"],
  "modules-right": [
    "network", // net
    "battery",
    "clock"
  ]
}"#;

    #[test]
    fn parses_and_round_trips_untouched() {
        let doc = JsoncDoc::parse(SAMPLE).unwrap();
        assert_eq!(doc.to_string(), SAMPLE);
    }

    #[test]
    fn reads_scalars_and_arrays() {
        let doc = JsoncDoc::parse(SAMPLE).unwrap();
        assert_eq!(doc.get("height").as_deref(), Some("26"));
        assert_eq!(
            doc.array_items("modules-left").unwrap(),
            vec!["\"custom/omarchy\"", "\"hyprland/workspaces\""]
        );
    }

    #[test]
    fn set_scalar_preserves_comments_and_rest() {
        let mut doc = JsoncDoc::parse(SAMPLE).unwrap();
        doc.set_scalar("height", "34").unwrap();
        let out = doc.to_string();
        assert!(out.contains("\"height\": 34,"));
        assert!(out.contains("// top comment")); // comments intact
        assert!(out.contains("\"spacing\": 0"));
        assert_eq!(doc.get("height").as_deref(), Some("34"));
    }

    #[test]
    fn reorder_array_by_name_preserves_comments_and_rest() {
        let mut doc = JsoncDoc::parse(SAMPLE).unwrap();
        doc.reorder(
            "modules-right",
            &[
                "\"clock\"".into(),
                "\"network\"".into(),
                "\"battery\"".into(),
            ],
        )
        .unwrap();
        let out = doc.to_string();
        // reordered by module name
        let clock = out.find("\"clock\"").unwrap();
        let network = out.find("\"network\"").unwrap();
        let battery = out.find("\"battery\"").unwrap();
        assert!(clock < network && network < battery, "reordered: {out}");
        // no comment was lost anywhere, and untouched content is intact
        assert!(out.contains("// net"));
        assert!(out.contains("// top comment"));
        assert!(out.contains("\"custom/omarchy\""));
        assert!(out.contains("\"height\": 26"));
        // the result still parses and the array reflects the new order
        let re = JsoncDoc::parse(&out).unwrap();
        assert_eq!(
            re.array_items("modules-right").unwrap(),
            vec!["\"clock\"", "\"network\"", "\"battery\""]
        );
    }

    #[test]
    fn add_and_remove_items_preserve_the_rest() {
        let mut doc = JsoncDoc::parse(SAMPLE).unwrap();
        doc.add_item("modules-left", "\"cpu\"").unwrap();
        assert_eq!(
            doc.array_items("modules-left").unwrap(),
            vec!["\"custom/omarchy\"", "\"hyprland/workspaces\"", "\"cpu\""]
        );
        // idempotent add
        doc.add_item("modules-left", "\"cpu\"").unwrap();
        assert_eq!(doc.array_items("modules-left").unwrap().len(), 3);
        // remove from another lane; structural (own-line) comments stay intact
        doc.remove_item("modules-right", "\"battery\"").unwrap();
        let items = doc.array_items("modules-right").unwrap();
        assert_eq!(items, vec!["\"network\"", "\"clock\""]);
        assert!(doc.to_string().contains("// top comment"));
        assert!(doc.to_string().contains("\"height\": 26"));
        // removing an absent module is a no-op
        doc.remove_item("modules-right", "\"nope\"").unwrap();
        assert_eq!(doc.array_items("modules-right").unwrap().len(), 2);
    }

    #[test]
    fn insert_member_appends_and_replaces() {
        let mut doc = JsoncDoc::parse(SAMPLE).unwrap();
        // append a new custom module object at the root
        doc.insert_member(
            "",
            "custom/hello",
            "{\n  \"exec\": \"echo hi\",\n  \"interval\": 5\n}",
        )
        .unwrap();
        let out = doc.to_string();
        assert!(out.contains("\"custom/hello\""));
        assert!(out.contains("\"exec\": \"echo hi\""));
        // untouched content survives, still parses, new member is reachable
        assert!(out.contains("// top comment"));
        let re = JsoncDoc::parse(&out).unwrap();
        assert_eq!(re.get("custom/hello.interval").as_deref(), Some("5"));
        assert_eq!(re.get("height").as_deref(), Some("26"));

        // inserting the same key again replaces its value in place
        let mut doc2 = JsoncDoc::parse(&out).unwrap();
        doc2.insert_member("", "custom/hello", "42").unwrap();
        assert_eq!(doc2.get("custom/hello").as_deref(), Some("42"));
        assert!(doc2.to_string().contains("// top comment"));
    }

    #[test]
    fn round_trips_the_real_waybar_config() {
        // vendored real config lives in tests/fixtures; here just re-check the
        // parser accepts nested groups/objects without losing bytes.
        let nested = r#"{
  "group/tray": { "modules": ["tray", "custom/x"] },
  "clock": { "format": "{:%H:%M}" } // time
}"#;
        let doc = JsoncDoc::parse(nested).unwrap();
        assert_eq!(doc.to_string(), nested);
        assert_eq!(doc.get("clock.format").as_deref(), Some("\"{:%H:%M}\""));
        assert_eq!(
            doc.array_items("group/tray.modules").unwrap(),
            vec!["\"tray\"", "\"custom/x\""]
        );
    }
}

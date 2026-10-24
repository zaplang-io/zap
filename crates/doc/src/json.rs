//! A tiny, self-contained JSON writer for the doc model.
//!
//! The toolchain ships no external dependencies beyond LLVM — `zap mcp`,
//! `zap lsp`, and `zap test --json` all hand-roll JSON (`driver/src/json.rs`),
//! so `zap doc` does too rather than pulling in `serde`. This writer only
//! *emits* (the doc model is never read back from JSON), and it is deliberately
//! minimal: objects keep insertion order and output is pretty-printed, so the
//! JSON contract is stable and diff-friendly for the #233 renderers and goldens.

use std::fmt::Write as _;

/// A JSON value. Object members keep insertion order for deterministic output.
pub enum Json {
    Null,
    Bool(bool),
    /// Non-negative integers (offsets, line/column, bitfield widths) — all the
    /// numbers the doc model emits.
    Num(u32),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// A JSON string from anything string-like.
    pub fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }

    /// A JSON number.
    pub fn num(n: u32) -> Json {
        Json::Num(n)
    }

    /// An empty object, filled with [`Json::push`].
    pub fn object() -> Json {
        Json::Obj(Vec::new())
    }

    /// An object from `(key, value)` pairs, in order.
    pub fn object_from(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Append a member to an object (no-op on a non-object).
    pub fn push(&mut self, key: &str, value: Json) {
        if let Json::Obj(members) = self {
            members.push((key.to_string(), value));
        }
    }

    /// Pretty-print with 2-space indentation, starting at `indent` levels.
    pub fn to_pretty(&self, indent: usize) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, indent);
        out
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => write_json_string(out, s),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(out, indent + 1);
                    item.write_pretty(out, indent + 1);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(out, indent);
                out.push(']');
            }
            Json::Obj(members) => {
                if members.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in members.iter().enumerate() {
                    pad(out, indent + 1);
                    write_json_string(out, k);
                    out.push_str(": ");
                    v.write_pretty(out, indent + 1);
                    if i + 1 < members.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(out, indent);
                out.push('}');
            }
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// Write `s` as a quoted, escaped JSON string.
fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

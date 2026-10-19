//! A minimal JSON value, parser, and serializer.
//!
//! The toolchain deliberately ships **no external dependencies** beyond LLVM
//! (the `--json` test reporter is hand-built for the same reason; see
//! `testing/report.rs`). The MCP server speaks JSON-RPC 2.0, so it needs to
//! both read and write JSON — this module provides just enough for that, with
//! no `serde`.
//!
//! The parser is a straightforward recursive-descent over the UTF-8 bytes of
//! the input; the serializer emits compact, single-line JSON (the MCP stdio
//! transport is newline-delimited, so a message must never contain a raw
//! newline — [`Json::to_string`] guarantees this by escaping them).

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

/// A JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// All JSON numbers are kept as `f64`. Integral values round-trip exactly
    /// for the ranges JSON-RPC ids and our payloads use, and serialize back
    /// without a fractional part (see [`Json::to_string`]).
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Object members, preserved in insertion order so output is stable.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Build an object from `(key, value)` pairs.
    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// A JSON string from anything string-like.
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    /// A JSON number from an integer.
    pub fn int(n: i64) -> Json {
        Json::Num(n as f64)
    }

    /// Look up a member of an object by key.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(xs) => Some(xs),
            _ => None,
        }
    }

    /// The number as `i64`, if this is an integral number.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 => Some(*n as i64),
            _ => None,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                if n.is_finite() {
                    if n.fract() == 0.0 && n.abs() < 1e15 {
                        let _ = write!(out, "{}", *n as i64);
                    } else {
                        let _ = write!(out, "{n}");
                    }
                } else {
                    // JSON has no NaN/Infinity; null is the conventional stand-in.
                    out.push_str("null");
                }
            }
            Json::Str(s) => write_json_string(out, s),
            Json::Arr(xs) => {
                out.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::Obj(members) => {
                out.push('{');
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(out, k);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl fmt::Display for Json {
    /// Serialize to compact, single-line JSON. `Json::to_string()` comes from
    /// this via the blanket `ToString` impl.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        self.write(&mut out);
        f.write_str(&out)
    }
}

/// Escape and quote a string into `out` (also escapes raw newlines, so a
/// serialized message is always a single line — required by the stdio
/// transport).
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

/// Parse a JSON document, returning the value or a human-readable error.
pub fn parse(input: &str) -> Result<Json, String> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing characters at byte {}", p.pos));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') | Some(b'f') => self.boolean(),
            Some(b'n') => self.null(),
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            Some(b) => Err(format!("unexpected byte {:?} at {}", b as char, self.pos)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.pos += 1; // '{'
        let mut members = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(members));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(format!("expected object key at {}", self.pos));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(format!("expected ':' at {}", self.pos));
            }
            self.pos += 1;
            let val = self.value()?;
            members.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(format!("expected ',' or '}}' at {}", self.pos)),
            }
        }
        Ok(Json::Obj(members))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            let val = self.value()?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(format!("expected ',' or ']' at {}", self.pos)),
            }
        }
        Ok(Json::Arr(items))
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err("unterminated string".to_string()),
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => s.push('"'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'/') => s.push('/'),
                        Some(b'n') => s.push('\n'),
                        Some(b't') => s.push('\t'),
                        Some(b'r') => s.push('\r'),
                        Some(b'b') => s.push('\u{0008}'),
                        Some(b'f') => s.push('\u{000c}'),
                        Some(b'u') => {
                            let cp = self.hex4()?;
                            // Handle UTF-16 surrogate pairs.
                            if (0xD800..=0xDBFF).contains(&cp) {
                                if self.bytes.get(self.pos + 1) == Some(&b'\\')
                                    && self.bytes.get(self.pos + 2) == Some(&b'u')
                                {
                                    self.pos += 2;
                                    let lo = self.hex4()?;
                                    let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    s.push(char::from_u32(c).ok_or("invalid surrogate pair")?);
                                } else {
                                    return Err("lone high surrogate".to_string());
                                }
                            } else {
                                s.push(char::from_u32(cp).ok_or("invalid \\u escape")?);
                            }
                        }
                        other => {
                            return Err(format!("invalid escape \\{:?}", other.map(|b| b as char)))
                        }
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    // Copy a full UTF-8 code point.
                    let start = self.pos;
                    let b = self.bytes[start];
                    let len = utf8_len(b);
                    let end = (start + len).min(self.bytes.len());
                    let chunk = std::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| "invalid UTF-8 in string".to_string())?;
                    s.push_str(chunk);
                    self.pos = end;
                }
            }
        }
        Ok(s)
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..4 {
            self.pos += 1;
            let b = self.peek().ok_or("truncated \\u escape")?;
            let d = (b as char)
                .to_digit(16)
                .ok_or("invalid hex in \\u escape")?;
            v = v * 16 + d;
        }
        Ok(v)
    }

    fn boolean(&mut self) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(Json::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(Json::Bool(false))
        } else {
            Err(format!("invalid literal at {}", self.pos))
        }
    }

    fn null(&mut self) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(Json::Null)
        } else {
            Err(format!("invalid literal at {}", self.pos))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "invalid number".to_string())?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number `{text}`"))
    }
}

/// The length in bytes of a UTF-8 code point from its lead byte.
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// Convert an ordered map of `&str -> Json` into an object. Convenience for
/// callers that build members in a map.
pub fn object_from(map: BTreeMap<String, Json>) -> Json {
    Json::Obj(map.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_scalars() {
        assert_eq!(parse("null").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("false").unwrap(), Json::Bool(false));
        assert_eq!(parse("42").unwrap(), Json::Num(42.0));
        assert_eq!(parse("-3.5").unwrap(), Json::Num(-3.5));
        assert_eq!(parse("\"hi\"").unwrap(), Json::Str("hi".into()));
    }

    #[test]
    fn parses_nested_structures() {
        let v = parse(r#"{"a":[1,2,{"b":"c"}],"d":null}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(
            v.get("a").unwrap().as_array().unwrap()[2]
                .get("b")
                .unwrap()
                .as_str(),
            Some("c")
        );
        assert_eq!(v.get("d"), Some(&Json::Null));
    }

    #[test]
    fn integers_serialize_without_fraction() {
        assert_eq!(Json::Num(7.0).to_string(), "7");
        assert_eq!(Json::int(-12).to_string(), "-12");
        assert_eq!(Json::Num(1.5).to_string(), "1.5");
    }

    #[test]
    fn strings_escape_control_and_newlines() {
        let s = Json::str("line1\nline2\t\"q\"");
        let out = s.to_string();
        assert!(!out.contains('\n'), "serialized form must be one line");
        assert_eq!(out, r#""line1\nline2\t\"q\"""#);
        // And it parses back to the original.
        assert_eq!(parse(&out).unwrap(), s);
    }

    #[test]
    fn parses_unicode_escapes_and_surrogates() {
        assert_eq!(parse(r#""A""#).unwrap(), Json::Str("A".into()));
        // U+1F600 as a surrogate pair.
        assert_eq!(parse(r#""😀""#).unwrap(), Json::Str("😀".into()));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse("1 2").is_err());
        assert!(parse("{").is_err());
    }

    #[test]
    fn object_preserves_insertion_order() {
        let o = Json::obj(vec![("z", Json::int(1)), ("a", Json::int(2))]);
        assert_eq!(o.to_string(), r#"{"z":1,"a":2}"#);
    }
}

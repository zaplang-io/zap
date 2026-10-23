//! End-to-end LSP session test (I#229): a full, ordered JSON-RPC exchange
//! through the public [`zapc::lsp::Server`] API over an in-memory byte stream —
//! `initialize` → `initialized` → `didOpen` (error) → `didChange` (fixed) →
//! `didClose` → `shutdown` → `exit` — asserting the server's `Content-Length`
//! framed replies and pushed `publishDiagnostics` at each step.

use std::io::Write;

use zapc::json::{self, Json};
use zapc::lsp::Server;

/// Frame JSON-RPC bodies into one LSP input stream.
fn frame(bodies: &[&str]) -> Vec<u8> {
    let mut buf = Vec::new();
    for b in bodies {
        write!(buf, "Content-Length: {}\r\n\r\n{}", b.len(), b).unwrap();
    }
    buf
}

/// Parse the framed output stream back into its JSON message bodies.
fn parse_frames(framed: &str) -> Vec<Json> {
    let mut msgs = Vec::new();
    let mut i = 0;
    while i < framed.len() {
        let rest = &framed[i..];
        let sep = rest.find("\r\n\r\n").expect("framed message separator");
        let len: usize = rest[..sep]
            .strip_prefix("Content-Length:")
            .expect("Content-Length header")
            .trim()
            .parse()
            .expect("numeric length");
        let body_start = i + sep + 4;
        let body = &framed[body_start..body_start + len];
        msgs.push(json::parse(body).expect("valid JSON body"));
        i = body_start + len;
    }
    msgs
}

fn diagnostics_of(msg: &Json) -> &[Json] {
    msg.get("params")
        .and_then(|p| p.get("diagnostics"))
        .and_then(Json::as_array)
        .expect("publishDiagnostics params")
}

/// The JSON-RPC response message with the given id (ignoring interleaved
/// notifications such as pushed diagnostics).
fn response(msgs: &[Json], id: i64) -> &Json {
    msgs.iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(id))
        .unwrap_or_else(|| panic!("no response for id {id}"))
}

/// Byte offset of the `n`-th (0-based) occurrence of `needle`.
fn find_byte(src: &str, needle: &str, n: usize) -> usize {
    let mut from = 0;
    for _ in 0..n {
        from = src[from..].find(needle).expect("occurrence") + from + needle.len();
    }
    src[from..].find(needle).expect("occurrence") + from
}

/// The 0-based (line, UTF-16 character) of a byte offset — the LSP position the
/// client would send.
fn line_char(src: &str, byte: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, c) in src.char_indices() {
        if i >= byte {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }
    (line, col)
}

/// A `didOpen` notification body for `uri` holding `text`.
fn did_open(uri: &str, text: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":{},"languageId":"zap","version":1,"text":{}}}}}}}"#,
        Json::str(uri),
        Json::str(text)
    )
}

/// A position-based request (definition / hover / completion) at `(line, ch)`.
fn pos_req(id: i64, method: &str, uri: &str, line: u32, ch: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":{},"params":{{"textDocument":{{"uri":{}}},"position":{{"line":{line},"character":{ch}}}}}}}"#,
        Json::str(method),
        Json::str(uri)
    )
}

#[test]
fn full_session_handshake_open_change_close_shutdown() {
    // `1 + "a"` is a type error (ZAP4012); `1 + 2` is clean.
    let bad = "let x = 1 + \"a\"\n";
    let good = "let x = 1 + 2\n";

    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file:///m.zap","languageId":"zap","version":1,"text":{}}}}}}}"#,
        Json::str(bad)
    );
    let change = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"file:///m.zap","version":2}},"contentChanges":[{{"text":{}}}]}}}}"#,
        Json::str(good)
    );

    let input = frame(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        &open,
        &change,
        r#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///m.zap"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ]);

    let mut out = Vec::new();
    let mut server = Server::new();
    let exited = server.run(&mut &input[..], &mut out).expect("session runs");
    assert!(exited, "session ended via exit notification");

    let msgs = parse_frames(&String::from_utf8(out).unwrap());
    // Expected replies, in order:
    //   1. initialize result
    //   2. publishDiagnostics (open, has error)
    //   3. publishDiagnostics (change, cleared)
    //   4. publishDiagnostics (close, cleared)
    //   5. shutdown result
    // (initialized / exit are notifications with no reply.)
    assert_eq!(msgs.len(), 5, "one reply per request + three publishes");

    // 1. initialize advertises UTF-16 + sync + the #230 interactive features.
    let caps = msgs[0].get("result").unwrap().get("capabilities").unwrap();
    assert_eq!(
        caps.get("positionEncoding").unwrap().as_str(),
        Some("utf-16")
    );
    assert_eq!(caps.get("hoverProvider"), Some(&Json::Bool(true)));

    // 2. open → a real diagnostic with a stable ZAPnnnn code.
    assert_eq!(
        msgs[1].get("method").unwrap().as_str(),
        Some("textDocument/publishDiagnostics")
    );
    let open_diags = diagnostics_of(&msgs[1]);
    assert!(!open_diags.is_empty(), "the bad program reports an error");
    let code = open_diags[0].get("code").unwrap().as_str().unwrap();
    assert!(code.starts_with("ZAP") && code.len() == 7, "got {code}");

    // 3. change to a valid program → diagnostics cleared.
    assert!(
        diagnostics_of(&msgs[2]).is_empty(),
        "the fixed program clears diagnostics"
    );

    // 4. close → diagnostics cleared.
    assert!(
        diagnostics_of(&msgs[3]).is_empty(),
        "close clears squiggles"
    );

    // 5. shutdown reply.
    assert_eq!(msgs[4].get("id").unwrap().as_i64(), Some(2));
    assert_eq!(msgs[4].get("result"), Some(&Json::Null));
}

/// Drive `initialize` + `didOpen` + a batch of requests through the server and
/// return the parsed reply messages.
fn session(setup: &[String]) -> Vec<Json> {
    let mut bodies: Vec<&str> =
        vec![r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#];
    for b in setup {
        bodies.push(b.as_str());
    }
    let exit = r#"{"jsonrpc":"2.0","method":"exit"}"#;
    bodies.push(exit);
    let input = frame(&bodies);
    let mut out = Vec::new();
    let mut server = Server::new();
    server.run(&mut &input[..], &mut out).expect("session runs");
    parse_frames(&String::from_utf8(out).unwrap())
}

const URI: &str = "file:///feat.zap";

/// A clean, parseable document exercising a struct+method, a free function, an
/// import and locals — the go-to-def / hover surface.
const DOC: &str = "\
import python \"numpy\" as np

struct Point:
    int64 x
    int64 y

    def mag(self) -> int64:
        \"The magnitude.\"
        return self.x + self.y

def area(int64 w, int64 h) -> int64:
    return w * h

def main():
    auto p = Point(1, 2)
    auto a = area(2, 3)
    auto m = p.mag()
    auto n = np.zeros(3)
";

#[test]
fn definition_jumps_to_function_struct_and_import() {
    // go-to-def on the `area(2, 3)` use → the `def area` declaration.
    let area_use = find_byte(DOC, "area", 1);
    let (ul, uc) = line_char(DOC, area_use);
    let area_def_line = line_char(DOC, find_byte(DOC, "def area", 0)).0;

    // go-to-def on the `np.zeros` use → the import.
    let np_use = find_byte(DOC, "np.zeros", 0);
    let (nl, nc) = line_char(DOC, np_use);
    let import_line = line_char(DOC, find_byte(DOC, "import python", 0)).0;

    let msgs = session(&[
        did_open(URI, DOC),
        pos_req(2, "textDocument/definition", URI, ul, uc),
        pos_req(3, "textDocument/definition", URI, nl, nc),
    ]);

    let area_loc = response(&msgs, 2).get("result").expect("definition result");
    assert_eq!(area_loc.get("uri").unwrap().as_str(), Some(URI));
    let start = area_loc.get("range").unwrap().get("start").unwrap();
    assert_eq!(
        start.get("line").unwrap().as_i64(),
        Some(area_def_line as i64),
        "area def line"
    );

    let np_loc = response(&msgs, 3).get("result").expect("import definition");
    assert_eq!(
        np_loc
            .get("range")
            .unwrap()
            .get("start")
            .unwrap()
            .get("line")
            .unwrap()
            .as_i64(),
        Some(import_line as i64),
        "import def line"
    );
}

#[test]
fn hover_shows_function_and_method_signatures_with_doc() {
    // hover on the `area(2, 3)` use.
    let (al, ac) = line_char(DOC, find_byte(DOC, "area", 1));
    // hover on the `p.mag()` method use (the `mag` in `p.mag`, not the one in
    // the "magnitude" docstring).
    let (ml, mc) = line_char(DOC, find_byte(DOC, "p.mag", 0) + "p.".len());

    let msgs = session(&[
        did_open(URI, DOC),
        pos_req(2, "textDocument/hover", URI, al, ac),
        pos_req(3, "textDocument/hover", URI, ml, mc),
    ]);

    let area_hover = response(&msgs, 2).get("result").expect("hover result");
    let area_md = area_hover
        .get("contents")
        .unwrap()
        .get("value")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        area_md.contains("def area(int64 w, int64 h) -> int64"),
        "signature in hover: {area_md}"
    );
    // A hover range is present.
    assert!(area_hover.get("range").is_some(), "hover carries a range");

    let mag_md = response(&msgs, 3)
        .get("result")
        .unwrap()
        .get("contents")
        .unwrap()
        .get("value")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        mag_md.contains("def mag(self) -> int64"),
        "method signature: {mag_md}"
    );
    assert!(mag_md.contains("The magnitude."), "docstring: {mag_md}");
}

#[test]
fn completion_scope_offers_in_scope_names() {
    let scope_doc = "\
def alpha() -> int64:
    return 1

def main():
    auto z = al
";
    let (l, c) = line_char(scope_doc, scope_doc.rfind("al").unwrap() + 2);
    let msgs = session(&[
        did_open(URI, scope_doc),
        pos_req(2, "textDocument/completion", URI, l, c),
    ]);
    let items = response(&msgs, 2)
        .get("result")
        .unwrap()
        .get("items")
        .unwrap()
        .as_array()
        .unwrap();
    let labels: Vec<&str> = items
        .iter()
        .map(|i| i.get("label").unwrap().as_str().unwrap())
        .collect();
    assert!(labels.contains(&"alpha"), "alpha offered: {labels:?}");
    assert!(!labels.contains(&"main"), "prefix-filtered: {labels:?}");
    // `alpha` comes back as a Function (kind 3).
    let alpha = items
        .iter()
        .find(|i| i.get("label").unwrap().as_str() == Some("alpha"))
        .unwrap();
    assert_eq!(alpha.get("kind").unwrap().as_i64(), Some(3));
}

#[test]
fn completion_member_after_dot_offers_fields_and_methods() {
    let member_doc = "\
struct Point:
    int64 x
    int64 y

    def mag(self) -> int64:
        return self.x

def main():
    auto p = Point(1, 2)
    auto q = p.
";
    // Cursor right after the trailing `p.` (a parse error the server repairs).
    let (l, c) = line_char(member_doc, member_doc.rfind("p.").unwrap() + 2);
    let msgs = session(&[
        did_open(URI, member_doc),
        pos_req(2, "textDocument/completion", URI, l, c),
    ]);
    let items = response(&msgs, 2)
        .get("result")
        .unwrap()
        .get("items")
        .unwrap()
        .as_array()
        .unwrap();
    let labels: Vec<&str> = items
        .iter()
        .map(|i| i.get("label").unwrap().as_str().unwrap())
        .collect();
    assert!(labels.contains(&"x"), "field x: {labels:?}");
    assert!(labels.contains(&"y"), "field y: {labels:?}");
    assert!(labels.contains(&"mag"), "method mag: {labels:?}");
    // `mag` is a Method (kind 2); `x` is a Field (kind 5).
    let kind_of = |name: &str| {
        items
            .iter()
            .find(|i| i.get("label").unwrap().as_str() == Some(name))
            .unwrap()
            .get("kind")
            .unwrap()
            .as_i64()
    };
    assert_eq!(kind_of("mag"), Some(2));
    assert_eq!(kind_of("x"), Some(5));
}

#[test]
fn document_symbol_returns_outline_tree() {
    let msgs = session(&[
        did_open(URI, DOC),
        format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{{"textDocument":{{"uri":{}}}}}}}"#,
            Json::str(URI)
        ),
    ]);
    let syms = response(&msgs, 2)
        .get("result")
        .unwrap()
        .as_array()
        .unwrap();
    let names: Vec<&str> = syms
        .iter()
        .map(|s| s.get("name").unwrap().as_str().unwrap())
        .collect();
    assert!(names.contains(&"Point"), "struct in outline: {names:?}");
    assert!(names.contains(&"area"), "function in outline: {names:?}");
    assert!(names.contains(&"main"), "function in outline: {names:?}");
    // Point carries its fields/methods as children.
    let point = syms
        .iter()
        .find(|s| s.get("name").unwrap().as_str() == Some("Point"))
        .unwrap();
    let children = point.get("children").unwrap().as_array().unwrap();
    let child_names: Vec<&str> = children
        .iter()
        .map(|c| c.get("name").unwrap().as_str().unwrap())
        .collect();
    assert!(child_names.contains(&"x"), "field child: {child_names:?}");
    assert!(
        child_names.contains(&"mag"),
        "method child: {child_names:?}"
    );
    // A struct maps to LSP SymbolKind::Struct (23).
    assert_eq!(point.get("kind").unwrap().as_i64(), Some(23));
}

#[test]
fn language_requests_on_unknown_document_are_null_or_empty() {
    // definition/hover with no open document → null; completion → empty list.
    let msgs = session(&[
        pos_req(2, "textDocument/definition", URI, 0, 0),
        pos_req(3, "textDocument/hover", URI, 0, 0),
        pos_req(4, "textDocument/completion", URI, 0, 0),
    ]);
    assert_eq!(response(&msgs, 2).get("result"), Some(&Json::Null));
    assert_eq!(response(&msgs, 3).get("result"), Some(&Json::Null));
    let items = response(&msgs, 4)
        .get("result")
        .unwrap()
        .get("items")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(items.is_empty(), "no completions without a document");
}

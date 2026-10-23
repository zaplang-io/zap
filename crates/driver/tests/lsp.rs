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

    // 1. initialize advertises UTF-16 + sync, nothing from #230.
    let caps = msgs[0].get("result").unwrap().get("capabilities").unwrap();
    assert_eq!(
        caps.get("positionEncoding").unwrap().as_str(),
        Some("utf-16")
    );
    assert!(caps.get("hoverProvider").is_none());

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

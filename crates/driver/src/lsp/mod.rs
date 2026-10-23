//! `zap lsp` — the Zap language server (I#157/B2, issue #229).
//!
//! A thin transport + dispatch layer over [`zapc_analysis`]. It speaks the
//! [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
//! over a `Content-Length`-framed JSON-RPC 2.0 stdio transport and, for #229,
//! implements exactly the "red squiggles" surface:
//!
//! - the `initialize` / `initialized` handshake and `shutdown` / `exit`
//!   lifecycle,
//! - the `textDocument/didOpen` / `didChange` / `didClose` / `didSave`
//!   document-sync lifecycle backed by an in-memory document store, and
//! - live `textDocument/publishDiagnostics`: on open/change/save it runs
//!   [`zapc_analysis::Analysis::analyze`] in-process (lex → parse → type-check,
//!   no LLVM) and maps the structured diagnostics — carrying their stable
//!   `ZAPnnnn` codes (#239) — to LSP diagnostics with correct 0-based UTF-16
//!   ranges (see [`position`]).
//!
//! Completion, hover and go-to-definition (#230) and the VS Code extension
//! (#231) are **not** here; the server advertises only what it implements.
//!
//! **No external dependencies.** Like the MCP server (`zap mcp`, #72), the
//! JSON-RPC layer is hand-rolled over the shared [`crate::json`] module rather
//! than pulling in `tower-lsp`/`lsp-types`/`serde`/`tokio` — matching the
//! toolchain's "nothing beyond LLVM" rule. The only transport difference from
//! MCP is the framing (LSP's `Content-Length` headers vs MCP's newline
//! delimiting) and that this server *pushes* notifications (`publishDiagnostics`)
//! as well as replying to requests. The core ([`Server::run`]) is
//! transport-agnostic over a `BufRead`/`Write` pair, which is what the tests
//! drive.

pub mod position;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use zapc_analysis::{Analysis, Severity};

use crate::json::{self, Json};
use position::PositionMapper;

/// LSP `DiagnosticSeverity`.
const SEVERITY_ERROR: i64 = 1;
const SEVERITY_WARNING: i64 = 2;

/// LSP `TextDocumentSyncKind::Full` — the whole document text is resent on every
/// change. Simple and correct; incremental sync is a later optimization.
const SYNC_FULL: i64 = 1;

/// Entry point for `zap lsp`. Serves over stdio until stdin reaches EOF (or an
/// `exit` notification). Follows the LSP exit-code rule: `exit` after a
/// `shutdown` is a clean exit (0); `exit` without one is an error (1); plain EOF
/// is clean.
pub fn serve() -> ExitCode {
    let mut server = Server::new();
    let stdin = io::stdin();
    let stdout = io::stdout();
    eprintln!(
        "zap lsp: serving over stdio (LSP); send Content-Length-framed JSON-RPC, EOF to stop"
    );
    match server.run(stdin.lock(), stdout.lock()) {
        Ok(exited_by_notification) => {
            if exited_by_notification && !server.shutdown_requested {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("zap lsp: I/O error: {e}");
            ExitCode::from(1)
        }
    }
}

/// What the loop should do after handling one message.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Exit,
}

/// The language server. Holds the open documents (`uri → text`) and the
/// shutdown state; everything else is derived per request by re-analysing a
/// document.
pub struct Server {
    /// Open documents by URI, each holding its current full text.
    docs: HashMap<String, String>,
    /// Set once the client sends `shutdown`; gates the `exit` code.
    shutdown_requested: bool,
}

impl Default for Server {
    fn default() -> Server {
        Server::new()
    }
}

impl Server {
    pub fn new() -> Server {
        Server {
            docs: HashMap::new(),
            shutdown_requested: false,
        }
    }

    /// Serve an LSP session over a reader/writer pair until EOF or `exit`.
    /// Returns `true` if the session ended because of an `exit` notification
    /// (vs EOF). Each outbound message — responses *and* server-initiated
    /// notifications — is written `Content-Length`-framed.
    pub fn run(&mut self, mut input: impl BufRead, mut output: impl Write) -> io::Result<bool> {
        while let Some(body) = read_frame(&mut input)? {
            let mut out = Vec::new();
            let flow = self.handle_body(&body, &mut out);
            for msg in &out {
                write_frame(&mut output, msg)?;
            }
            output.flush()?;
            if flow == Flow::Exit {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Parse and dispatch one message body, pushing any outbound messages onto
    /// `out`. A malformed body is answered with a JSON-RPC parse error.
    fn handle_body(&mut self, body: &str, out: &mut Vec<Json>) -> Flow {
        let msg = match json::parse(body) {
            Ok(v) => v,
            Err(e) => {
                out.push(error_response(
                    Json::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ));
                return Flow::Continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        let is_request = id.is_some();

        match method {
            // --- lifecycle requests ---
            "initialize" if is_request => {
                out.push(result_response(id.unwrap(), self.initialize()));
            }
            "shutdown" if is_request => {
                self.shutdown_requested = true;
                out.push(result_response(id.unwrap(), Json::Null));
            }
            "exit" => return Flow::Exit,
            "initialized" => {} // notification; nothing to do

            // --- document sync ---
            "textDocument/didOpen" => self.did_open(&params, out),
            "textDocument/didChange" => self.did_change(&params, out),
            "textDocument/didSave" => self.did_save(&params, out),
            "textDocument/didClose" => self.did_close(&params, out),

            // Unknown requests get a proper error; unknown notifications are
            // ignored (the client may send capabilities we don't implement).
            other => {
                if is_request {
                    out.push(error_response(
                        id.unwrap(),
                        -32601,
                        &format!("method not found: {other}"),
                    ));
                }
            }
        }
        Flow::Continue
    }

    /// The `initialize` result — advertises only the capabilities #229
    /// implements: full-text document sync (open/close/save) and pushed
    /// diagnostics, with UTF-16 position encoding.
    fn initialize(&self) -> Json {
        Json::obj(vec![
            (
                "capabilities",
                Json::obj(vec![
                    ("positionEncoding", Json::str("utf-16")),
                    (
                        "textDocumentSync",
                        Json::obj(vec![
                            ("openClose", Json::Bool(true)),
                            ("change", Json::int(SYNC_FULL)),
                            ("save", Json::obj(vec![("includeText", Json::Bool(false))])),
                        ]),
                    ),
                ]),
            ),
            (
                "serverInfo",
                Json::obj(vec![
                    ("name", Json::str("zap-lsp")),
                    ("version", Json::str(crate::VERSION)),
                ]),
            ),
        ])
    }

    fn did_open(&mut self, params: &Json, out: &mut Vec<Json>) {
        let Some(doc) = params.get("textDocument") else {
            return;
        };
        let (Some(uri), Some(text)) = (
            doc.get("uri").and_then(Json::as_str),
            doc.get("text").and_then(Json::as_str),
        ) else {
            return;
        };
        let uri = uri.to_string();
        self.docs.insert(uri.clone(), text.to_string());
        self.publish(&uri, out);
    }

    fn did_change(&mut self, params: &Json, out: &mut Vec<Json>) {
        let Some(uri) = params
            .get("textDocument")
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
        else {
            return;
        };
        // Full-sync: the last content change carries the whole new document text.
        let Some(text) = params
            .get("contentChanges")
            .and_then(Json::as_array)
            .and_then(|cs| cs.last())
            .and_then(|c| c.get("text"))
            .and_then(Json::as_str)
        else {
            return;
        };
        let uri = uri.to_string();
        self.docs.insert(uri.clone(), text.to_string());
        self.publish(&uri, out);
    }

    fn did_save(&mut self, params: &Json, out: &mut Vec<Json>) {
        let Some(uri) = params
            .get("textDocument")
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
        else {
            return;
        };
        // If the client includes the saved text, adopt it; otherwise re-analyse
        // the text we already hold.
        if let Some(text) = params.get("text").and_then(Json::as_str) {
            self.docs.insert(uri.to_string(), text.to_string());
        }
        let uri = uri.to_string();
        self.publish(&uri, out);
    }

    fn did_close(&mut self, params: &Json, out: &mut Vec<Json>) {
        let Some(uri) = params
            .get("textDocument")
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
        else {
            return;
        };
        self.docs.remove(uri);
        // Clear the client's squiggles for a closed document (LSP spec).
        out.push(publish_diagnostics(uri, Vec::new()));
    }

    /// Analyse the document at `uri` and push a `publishDiagnostics` for it.
    fn publish(&self, uri: &str, out: &mut Vec<Json>) {
        let Some(text) = self.docs.get(uri) else {
            return;
        };
        let analysis = Analysis::analyze(text);
        let mapper = PositionMapper::new(text);
        let diags = analysis
            .diagnostics()
            .iter()
            .map(|d| lsp_diagnostic(&mapper, d))
            .collect();
        out.push(publish_diagnostics(uri, diags));
    }
}

/// Map one [`zapc_analysis::Diagnostic`] to an LSP `Diagnostic` JSON object,
/// carrying the stable `ZAPnnnn` code and a `"zap"` source.
fn lsp_diagnostic(mapper: &PositionMapper, d: &zapc_analysis::Diagnostic) -> Json {
    let (start, end) = mapper.range(d.span);
    let severity = match d.severity {
        Severity::Error => SEVERITY_ERROR,
        Severity::Warning => SEVERITY_WARNING,
    };
    Json::obj(vec![
        (
            "range",
            Json::obj(vec![
                ("start", lsp_position(start)),
                ("end", lsp_position(end)),
            ]),
        ),
        ("severity", Json::int(severity)),
        // The code is the machine-stable identifier (#239); its `Display` is
        // exactly `ZAPnnnn`.
        ("code", Json::str(d.code.to_string())),
        ("source", Json::str("zap")),
        ("message", Json::str(d.message.clone())),
    ])
}

fn lsp_position(p: position::LspPos) -> Json {
    Json::obj(vec![
        ("line", Json::int(p.line as i64)),
        ("character", Json::int(p.character as i64)),
    ])
}

/// A `textDocument/publishDiagnostics` notification for `uri`.
fn publish_diagnostics(uri: &str, diagnostics: Vec<Json>) -> Json {
    notification(
        "textDocument/publishDiagnostics",
        Json::obj(vec![
            ("uri", Json::str(uri)),
            ("diagnostics", Json::Arr(diagnostics)),
        ]),
    )
}

/// A JSON-RPC 2.0 notification (no `id`).
fn notification(method: &str, params: Json) -> Json {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("method", Json::str(method)),
        ("params", params),
    ])
}

/// A JSON-RPC 2.0 success response.
fn result_response(id: Json, result: Json) -> Json {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        ("result", result),
    ])
}

/// A JSON-RPC 2.0 error response.
fn error_response(id: Json, code: i64, message: &str) -> Json {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        (
            "error",
            Json::obj(vec![
                ("code", Json::int(code)),
                ("message", Json::str(message)),
            ]),
        ),
    ])
}

/// Read one `Content-Length`-framed message, returning its body, or `None` at
/// EOF. Unknown headers (e.g. `Content-Type`) are ignored; a missing
/// `Content-Length` at EOF is treated as end of stream.
fn read_frame(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = input.read_line(&mut line)?;
        if n == 0 {
            // EOF. If we were mid-headers with a length, that is a truncated
            // message; treat it as end of stream either way.
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // blank line ends the header block
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
        // Other headers are ignored.
    }
    let len = match content_length {
        Some(l) => l,
        None => return Ok(None),
    };
    let mut buf = vec![0u8; len];
    input.read_exact(&mut buf)?;
    let body = String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(body))
}

/// Write one message `Content-Length`-framed. `Content-Length` counts bytes of
/// the UTF-8 body (`String::len`), as the protocol requires.
fn write_frame(output: &mut impl Write, msg: &Json) -> io::Result<()> {
    let body = msg.to_string();
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frame a list of JSON-RPC bodies into one LSP input stream.
    fn frame(bodies: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        for b in bodies {
            write!(buf, "Content-Length: {}\r\n\r\n{}", b.len(), b).unwrap();
        }
        buf
    }

    /// Run the server over framed input and return the framed output bytes.
    fn drive(bodies: &[&str]) -> String {
        let input = frame(bodies);
        let mut out = Vec::new();
        let mut server = Server::new();
        server.run(&mut &input[..], &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Split framed output into its JSON bodies.
    fn bodies(framed: &str) -> Vec<Json> {
        let mut msgs = Vec::new();
        let bytes = framed.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Find the header/body separator.
            let rest = &framed[i..];
            let sep = rest.find("\r\n\r\n").expect("framed message");
            let header = &rest[..sep];
            let len: usize = header
                .strip_prefix("Content-Length:")
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let body_start = i + sep + 4;
            let body = &framed[body_start..body_start + len];
            msgs.push(json::parse(body).unwrap());
            i = body_start + len;
        }
        msgs
    }

    #[test]
    fn initialize_advertises_only_implemented_capabilities() {
        let out = drive(&[r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#]);
        let msgs = bodies(&out);
        assert_eq!(msgs.len(), 1);
        let caps = msgs[0].get("result").unwrap().get("capabilities").unwrap();
        assert_eq!(
            caps.get("positionEncoding").unwrap().as_str(),
            Some("utf-16")
        );
        let sync = caps.get("textDocumentSync").unwrap();
        assert_eq!(sync.get("openClose"), Some(&Json::Bool(true)));
        assert_eq!(sync.get("change").unwrap().as_i64(), Some(SYNC_FULL));
        // Nothing beyond #229's surface is advertised.
        assert!(caps.get("completionProvider").is_none());
        assert!(caps.get("hoverProvider").is_none());
        assert!(caps.get("definitionProvider").is_none());
    }

    #[test]
    fn did_open_publishes_diagnostic_with_code_and_range() {
        // `1 + "x"` is a type error — a stable ZAP4xxx code.
        let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.zap","languageId":"zap","version":1,"text":"let x = 1 + \"a\"\n"}}}"#;
        let out = drive(&[open]);
        let msgs = bodies(&out);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].get("method").unwrap().as_str(),
            Some("textDocument/publishDiagnostics")
        );
        let params = msgs[0].get("params").unwrap();
        assert_eq!(params.get("uri").unwrap().as_str(), Some("file:///t.zap"));
        let diags = params.get("diagnostics").unwrap().as_array().unwrap();
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        let d = &diags[0];
        assert_eq!(d.get("source").unwrap().as_str(), Some("zap"));
        let code = d.get("code").unwrap().as_str().unwrap();
        assert!(
            code.starts_with("ZAP") && code.len() == 7,
            "stable code, got {code}"
        );
        // Severity is Error for a type error.
        assert_eq!(d.get("severity").unwrap().as_i64(), Some(SEVERITY_ERROR));
        // Range is on the first (0-based) line.
        let range = d.get("range").unwrap();
        assert_eq!(
            range.get("start").unwrap().get("line").unwrap().as_i64(),
            Some(0)
        );
    }

    #[test]
    fn did_change_to_valid_clears_diagnostics() {
        let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.zap","languageId":"zap","version":1,"text":"let x = 1 + \"a\"\n"}}}"#;
        let change = r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///t.zap","version":2},"contentChanges":[{"text":"let x = 1 + 2\n"}]}}"#;
        let out = drive(&[open, change]);
        let msgs = bodies(&out);
        assert_eq!(msgs.len(), 2);
        // First publish has the error; second clears it.
        let first = msgs[0].get("params").unwrap().get("diagnostics").unwrap();
        assert!(!first.as_array().unwrap().is_empty());
        let second = msgs[1].get("params").unwrap().get("diagnostics").unwrap();
        assert!(
            second.as_array().unwrap().is_empty(),
            "fixed document should clear diagnostics"
        );
    }

    #[test]
    fn did_close_clears_and_drops_document() {
        let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.zap","languageId":"zap","version":1,"text":"let x = 1 + \"a\"\n"}}}"#;
        let close = r#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///t.zap"}}}"#;
        let input = frame(&[open, close]);
        let mut out = Vec::new();
        let mut server = Server::new();
        server.run(&mut &input[..], &mut out).unwrap();
        let msgs = bodies(&String::from_utf8(out).unwrap());
        assert_eq!(msgs.len(), 2);
        let closed = msgs[1].get("params").unwrap().get("diagnostics").unwrap();
        assert!(closed.as_array().unwrap().is_empty());
        assert!(server.docs.is_empty(), "document dropped on close");
    }

    #[test]
    fn shutdown_then_exit_is_clean() {
        let shutdown = r#"{"jsonrpc":"2.0","id":9,"method":"shutdown"}"#;
        let exit = r#"{"jsonrpc":"2.0","method":"exit"}"#;
        let input = frame(&[shutdown, exit]);
        let mut out = Vec::new();
        let mut server = Server::new();
        let exited = server.run(&mut &input[..], &mut out).unwrap();
        assert!(exited, "exit notification ends the loop");
        assert!(server.shutdown_requested);
        let msgs = bodies(&String::from_utf8(out).unwrap());
        // Only the shutdown reply (exit yields no response).
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].get("id").unwrap().as_i64(), Some(9));
        assert_eq!(msgs[0].get("result"), Some(&Json::Null));
    }

    #[test]
    fn unknown_request_gets_method_not_found() {
        let out = drive(&[r#"{"jsonrpc":"2.0","id":5,"method":"textDocument/hover","params":{}}"#]);
        let msgs = bodies(&out);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].get("error").unwrap().get("code").unwrap().as_i64(),
            Some(-32601)
        );
    }

    #[test]
    fn malformed_body_is_parse_error() {
        let out = drive(&["{not json"]);
        let msgs = bodies(&out);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].get("error").unwrap().get("code").unwrap().as_i64(),
            Some(-32700)
        );
    }
}

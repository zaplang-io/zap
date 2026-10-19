//! `zap mcp` — the built-in MCP server for AI coding agents (D-0055, issue #72).
//!
//! `zap mcp` is a first-class agent interface, not merely a test runner. It
//! speaks the [Model Context Protocol](https://modelcontextprotocol.io) over a
//! newline-delimited JSON-RPC 2.0 stdio transport and exposes four surfaces:
//!
//! 1. **Language understanding** ([`reference`]) — the spec, grammar, design
//!    decisions, and canonical examples, served as MCP resources and via the
//!    `zap_reference` tool, so an LLM can write correct Zap with little or no
//!    Zap in its training corpus.
//! 2. **Fast checks without a build** ([`check`]) — parse + type-check only (no
//!    LLVM), returning structured diagnostics for a tight agent loop
//!    (`zap_check`).
//! 3. **Code graph / packed memory** ([`graph`]) — a compact semantic index of
//!    the codebase that agents query for token-efficient context
//!    (`zap_code_graph`).
//! 4. **Targeted testing** ([`test`]) — run tests on changed/affected paths,
//!    returning structured results (`zap_test`).
//! 5. **Repo/org conventions** ([`conventions`]) — an organization-authored
//!    conventions file checked into the repo, discovered at runtime and served
//!    as a resource and via the `conventions` tool, so an agent writes code in
//!    *this repo's* house style (issue #108).
//!
//! **No external dependencies**: the toolchain ships none beyond LLVM, so the
//! JSON layer ([`json`]) and the JSON-RPC/MCP protocol here are hand-rolled,
//! matching the hand-built `--json` test reporter (`testing/report.rs`). This
//! is a deliberate choice, flagged in the PR.
//!
//! Transport: **stdio** (the primary MCP transport). A socket transport is
//! noted by D-0055 as optional and is deferred (flagged in the PR); the server
//! core ([`Server::run`]) is transport-agnostic — it works over any
//! `BufRead`/`Write` pair — so adding one later is mechanical.

pub mod check;
pub mod conventions;
pub mod graph;
pub mod json;
pub mod reference;
pub mod test;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use json::Json;

/// The MCP protocol revision this server implements. Broadly supported by
/// current clients; if a client requests a different one at `initialize`, we
/// echo theirs back (the handshake is forward-compatible).
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Entry point for `zap mcp`. Serves over stdio until stdin reaches EOF.
pub fn serve(root: Option<String>) -> ExitCode {
    let root = root
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let server = Server::new(root);

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    eprintln!("zap mcp: serving over stdio (D-0055); send JSON-RPC, EOF to stop");
    match server.run(stdin.lock(), stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zap mcp: I/O error: {e}");
            ExitCode::from(1)
        }
    }
}

/// The MCP server. Holds the codebase root used by the code-graph and test
/// surfaces (overridable per call).
pub struct Server {
    root: PathBuf,
}

impl Server {
    pub fn new(root: PathBuf) -> Server {
        Server { root }
    }

    /// Serve the MCP session over a reader/writer pair until EOF. Each inbound
    /// line is one JSON-RPC message; each response is written as one line
    /// (the stdio transport is newline-delimited).
    pub fn run(&self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(&line) {
                output.write_all(response.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Handle one message line. Returns the serialized response line, or `None`
    /// for a notification (no `id`) or a message that warrants no reply.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        let msg = match json::parse(line) {
            Ok(v) => v,
            Err(e) => {
                // Parse error: id is unknown, so reply with null id (JSON-RPC).
                return Some(error_response(
                    Json::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ));
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Json::Null);

        // Notifications (no id) get no response.
        let is_notification = id.is_none();

        let outcome = self.dispatch(method, &params);

        match (is_notification, outcome) {
            (true, _) => None,
            (false, Ok(result)) => Some(result_response(id.unwrap(), result)),
            (false, Err((code, message))) => Some(error_response(id.unwrap(), code, &message)),
        }
    }

    /// Route a method to its handler. `Ok(result)` or `Err((code, message))`.
    fn dispatch(&self, method: &str, params: &Json) -> Result<Json, (i64, String)> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(Json::obj(vec![])),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(params),
            "resources/list" => Ok(Json::obj(vec![("resources", self.resources_list())])),
            "resources/read" => self.resources_read(params),
            "prompts/list" => Ok(Json::obj(vec![("prompts", Json::Arr(vec![]))])),
            // Notifications we accept silently.
            m if m.starts_with("notifications/") => Ok(Json::Null),
            other => Err((-32601, format!("method not found: {other}"))),
        }
    }

    fn initialize(&self, params: &Json) -> Json {
        let version = params
            .get("protocolVersion")
            .and_then(Json::as_str)
            .unwrap_or(PROTOCOL_VERSION)
            .to_string();
        Json::obj(vec![
            ("protocolVersion", Json::str(version)),
            (
                "capabilities",
                Json::obj(vec![
                    ("tools", Json::obj(vec![])),
                    (
                        "resources",
                        Json::obj(vec![("subscribe", Json::Bool(false))]),
                    ),
                ]),
            ),
            (
                "serverInfo",
                Json::obj(vec![
                    ("name", Json::str("zap-mcp")),
                    ("version", Json::str(crate::VERSION)),
                ]),
            ),
            (
                "instructions",
                Json::str(
                    "Zap's agent interface. Use zap_reference for the language \
                     spec/grammar/examples, zap_check to parse+type-check code \
                     without a build, zap_code_graph for a token-efficient map \
                     of the project, and zap_test to run targeted tests. \
                     Before writing or editing code in this repository, call \
                     the `conventions` tool (or read the zap://conventions \
                     resource) to follow the organization's house coding \
                     practices; when no conventions file is present it says so \
                     and you may proceed with idiomatic Zap.",
                ),
            ),
        ])
    }

    fn tools_list(&self) -> Json {
        Json::obj(vec![("tools", Json::Arr(tool_defs()))])
    }

    fn tools_call(&self, params: &Json) -> Result<Json, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Json::as_str)
            .ok_or((-32602, "missing tool name".to_string()))?;
        let args = params.get("arguments").cloned().unwrap_or(Json::Null);

        let result = match name {
            "zap_reference" => self.tool_reference(&args),
            "zap_check" => self.tool_check(&args),
            "zap_code_graph" => self.tool_code_graph(&args),
            "zap_test" => self.tool_test(&args),
            "conventions" => self.tool_conventions(&args),
            other => return Err((-32602, format!("unknown tool: {other}"))),
        };
        Ok(result.into_json())
    }

    /// The `resources/list` array: the embedded language reference plus, when
    /// present, this repo's conventions file (discovered under the root).
    fn resources_list(&self) -> Json {
        let mut resources = match reference::resources_json() {
            Json::Arr(v) => v,
            other => vec![other],
        };
        if let Some(c) = conventions::discover(&self.root) {
            resources.push(c.resource_descriptor());
        }
        Json::Arr(resources)
    }

    fn resources_read(&self, params: &Json) -> Result<Json, (i64, String)> {
        let uri = params
            .get("uri")
            .and_then(Json::as_str)
            .ok_or((-32602, "missing resource uri".to_string()))?;

        // The conventions resource is repo-specific (resolved under the root),
        // so it is handled here rather than via the embedded reference catalog.
        if uri == conventions::URI {
            let c = conventions::discover(&self.root)
                .ok_or((-32002, conventions::absent_message()))?;
            return Ok(contents(conventions::URI, conventions::MIME, c.body));
        }

        let doc =
            reference::find_by_uri(uri).ok_or((-32602, format!("no such resource: {uri}")))?;
        Ok(contents(&doc.uri(), doc.mime, doc.body.to_string()))
    }

    // ---- tool implementations ------------------------------------------------

    fn tool_reference(&self, args: &Json) -> ToolOutcome {
        match args.get("name").and_then(Json::as_str) {
            None => ToolOutcome::ok(reference::catalog_json().to_string()),
            Some(name) => match reference::find(name) {
                Some(doc) => ToolOutcome::ok_text(doc.body.to_string()),
                None => ToolOutcome::err(format!(
                    "no reference doc named `{name}`; call zap_reference with no \
                     name to list available docs"
                )),
            },
        }
    }

    fn tool_check(&self, args: &Json) -> ToolOutcome {
        let code = match self.resolve_source(args) {
            Ok(c) => c,
            Err(e) => return ToolOutcome::err(e),
        };
        let result = check::check_source(&code);
        ToolOutcome::ok(check::result_to_json(&result).to_string())
    }

    /// Resolve the source to check from `code` (inline) or `path` (a file,
    /// relative to the server root).
    fn resolve_source(&self, args: &Json) -> Result<String, String> {
        if let Some(code) = args.get("code").and_then(Json::as_str) {
            return Ok(code.to_string());
        }
        if let Some(path) = args.get("path").and_then(Json::as_str) {
            let full = self.root.join(path);
            return std::fs::read_to_string(&full)
                .map_err(|e| format!("cannot read {}: {e}", full.display()));
        }
        Err("zap_check needs `code` (a snippet) or `path` (a file)".to_string())
    }

    fn tool_code_graph(&self, args: &Json) -> ToolOutcome {
        let root = match args.get("root").and_then(Json::as_str) {
            Some(r) => self.root.join(r),
            None => self.root.clone(),
        };
        let index = graph::Index::build(&root);
        let name = args.get("name").and_then(Json::as_str);
        let kind = args.get("kind").and_then(Json::as_str);
        // With neither filter, return the whole index.
        let matches: Vec<&graph::Symbol> = if name.is_none() && kind.is_none() {
            index.symbols.iter().collect()
        } else {
            index.query(name, kind)
        };
        ToolOutcome::ok(index.to_json(&matches).to_string())
    }

    fn tool_test(&self, args: &Json) -> ToolOutcome {
        // Targeted testing: a list of changed/affected `paths`, or a single
        // `file`, or (none) the whole project.
        let mut paths: Vec<String> = Vec::new();
        if let Some(arr) = args.get("paths").and_then(Json::as_array) {
            for p in arr {
                if let Some(s) = p.as_str() {
                    paths.push(s.to_string());
                }
            }
        }
        if let Some(f) = args.get("file").and_then(Json::as_str) {
            paths.push(f.to_string());
        }
        test::run(&self.root, &paths)
    }

    /// Serve this repository's coding conventions (issue #108). Returns the
    /// file's markdown body, or a graceful "no conventions file present"
    /// message (not an error) so absence never breaks an agent loop.
    fn tool_conventions(&self, _args: &Json) -> ToolOutcome {
        match conventions::discover(&self.root) {
            Some(c) => ToolOutcome::ok_text(c.body),
            None => ToolOutcome::ok_text(conventions::absent_message()),
        }
    }
}

/// The result of a tool call before it is wrapped for the wire.
struct ToolOutcome {
    text: String,
    is_error: bool,
}

impl ToolOutcome {
    /// A successful structured (JSON-string) result.
    fn ok(text: String) -> ToolOutcome {
        ToolOutcome {
            text,
            is_error: false,
        }
    }

    /// A successful plain-text result (e.g. a reference doc body).
    fn ok_text(text: String) -> ToolOutcome {
        ToolOutcome {
            text,
            is_error: false,
        }
    }

    fn err(text: String) -> ToolOutcome {
        ToolOutcome {
            text,
            is_error: true,
        }
    }

    /// Wrap as an MCP `tools/call` result: `{ content: [text], isError }`.
    fn into_json(self) -> Json {
        Json::obj(vec![
            (
                "content",
                Json::Arr(vec![Json::obj(vec![
                    ("type", Json::str("text")),
                    ("text", Json::str(self.text)),
                ])]),
            ),
            ("isError", Json::Bool(self.is_error)),
        ])
    }
}

/// The `tools/list` tool definitions (name, description, JSON-Schema input).
fn tool_defs() -> Vec<Json> {
    fn schema(props: Vec<(&str, Json)>, required: Vec<&str>) -> Json {
        Json::obj(vec![
            ("type", Json::str("object")),
            ("properties", Json::obj(props)),
            (
                "required",
                Json::Arr(required.into_iter().map(Json::str).collect()),
            ),
        ])
    }
    fn prop(ty: &str, desc: &str) -> Json {
        Json::obj(vec![
            ("type", Json::str(ty)),
            ("description", Json::str(desc)),
        ])
    }

    vec![
        Json::obj(vec![
            ("name", Json::str("zap_reference")),
            (
                "description",
                Json::str(
                    "Serve the Zap language reference: the spec, grammar (EBNF), \
                     design decisions, and canonical examples. Call with no \
                     arguments to list available docs; pass `name` to fetch one. \
                     Use this to learn Zap syntax and semantics before writing code.",
                ),
            ),
            (
                "inputSchema",
                schema(
                    vec![(
                        "name",
                        prop("string", "Doc name from the catalog (e.g. `grammar`, `spec`, `ex-fib`). Omit to list all."),
                    )],
                    vec![],
                ),
            ),
        ]),
        Json::obj(vec![
            ("name", Json::str("zap_check")),
            (
                "description",
                Json::str(
                    "Parse and type-check Zap code WITHOUT building (no LLVM), \
                     returning structured diagnostics (severity, phase, message, \
                     byte span, line/col). Fast enough for a tight edit loop. \
                     Pass `code` (a snippet) or `path` (a file).",
                ),
            ),
            (
                "inputSchema",
                schema(
                    vec![
                        ("code", prop("string", "Zap source to check inline.")),
                        ("path", prop("string", "Path to a .zap file (relative to the project root).")),
                    ],
                    vec![],
                ),
            ),
        ]),
        Json::obj(vec![
            ("name", Json::str("zap_code_graph")),
            (
                "description",
                Json::str(
                    "Query a compact semantic index of the codebase: top-level \
                     symbols (functions, structs, enums, interfaces) with rendered \
                     signatures, files, lines, and members. Far fewer tokens than \
                     reading raw files. Filter by `name` (substring) and/or `kind`.",
                ),
            ),
            (
                "inputSchema",
                schema(
                    vec![
                        ("name", prop("string", "Case-insensitive substring to match symbol names.")),
                        ("kind", prop("string", "One of function, struct, enum, interface, annotation, decorator, field, method, variant.")),
                        ("root", prop("string", "Subdirectory to index (relative to the project root). Defaults to the whole project.")),
                    ],
                    vec![],
                ),
            ),
        ]),
        Json::obj(vec![
            ("name", Json::str("zap_test")),
            (
                "description",
                Json::str(
                    "Run Zap tests and return structured (JSON) results. Targeted: \
                     pass `paths` (changed/affected files or dirs) or a single \
                     `file` to run only those; omit to run the whole project. \
                     Requires a codegen-enabled build.",
                ),
            ),
            (
                "inputSchema",
                schema(
                    vec![
                        (
                            "paths",
                            Json::obj(vec![
                                ("type", Json::str("array")),
                                ("items", Json::obj(vec![("type", Json::str("string"))])),
                                ("description", Json::str("Changed/affected files or directories to test.")),
                            ]),
                        ),
                        ("file", prop("string", "A single test file or directory.")),
                    ],
                    vec![],
                ),
            ),
        ]),
        Json::obj(vec![
            ("name", Json::str("conventions")),
            (
                "description",
                Json::str(
                    "Fetch THIS repository's organization-authored coding \
                     conventions (naming, error handling, preferred modules, \
                     layering, testing, do's and don'ts) as markdown. Consult \
                     it BEFORE writing or editing code so your output matches \
                     the repo's house style. Takes no arguments. If no \
                     conventions file is present it says so (not an error).",
                ),
            ),
            ("inputSchema", schema(vec![], vec![])),
        ]),
    ]
}

/// Build a `resources/read` result: `{ contents: [{ uri, mimeType, text }] }`.
fn contents(uri: &str, mime: &str, text: String) -> Json {
    Json::obj(vec![(
        "contents",
        Json::Arr(vec![Json::obj(vec![
            ("uri", Json::str(uri)),
            ("mimeType", Json::str(mime)),
            ("text", Json::str(text)),
        ])]),
    )])
}

/// Build a JSON-RPC success response envelope.
fn result_response(id: Json, result: Json) -> String {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        ("result", result),
    ])
    .to_string()
}

/// Build a JSON-RPC error response envelope.
fn error_response(id: Json, code: i64, message: &str) -> String {
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
    .to_string()
}

/// Resolve a byte offset in `src` to a 1-based `(line, column)`.
pub fn span_to_line_col(src: &str, offset: u32) -> (u32, u32) {
    let offset = (offset as usize).min(src.len());
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, b) in src.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn server() -> Server {
        Server::new(std::env::current_dir().unwrap())
    }

    /// Drive one request line through a fresh server and parse the response.
    fn call(line: &str) -> Json {
        let resp = server().handle_line(line).expect("expected a response");
        json::parse(&resp).expect("response must be valid JSON")
    }

    #[test]
    fn line_col_resolution() {
        let src = "ab\ncd\n";
        assert_eq!(span_to_line_col(src, 0), (1, 1));
        assert_eq!(span_to_line_col(src, 1), (1, 2));
        assert_eq!(span_to_line_col(src, 3), (2, 1)); // first char of line 2
        assert_eq!(span_to_line_col(src, 999), (3, 1)); // clamped past EOF
    }

    #[test]
    fn initialize_reports_server_info_and_capabilities() {
        let resp = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        let result = resp.get("result").unwrap();
        assert_eq!(
            result
                .get("serverInfo")
                .unwrap()
                .get("name")
                .unwrap()
                .as_str(),
            Some("zap-mcp")
        );
        assert!(result.get("capabilities").unwrap().get("tools").is_some());
        assert_eq!(resp.get("id").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn notifications_get_no_response() {
        assert!(server()
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    #[test]
    fn unknown_method_is_a_jsonrpc_error() {
        let resp = call(r#"{"jsonrpc":"2.0","id":2,"method":"no/such"}"#);
        assert_eq!(
            resp.get("error").unwrap().get("code").unwrap().as_i64(),
            Some(-32601)
        );
    }

    #[test]
    fn tools_list_has_all_surfaces() {
        let resp = call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let tools = resp
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .as_array()
            .unwrap();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name")?.as_str())
            .collect();
        for want in [
            "zap_reference",
            "zap_check",
            "zap_code_graph",
            "zap_test",
            "conventions",
        ] {
            assert!(names.contains(&want), "missing tool {want}");
        }
    }

    #[test]
    fn initialize_instructions_advertise_conventions() {
        let resp = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        let instructions = resp
            .get("result")
            .unwrap()
            .get("instructions")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            instructions.contains("conventions"),
            "instructions must tell agents to consult conventions: {instructions}"
        );
    }

    #[test]
    fn check_tool_reports_a_bad_snippet() {
        let resp = call(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"zap_check","arguments":{"code":"def main():\n    nope()\n"}}}"#,
        );
        let content = resp
            .get("result")
            .unwrap()
            .get("content")
            .unwrap()
            .as_array()
            .unwrap();
        let text = content[0].get("text").unwrap().as_str().unwrap();
        let payload = json::parse(text).unwrap();
        assert_eq!(payload.get("ok"), Some(&Json::Bool(false)));
        assert!(!payload
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reference_tool_lists_and_fetches() {
        // Listing.
        let resp = call(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"zap_reference","arguments":{}}}"#,
        );
        let text = first_text(&resp);
        assert!(text.contains("grammar"));
        // Fetching the grammar.
        let resp = call(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"zap_reference","arguments":{"name":"grammar"}}}"#,
        );
        assert!(first_text(&resp).contains("EBNF"));
    }

    #[test]
    fn resources_list_and_read_round_trip() {
        let resp = call(r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#);
        let resources = resp
            .get("result")
            .unwrap()
            .get("resources")
            .unwrap()
            .as_array()
            .unwrap();
        let uri = resources
            .iter()
            .find_map(|r| {
                let u = r.get("uri")?.as_str()?;
                u.contains("grammar").then(|| u.to_string())
            })
            .expect("grammar resource present");
        let read = call(&format!(
            r#"{{"jsonrpc":"2.0","id":8,"method":"resources/read","params":{{"uri":"{uri}"}}}}"#
        ));
        let contents = read
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(contents[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("EBNF"));
    }

    #[test]
    fn run_processes_a_stream_of_lines() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            "\n"
        );
        let mut out = Vec::new();
        server().run(Cursor::new(input), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // initialize + ping replies; the notification produced nothing.
        assert_eq!(lines.len(), 2, "got: {text}");
        assert!(lines[0].contains("serverInfo"));
        assert!(lines[1].contains("\"id\":2"));
    }

    /// A unique temp directory to act as a server root.
    fn tmproot() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "zap-mcp-conv-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// Drive one request line through a server rooted at `s`.
    fn call_on(s: &Server, line: &str) -> Json {
        let resp = s.handle_line(line).expect("expected a response");
        json::parse(&resp).expect("response must be valid JSON")
    }

    #[test]
    fn conventions_tool_serves_file_when_present() {
        let root = tmproot();
        std::fs::write(
            root.join(conventions::DEFAULT_NAME),
            "# House style\nUse the `errors` Result helper, never panic in library code.\n",
        )
        .unwrap();
        let s = Server::new(root.clone());
        let resp = call_on(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"conventions","arguments":{}}}"#,
        );
        let text = first_text(&resp);
        assert!(text.contains("Result helper"), "got: {text}");
        assert_eq!(
            resp.get("result").unwrap().get("isError"),
            Some(&Json::Bool(false))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn conventions_tool_degrades_gracefully_when_absent() {
        let root = tmproot();
        let s = Server::new(root.clone());
        let resp = call_on(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"conventions","arguments":{}}}"#,
        );
        let text = first_text(&resp);
        assert!(text.contains("No conventions file present"), "got: {text}");
        // Absence is not an error — the surface degrades gracefully.
        assert_eq!(
            resp.get("result").unwrap().get("isError"),
            Some(&Json::Bool(false))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn conventions_resource_listed_and_readable_when_present() {
        let root = tmproot();
        std::fs::write(
            root.join(conventions::DEFAULT_NAME),
            "# Conventions\nPrefer snake_case module names.\n",
        )
        .unwrap();
        let s = Server::new(root.clone());

        // Listed alongside the reference resources.
        let list = call_on(&s, r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#);
        let resources = list
            .get("result")
            .unwrap()
            .get("resources")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(
            resources
                .iter()
                .any(|r| r.get("uri").and_then(|u| u.as_str()) == Some(conventions::URI)),
            "conventions resource must be listed"
        );

        // Readable by URI.
        let read = call_on(
            &s,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"zap://conventions"}}"#,
        );
        let contents = read
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(contents[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("snake_case"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn conventions_resource_absent_when_no_file() {
        let root = tmproot();
        let s = Server::new(root.clone());
        let list = call_on(&s, r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#);
        let resources = list
            .get("result")
            .unwrap()
            .get("resources")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(
            !resources
                .iter()
                .any(|r| r.get("uri").and_then(|u| u.as_str()) == Some(conventions::URI)),
            "conventions resource must not be listed when no file is present"
        );
        // Reading it is a JSON-RPC error (nothing to serve).
        let read = call_on(
            &s,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"zap://conventions"}}"#,
        );
        assert!(read.get("error").is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn conventions_tool_truncates_a_large_file() {
        let root = tmproot();
        let big = "x".repeat(conventions::SIZE_CAP + 5_000);
        std::fs::write(root.join(conventions::DEFAULT_NAME), &big).unwrap();
        let s = Server::new(root.clone());
        let resp = call_on(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"conventions","arguments":{}}}"#,
        );
        let text = first_text(&resp);
        assert!(text.contains("conventions truncated"), "expected truncation notice");
        assert!(text.len() < big.len(), "served body must be smaller than the file");
        std::fs::remove_dir_all(&root).ok();
    }

    fn first_text(resp: &Json) -> String {
        resp.get("result")
            .unwrap()
            .get("content")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }
}

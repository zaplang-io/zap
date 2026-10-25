//! A minimal `std::net` HTTP/1.1 server for the playground.
//!
//! The surface is tiny and fixed — `GET /`, `GET /static/*`, `POST /run`,
//! `GET /healthz` — so, like the LSP and MCP servers (which took no transport
//! dependency), this is hand-written over `std::net::TcpListener` with the
//! in-house `json` codec, rather than pulling in a web framework. It is a
//! blocking, thread-per-connection server bound to loopback by default.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::json::{self, Json};
use crate::playground::sandbox::{handle_run, PlaygroundConfig, RunResult, Status};

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const STYLE_CSS: &str = include_str!("assets/style.css");

/// Largest request body we will read for `POST /run` (snippets are small; this
/// bounds a hostile `Content-Length`).
const MAX_BODY: usize = 256 * 1024;

/// Run the playground HTTP server until the process is interrupted.
pub fn serve(host: String, port: u16, open: bool) -> ExitCode {
    let config = PlaygroundConfig::default();
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("zap playground: cannot bind {host}:{port}: {e}");
            return ExitCode::from(1);
        }
    };
    let addr = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("zap playground: {e}");
            return ExitCode::from(1);
        }
    };

    let url = format!("http://{host}:{}/", addr.port());
    eprintln!("Zap playground (I#238) listening on {url}");
    if !is_loopback(&host) {
        eprintln!(
            "WARNING: bound to a non-loopback address. The playground runs \
             arbitrary submitted code; do not expose it publicly without the \
             deployment hardening (OS sandbox) described in design/0026."
        );
    }
    eprintln!("Press Ctrl-C to stop.");
    if open {
        open_browser(&url);
    }

    serve_on(listener, config);
    ExitCode::SUCCESS
}

/// Serve the playground on an already-bound listener until the process ends.
/// Exposed so tests can drive the full HTTP surface on an ephemeral port.
pub fn serve_on(listener: TcpListener, config: PlaygroundConfig) {
    let gate = Arc::new(Semaphore::new(config.max_concurrent));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let config = config.clone();
                let gate = Arc::clone(&gate);
                thread::spawn(move || {
                    let _ = handle_conn(stream, &config, &gate);
                });
            }
            Err(e) => eprintln!("zap playground: accept error: {e}"),
        }
    }
}

/// A counting semaphore (permits for concurrent workers). `try_acquire` returns
/// a guard that releases on drop, or `None` when the pool is full.
struct Semaphore {
    permits: Mutex<usize>,
    cv: Condvar,
}

impl Semaphore {
    fn new(n: usize) -> Self {
        Self {
            permits: Mutex::new(n),
            cv: Condvar::new(),
        }
    }

    fn try_acquire(self: &Arc<Self>) -> Option<Permit> {
        let mut p = self.permits.lock().unwrap();
        if *p == 0 {
            return None;
        }
        *p -= 1;
        Some(Permit {
            sem: Arc::clone(self),
        })
    }
}

struct Permit {
    sem: Arc<Semaphore>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut p = self.sem.permits.lock().unwrap();
        *p += 1;
        self.sem.cv.notify_one();
    }
}

/// A parsed HTTP request (just what we need).
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn handle_conn(
    stream: TcpStream,
    config: &PlaygroundConfig,
    gate: &Arc<Semaphore>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let req = match read_request(&mut reader) {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(()), // empty/closed connection
        Err(_) => {
            return write_response(&mut writer, 400, "text/plain", b"bad request");
        }
    };

    match (req.method.as_str(), route(&req.path)) {
        ("GET", "/") => write_response(
            &mut writer,
            200,
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes(),
        ),
        ("GET", "/static/app.js") => write_response(
            &mut writer,
            200,
            "text/javascript; charset=utf-8",
            APP_JS.as_bytes(),
        ),
        ("GET", "/static/style.css") => write_response(
            &mut writer,
            200,
            "text/css; charset=utf-8",
            STYLE_CSS.as_bytes(),
        ),
        ("GET", "/healthz") => write_response(&mut writer, 200, "text/plain", b"ok"),
        ("POST", "/run") => {
            // Bound concurrent workers; shed load rather than overcommit.
            let Some(_permit) = gate.try_acquire() else {
                let body = Json::obj(vec![
                    ("status", Json::str("busy")),
                    (
                        "message",
                        Json::str("too many concurrent runs; try again shortly"),
                    ),
                ]);
                return write_response(
                    &mut writer,
                    429,
                    "application/json",
                    body.to_string().as_bytes(),
                );
            };
            let result = run_from_body(&req.body, config);
            write_response(
                &mut writer,
                200,
                "application/json",
                result.to_json().to_string().as_bytes(),
            )
        }
        ("GET", _) | ("POST", _) => write_response(&mut writer, 404, "text/plain", b"not found"),
        _ => write_response(&mut writer, 405, "text/plain", b"method not allowed"),
    }
}

/// Parse the `{ "code": "..." }` body and run it, mapping bad input to an
/// internal-status result rather than failing the request.
fn run_from_body(body: &[u8], config: &PlaygroundConfig) -> RunResult {
    let text = String::from_utf8_lossy(body);
    let code = match json::parse(text.trim()) {
        Ok(v) => v.get("code").and_then(Json::as_str).map(str::to_string),
        Err(_) => None,
    };
    match code {
        Some(code) => handle_run(&code, config),
        None => RunResult {
            status: Status::Internal,
            stdout: String::new(),
            diagnostics: Vec::new(),
            runtime_error: None,
            truncated: false,
            message: Some("request body must be JSON with a string \"code\" field".to_string()),
        },
    }
}

/// Read one HTTP/1.1 request: the request line, headers, and (for a body) up to
/// `Content-Length` bytes. Returns `None` on an immediately-closed connection.
fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "malformed request line",
        ));
    }

    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let trimmed = header.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }

    if content_length > MAX_BODY {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    Ok(Some(Request { method, path, body }))
}

/// Strip a query string and return the path for routing.
fn route(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// Write a complete HTTP/1.1 response and close.
fn write_response(
    writer: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         X-Content-Type-Options: nosniff\r\n\
         \r\n",
        body.len()
    );
    writer.write_all(head.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

fn is_loopback(host: &str) -> bool {
    host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost")
}

/// Best-effort open of the default browser (non-fatal if it fails).
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let _ = std::process::Command::new(launcher)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

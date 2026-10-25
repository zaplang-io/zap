//! I#238 playground end-to-end tests. Each run drives the real worker process
//! (`zap playground-worker`) through the sandbox, so these require the
//! `codegen` feature (the worker JITs the snippet through the REPL `Session`)
//! and run in the dev container under both collectors.
//!
//! Coverage mirrors the ticket: a clean snippet, a `ZAPnnnn` compile error, a
//! runtime `raise`, a timeout, an output-cap/runaway case, two concurrent
//! requests proving no shared state, and the HTTP surface end to end.

#![cfg(feature = "codegen")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use zapc::json;
use zapc::playground::{handle_run, serve_on, PlaygroundConfig, Status};

/// A test config pointing the worker at the built `zap` binary, with a short
/// wall-clock so the timeout test does not stall the suite.
fn cfg() -> PlaygroundConfig {
    PlaygroundConfig {
        worker_bin: PathBuf::from(env!("CARGO_BIN_EXE_zap")),
        // The budget bounds user code only (startup is not counted), so a small
        // value keeps the timeout test quick without risking false positives.
        timeout: Duration::from_secs(2),
        cpu_secs: 30,
        mem_bytes: 1024 * 1024 * 1024,
        output_cap: 64 * 1024,
        max_concurrent: 4,
    }
}

#[test]
fn clean_snippet_prints_and_reports_ok() {
    let r = handle_run("print(6 * 7)\n", &cfg());
    assert_eq!(r.status, Status::Ok, "message: {:?}", r.message);
    assert_eq!(r.stdout.trim(), "42");
    assert!(r.diagnostics.is_empty());
    assert!(r.runtime_error.is_none());
}

#[test]
fn auto_print_of_trailing_expression() {
    // REPL semantics: a trailing bare expression auto-prints its value.
    let r = handle_run("auto n = 21\nn + n\n", &cfg());
    assert_eq!(r.status, Status::Ok, "message: {:?}", r.message);
    assert_eq!(r.stdout.trim(), "42");
}

#[test]
fn type_error_surfaces_zap_code_without_crashing() {
    let r = handle_run("auto x = 1 + \"a\"\n", &cfg());
    assert_eq!(r.status, Status::Error, "message: {:?}", r.message);
    assert!(
        r.diagnostics.iter().any(|d| d.contains("error[ZAP")),
        "diagnostics: {:?}",
        r.diagnostics
    );
    assert!(r.runtime_error.is_none());
}

#[test]
fn runtime_exception_is_contained_and_reported() {
    let r = handle_run("raise \"boom\"\n", &cfg());
    assert_eq!(r.status, Status::RuntimeError, "message: {:?}", r.message);
    assert!(
        r.runtime_error
            .as_deref()
            .is_some_and(|e| e.contains("boom")),
        "runtime_error: {:?}",
        r.runtime_error
    );
}

#[test]
fn infinite_loop_times_out_and_server_survives() {
    let r = handle_run("auto i = 0\nwhile True:\n    i = i + 1\n", &cfg());
    assert_eq!(r.status, Status::Timeout, "message: {:?}", r.message);
    // The parent (and thus a server built on it) survives: a fresh run works.
    let next = handle_run("print(1 + 1)\n", &cfg());
    assert_eq!(next.status, Status::Ok, "message: {:?}", next.message);
    assert_eq!(next.stdout.trim(), "2");
}

#[test]
fn runaway_output_is_capped_and_contained() {
    let mut c = cfg();
    c.output_cap = 256;
    // Finite but far larger than the cap: the worker truncates to the cap.
    let r = handle_run(
        "auto i = 0\nwhile i < 20000:\n    print(\"x\")\n    i = i + 1\n",
        &c,
    );
    // Either truncated cleanly, or the hard file-size/time limit fired — all are
    // "contained"; none is a crash or a hang.
    assert!(
        matches!(r.status, Status::Ok | Status::OutputLimit | Status::Timeout),
        "status: {:?} message: {:?}",
        r.status,
        r.message
    );
    if r.status == Status::Ok {
        assert!(r.truncated, "expected truncation at the cap");
        assert!(r.stdout.len() <= 256);
        assert!(r.stdout.starts_with('x'));
    }
    // Server survives.
    let next = handle_run("print(7)\n", &cfg());
    assert_eq!(next.status, Status::Ok);
    assert_eq!(next.stdout.trim(), "7");
}

#[test]
fn concurrent_requests_do_not_share_state() {
    // Each request defines the SAME name with a different value and prints it.
    // If requests shared a Session/heap, the values could collide; isolated
    // worker processes guarantee each sees only its own.
    let mut handles = Vec::new();
    for (val, _) in [(111, ()), (222, ()), (333, ()), (444, ())] {
        handles.push(thread::spawn(move || {
            let src = format!("auto x = {val}\nprint(x)\n");
            let r = handle_run(&src, &cfg());
            (val, r)
        }));
    }
    for h in handles {
        let (val, r) = h.join().expect("thread");
        assert_eq!(r.status, Status::Ok, "message: {:?}", r.message);
        assert_eq!(r.stdout.trim(), val.to_string());
    }
}

// --- HTTP surface ----------------------------------------------------------

/// Start a server on an ephemeral loopback port and return its address.
fn start_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve_on(listener, cfg()));
    // Give the accept loop a moment to come up.
    thread::sleep(Duration::from_millis(100));
    addr
}

/// Issue a raw HTTP request and return `(status_line, body)`.
fn http(addr: std::net::SocketAddr, raw: &str) -> (String, String) {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.write_all(raw.as_bytes()).expect("write");
    let mut resp = String::new();
    s.read_to_string(&mut resp).expect("read");
    let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
    let status = head.lines().next().unwrap_or("").to_string();
    (status, body.to_string())
}

#[test]
fn http_get_root_serves_the_editor_page() {
    let addr = start_server();
    let (status, body) = http(
        addr,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(status.contains("200"), "status: {status}");
    assert!(body.contains("Zap Playground"), "body missing title");
}

#[test]
fn http_post_run_executes_and_returns_json() {
    let addr = start_server();
    let payload = json::Json::obj(vec![("code", json::Json::str("print(6 * 7)\n"))]).to_string();
    let req = format!(
        "POST /run HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    let (status, body) = http(addr, &req);
    assert!(status.contains("200"), "status: {status}");
    let v = json::parse(body.trim()).expect("json body");
    assert_eq!(v.get("status").and_then(json::Json::as_str), Some("ok"));
    assert!(
        v.get("stdout")
            .and_then(json::Json::as_str)
            .is_some_and(|s| s.trim() == "42"),
        "body: {body}"
    );
}

#[test]
fn http_unknown_route_is_404() {
    let addr = start_server();
    let (status, _) = http(
        addr,
        "GET /nope HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(status.contains("404"), "status: {status}");
}

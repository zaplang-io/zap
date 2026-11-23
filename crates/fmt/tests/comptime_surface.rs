//! `zap fmt` formatting of the `comptime` surface — S1 (#1297, epic #1296).
//! The parser surface is tested in `zapc_syntax`; here we pin that `zap fmt`
//! renders `comptime for` / `comptime <expr>` canonically and idempotently.
use zapc_fmt::format_source;

fn fmt(s: &str) -> String {
    format_source(s).unwrap_or_else(|e| panic!("format error for {s:?}: {e}"))
}

#[test]
fn comptime_for_formats_canonically_and_is_idempotent() {
    let src = "def g():\n    comptime for f in reflect.type_info(T).fields:\n        h(comptime f.index)\n";
    let got = fmt(src);
    assert!(
        got.contains("comptime for f in reflect.type_info(T).fields:"),
        "got:\n{got}"
    );
    assert!(got.contains("comptime f.index"), "got:\n{got}");
    assert_eq!(fmt(&got), got, "not idempotent:\n{got}");
}

#[test]
fn comptime_binding_formats_and_is_idempotent() {
    let src = "def g():\n    auto info = comptime reflect.type_info(T)\n";
    let got = fmt(src);
    assert!(got.contains("comptime reflect.type_info(T)"), "got:\n{got}");
    assert_eq!(fmt(&got), got, "not idempotent:\n{got}");
}

#[test]
fn comptime_dot_error_still_formats_as_a_call() {
    let src = "def g():\n    comptime.error(\"boom\")\n";
    let got = fmt(src);
    assert!(got.contains("comptime.error(\"boom\")"), "got:\n{got}");
    assert_eq!(fmt(&got), got, "not idempotent:\n{got}");
}

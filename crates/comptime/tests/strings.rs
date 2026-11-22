//! String and `char` operations (#1250): concat, ASCII case folding, codepoint
//! slicing/indexing, and the `len`/`ord`/`chr`/`char`/`str` builtins.
//!
//! Parity note on slicing: the runtime (`zap_str_slice` / `zap_slice_pos` /
//! `zap_slice_neg`) **clamps** out-of-range slice bounds Python-style and faults
//! only on a zero step — so a comptime out-of-range *slice* clamps too (that is
//! the parity-correct behavior), while an out-of-range *index* and a zero slice
//! *step* are the two slice/index faults.

mod common;

use common::{err, ok};
use zapc_comptime::{CtErrorKind, Value};

// ---------------------------------------------------------------------------
// concat + case
// ---------------------------------------------------------------------------

#[test]
fn concat() {
    assert_eq!(ok("\"foo\" + \"bar\""), Value::str("foobar"));
    assert_eq!(ok("\"\" + \"x\""), Value::str("x"));
    assert_eq!(ok("\"a\" + \"b\" + \"c\""), Value::str("abc"));
}

#[test]
fn ascii_case_folding() {
    // ASCII-only, matching zap_str_upper / zap_str_lower (non-ASCII unchanged).
    assert_eq!(ok("\"Hello\".upper()"), Value::str("HELLO"));
    assert_eq!(ok("\"Hello\".lower()"), Value::str("hello"));
    assert_eq!(ok("\"snake_case\".upper()"), Value::str("SNAKE_CASE"));
    assert_eq!(ok("\"über\".upper()"), Value::str("üBER"));
    assert_eq!(ok("\"ÜBER\".lower()"), Value::str("Über"));
}

// ---------------------------------------------------------------------------
// slicing (codepoint, clamping)
// ---------------------------------------------------------------------------

#[test]
fn basic_slices() {
    assert_eq!(ok("\"abcdef\"[1:4]"), Value::str("bcd"));
    assert_eq!(ok("\"abcdef\"[:3]"), Value::str("abc"));
    assert_eq!(ok("\"abcdef\"[3:]"), Value::str("def"));
    assert_eq!(ok("\"abcdef\"[:]"), Value::str("abcdef"));
}

#[test]
fn negative_and_stepped_slices() {
    assert_eq!(ok("\"abcdef\"[-2:]"), Value::str("ef"));
    assert_eq!(ok("\"abcdef\"[:-2]"), Value::str("abcd"));
    assert_eq!(ok("\"abcdef\"[::2]"), Value::str("ace"));
    assert_eq!(ok("\"abcdef\"[::-1]"), Value::str("fedcba"));
    assert_eq!(ok("\"abcdef\"[4:1:-1]"), Value::str("edc"));
}

#[test]
fn out_of_range_slice_clamps_like_runtime() {
    assert_eq!(ok("\"abc\"[0:99]"), Value::str("abc"));
    assert_eq!(ok("\"abc\"[-99:2]"), Value::str("ab"));
    // An empty (reversed) range yields "".
    assert_eq!(ok("\"abc\"[2:1]"), Value::str(""));
    assert_eq!(ok("\"abc\"[5:9]"), Value::str(""));
}

#[test]
fn slice_codepoints_not_bytes() {
    // "über" is 4 codepoints; slicing is by codepoint, not byte.
    assert_eq!(ok("\"über\"[0:2]"), Value::str("üb"));
    assert_eq!(ok("\"über\"[1:]"), Value::str("ber"));
}

#[test]
fn zero_slice_step_is_an_error() {
    let e = err("\"abc\"[::0]");
    assert_eq!(e.kind, CtErrorKind::Value);
    assert!(e.message.contains("step cannot be zero"), "{}", e.message);
}

// ---------------------------------------------------------------------------
// indexing
// ---------------------------------------------------------------------------

#[test]
fn string_index_returns_char() {
    assert_eq!(ok("\"abc\"[0]"), Value::Char('a'));
    assert_eq!(ok("\"abc\"[2]"), Value::Char('c'));
    assert_eq!(ok("\"abc\"[-1]"), Value::Char('c'));
    assert_eq!(ok("\"über\"[0]"), Value::Char('ü'));
}

#[test]
fn out_of_range_index_is_an_error() {
    for src in ["\"abc\"[3]", "\"abc\"[-4]", "\"\"[0]"] {
        let e = err(src);
        assert_eq!(e.kind, CtErrorKind::Value, "{src}");
        assert!(
            e.message.contains("index out of range"),
            "{src}: {}",
            e.message
        );
    }
}

// ---------------------------------------------------------------------------
// char ops + builtins
// ---------------------------------------------------------------------------

#[test]
fn ord_chr_roundtrip() {
    assert_eq!(ok("ord(\"A\"[0])"), Value::Int(65));
    assert_eq!(ok("chr(65)"), Value::Char('A'));
    assert_eq!(ok("ord(chr(0x263A))"), Value::Int(0x263A));
    assert_eq!(ok("char(97)"), Value::Char('a'));
}

#[test]
fn char_comparison_by_codepoint() {
    assert_eq!(ok("\"a\"[0] < \"b\"[0]"), Value::Bool(true));
    assert_eq!(ok("\"z\"[0] == \"z\"[0]"), Value::Bool(true));
    assert_eq!(ok("\"a\"[0] > \"Z\"[0]"), Value::Bool(true));
}

#[test]
fn invalid_codepoint_is_an_error() {
    for src in ["chr(-1)", "chr(0x110000)", "chr(0xD800)"] {
        let e = err(src);
        assert_eq!(e.kind, CtErrorKind::Value, "{src}");
        assert!(e.message.contains("Unicode scalar"), "{src}: {}", e.message);
    }
}

#[test]
fn len_builtin() {
    assert_eq!(ok("len(\"hello\")"), Value::Int(5));
    assert_eq!(ok("len(\"über\")"), Value::Int(4)); // codepoints, not bytes
    assert_eq!(ok("len(\"\")"), Value::Int(0));
    assert_eq!(ok("len([1, 2, 3])"), Value::Int(3));
    assert_eq!(ok("len((1, 2))"), Value::Int(2));
}

#[test]
fn str_builtin_textual_forms() {
    assert_eq!(ok("str(42)"), Value::str("42"));
    assert_eq!(ok("str(-7)"), Value::str("-7"));
    assert_eq!(ok("str(True)"), Value::str("True")); // Python-style, zap_bool_to_str
    assert_eq!(ok("str(False)"), Value::str("False"));
    assert_eq!(ok("str(\"x\")"), Value::str("x"));
    assert_eq!(ok("str(chr(0x263A))"), Value::str("☺"));
}

// ---------------------------------------------------------------------------
// list / tuple indexing + slicing + concat
// ---------------------------------------------------------------------------

#[test]
fn list_index_slice_concat() {
    assert_eq!(ok("[10, 20, 30][1]"), Value::Int(20));
    assert_eq!(ok("[10, 20, 30][-1]"), Value::Int(30));
    assert_eq!(
        ok("[1, 2, 3, 4][1:3]"),
        Value::list(vec![Value::Int(2), Value::Int(3)])
    );
    assert_eq!(
        ok("[1, 2] + [3, 4]"),
        Value::list(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4)
        ])
    );
}

#[test]
fn tuple_index() {
    assert_eq!(ok("(1, \"two\", True)[0]"), Value::Int(1));
    assert_eq!(ok("(1, \"two\", True)[1]"), Value::str("two"));
}

#[test]
fn membership_in_list_and_str() {
    assert_eq!(ok("2 in [1, 2, 3]"), Value::Bool(true));
    assert_eq!(ok("5 in [1, 2, 3]"), Value::Bool(false));
    assert_eq!(ok("5 not in [1, 2, 3]"), Value::Bool(true));
    assert_eq!(ok("\"bc\" in \"abcd\""), Value::Bool(true));
    assert_eq!(ok("\"xy\" in \"abcd\""), Value::Bool(false));
}

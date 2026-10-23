//! Input -> canonical-output golden cases for the documented style (D-0037),
//! each also checked for idempotence.
use zapc_fmt::format_source;

fn fmt(s: &str) -> String {
    format_source(s).unwrap_or_else(|e| panic!("format error for {s:?}: {e}"))
}

/// Assert `input` formats to `want`, and that `want` is a fixed point.
fn check(input: &str, want: &str) {
    let got = fmt(input);
    assert_eq!(
        got, want,
        "\n--- input ---\n{input}\n--- got ---\n{got}\n--- want ---\n{want}"
    );
    let again = fmt(want);
    assert_eq!(again, want, "not idempotent:\n{again}");
}

#[test]
fn indentation_is_four_spaces() {
    check("def f():\n  return 1\n", "def f():\n    return 1\n");
}

#[test]
fn operator_and_comma_spacing() {
    check("x=1+2*3\n", "x = 1 + 2 * 3\n");
    check("f(a ,b,  c)\n", "f(a, b, c)\n");
    check("auto d={'a':1,'b':2}\n", "auto d = {\"a\": 1, \"b\": 2}\n");
}

#[test]
fn precedence_parentheses_preserved() {
    check("auto y = (a + b) * c\n", "auto y = (a + b) * c\n");
    check("auto z = a + b * c\n", "auto z = a + b * c\n");
    check("auto w = not (a or b)\n", "auto w = not (a or b)\n");
}

#[test]
fn strings_normalize_to_double_quotes() {
    check("x = 'hi'\n", "x = \"hi\"\n");
    // keep single quotes to avoid escaping an embedded double quote
    check("x = 'a\"b'\n", "x = 'a\"b'\n");
}

#[test]
fn blank_lines_collapse_in_body() {
    check(
        "def f():\n    x = 1\n\n\n\n    y = 2\n",
        "def f():\n    x = 1\n\n    y = 2\n",
    );
}

#[test]
fn top_level_blanks_cap_at_two() {
    check("a = 1\n\n\n\n\nb = 2\n", "a = 1\n\n\nb = 2\n");
}

#[test]
fn one_tuple_keeps_trailing_comma() {
    check("t = (1,)\n", "t = (1,)\n");
    check("t = (1, 2)\n", "t = (1, 2)\n");
}

#[test]
fn imports_render_one_per_line() {
    check(
        "import a, b.c, d as e\n",
        "import a\nimport b.c\nimport d as e\n",
    );
}

#[test]
fn long_call_wraps_with_trailing_comma() {
    let input = "x = some_function(argument_one, argument_two, argument_three, argument_four, argument_five)\n";
    let want = "x = some_function(\n    argument_one,\n    argument_two,\n    argument_three,\n    argument_four,\n    argument_five,\n)\n";
    check(input, want);
}

#[test]
fn leading_comment_preserved() {
    check("# a note\nx = 1\n", "# a note\nx = 1\n");
}

#[test]
fn trailing_comment_two_spaces() {
    check("x = 1 # note\n", "x = 1  # note\n");
    check("x = 1    #note\n", "x = 1  # note\n");
}

#[test]
fn dangling_comment_in_block() {
    check(
        "def f():\n    x = 1\n    # trailing in body\n",
        "def f():\n    x = 1\n    # trailing in body\n",
    );
}

#[test]
fn comment_inside_collection_forces_break() {
    let input = "xs = [\n    1,  # one\n    # two is next\n    2,\n]\n";
    let want = "xs = [\n    1,  # one\n    # two is next\n    2,\n]\n";
    check(input, want);
}

#[test]
fn struct_and_methods() {
    check(
        "struct Point:\n    int64 x\n    int64 y\n    def mag(self) -> int64:\n        return self.x\n",
        "struct Point:\n    int64 x\n    int64 y\n    def mag(self) -> int64:\n        return self.x\n",
    );
}

#[test]
fn blank_lines_between_top_level_defs_preserved() {
    // A single blank between two top-level defs is kept (not swallowed by the
    // def's span running to the next token).
    check(
        "def a():\n    return 1\n\ndef b():\n    return 2\n",
        "def a():\n    return 1\n\ndef b():\n    return 2\n",
    );
    // Two blanks are kept (top-level cap is 2); three collapse to two.
    check(
        "def a():\n    return 1\n\n\n\ndef b():\n    return 2\n",
        "def a():\n    return 1\n\n\ndef b():\n    return 2\n",
    );
}

#[test]
fn dangling_body_comment_vs_next_item_comment() {
    // The indented comment stays dangling in f's body; the col-0 comment leads g.
    check(
        "def f():\n    x = 1\n    # inside f\n\n# about g\ndef g():\n    return 2\n",
        "def f():\n    x = 1\n    # inside f\n\n# about g\ndef g():\n    return 2\n",
    );
}

// --- comment footguns the starter flagged (#226) ---

#[test]
fn comment_between_decorator_and_target() {
    check(
        "@deco\n# why this is decorated\ndef f():\n    return 1\n",
        "@deco\n# why this is decorated\ndef f():\n    return 1\n",
    );
}

#[test]
fn trailing_comment_survives_token_move() {
    // The code tokens are re-spaced (`x=1+2` -> `x = 1 + 2`) but the trailing
    // comment stays glued to the same statement.
    check("x=1+2  # sum\n", "x = 1 + 2  # sum\n");
}

#[test]
fn trailing_comment_after_wrapped_close() {
    // When a call wraps, a comment trailing the whole statement attaches to the
    // closing-paren line.
    let input = "result = compute(argument_one, argument_two, argument_three, argument_four, argument_five)  # tally\n";
    let want = "result = compute(\n    argument_one,\n    argument_two,\n    argument_three,\n    argument_four,\n    argument_five,\n)  # tally\n";
    check(input, want);
}

#[test]
fn comment_inside_empty_container_stays_inside() {
    check(
        "xs = [\n    # nothing yet\n]\n",
        "xs = [\n    # nothing yet\n]\n",
    );
    check(
        "d = {\n    # nothing yet\n}\n",
        "d = {\n    # nothing yet\n}\n",
    );
}

/// Comment placement is itself a fixed point: a tricky mix of leading/trailing/
/// dangling comments does not drift on a second format.
#[test]
fn comment_placement_is_idempotent() {
    let input = "\
# module header
import a

# about f
def f(int64 n) -> int64:  # signature note
    # body lead
    auto xs = [
        1,  # one
        # dangling before two
        2,
    ]
    return n  # return note
    # trailing in body


# about g
def g():
    pass_through()
";
    let once = fmt(input);
    let twice = fmt(&once);
    assert_eq!(
        once, twice,
        "comment placement drifted:\n--- once ---\n{once}\n--- twice ---\n{twice}"
    );
    // Every comment from the input is still present.
    for c in [
        "# module header",
        "# about f",
        "# signature note",
        "# body lead",
        "# one",
        "# dangling before two",
        "# return note",
        "# trailing in body",
        "# about g",
    ] {
        assert!(once.contains(c), "lost comment {c:?} in:\n{once}");
    }
}

#[test]
fn empty_module_is_empty() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("\n\n\n"), "");
}

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
fn interface_refinement_round_trips() {
    // `interface X: Y` supertype lists survive formatting unchanged and are a
    // fixed point (D-0139/#806): single, multiple, and none.
    check(
        "interface Hashable: Equatable\n    def hash(self) -> int64\n",
        "interface Hashable: Equatable\n    def hash(self) -> int64\n",
    );
    check(
        "interface C:  A,B\n    def c(self) -> int64\n",
        "interface C: A, B\n    def c(self) -> int64\n",
    );
    check(
        "interface Equatable:\n    def eq(self) -> bool\n",
        "interface Equatable:\n    def eq(self) -> bool\n",
    );
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

#[test]
fn when_target_block_item_position() {
    // `when target.*` conditional compilation (I#139, D-0079): messy spacing in
    // the predicate normalizes; `when`/`elif`/`else` keep their own lines.
    check(
        "when target.arch==.x86_64:\n    def step()->int64:\n        return 1\nelif target.arch == .aarch64:\n    def step() -> int64:\n        return 2\nelse:\n    def step() -> int64:\n        return 3\n",
        "when target.arch == .x86_64:\n    def step() -> int64:\n        return 1\nelif target.arch == .aarch64:\n    def step() -> int64:\n        return 2\nelse:\n    def step() -> int64:\n        return 3\n",
    );
}

#[test]
fn when_target_block_stmt_position_and_has() {
    check(
        "def f() -> int64:\n    when target.has(.sse4_2):\n        auto v = 1\n    else:\n        auto v = 0\n    return v\n",
        "def f() -> int64:\n    when target.has(.sse4_2):\n        auto v = 1\n    else:\n        auto v = 0\n    return v\n",
    );
}

#[test]
fn when_target_os_predicate() {
    check(
        "when target.os==.linux:\n    def plat() -> str:\n        return \"linux\"\n",
        "when target.os == .linux:\n    def plat() -> str:\n        return \"linux\"\n",
    );
}

#[test]
fn chained_assignment_round_trips(/* #594 */) {
    // Canonical spacing: each target and the shared RHS joined by ` = `.
    check("def f():\n    a=b=c=0\n", "def f():\n    a = b = c = 0\n");
    // Subscript / field targets in a chain also round-trip.
    check(
        "def f():\n    xs[0]=d[\"k\"]=y=9\n",
        "def f():\n    xs[0] = d[\"k\"] = y = 9\n",
    );
    // A single assignment is unchanged (not promoted to a chain).
    check("def f():\n    a = 1\n", "def f():\n    a = 1\n");
}

#[test]
fn del_statement_round_trips(/* #595 */) {
    check("def f():\n    del  xs[1]\n", "def f():\n    del xs[1]\n");
    check("def f():\n    del d['k']\n", "def f():\n    del d[\"k\"]\n");
}

#[test]
fn display_unpacking_round_trips(/* #598 */) {
    // Canonical spacing: `*`/`**` attach to their operand with no space, and
    // ordinary comma/colon spacing applies around them.
    check(
        "def f():\n    xs=[1,*a ,2,*b]\n",
        "def f():\n    xs = [1, *a, 2, *b]\n",
    );
    check(
        "def f():\n    s={ *a,*b }\n",
        "def f():\n    s = {*a, *b}\n",
    );
    check(
        "def f():\n    t=(x,*rest)\n",
        "def f():\n    t = (x, *rest)\n",
    );
    check(
        "def f():\n    d={'a':1,** other,'b':2}\n",
        "def f():\n    d = {\"a\": 1, **other, \"b\": 2}\n",
    );
}

#[test]
fn multi_clause_comprehension_round_trips(/* #599 */) {
    // Each `for`/`if` clause is joined by a single space, in source order.
    check(
        "def f():\n    xs=[x*y for x in a for y in b if x!=y]\n",
        "def f():\n    xs = [x * y for x in a for y in b if x != y]\n",
    );
    check(
        "def f():\n    g={k:v for k in a if k>0 for v in b}\n",
        "def f():\n    g = {k: v for k in a if k > 0 for v in b}\n",
    );
}

#[test]
fn generator_expression_round_trips(/* #596 */) {
    // A bare genexpr keeps its parentheses and the comprehension clause spacing.
    check(
        "def f():\n    g=(x*x for x in a if x>0)\n",
        "def f():\n    g = (x * x for x in a if x > 0)\n",
    );
    // Multiple clauses, same grammar as a comprehension.
    check(
        "def f():\n    g=(x*y for x in a for y in b if x!=y)\n",
        "def f():\n    g = (x * y for x in a for y in b if x != y)\n",
    );
    // A genexpr as the sole call argument is re-emitted with parentheses from
    // the call; formatting is idempotent.
    check(
        "def f():\n    t=sum(x*x for x in a)\n",
        "def f():\n    t = sum((x * x for x in a))\n",
    );
}

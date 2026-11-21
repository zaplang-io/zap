//! Grammar-production **coverage gate** (#469 spec↔impl gaps, epic #374).
//!
//! `docs/GRAMMAR.ebnf` is hand-maintained, and until now nothing forced a
//! production that the grammar *advertises* to also be *implemented end to end*
//! in the compiler. `crates/syntax/tests/grammar.rs` (I#396) lints the EBNF's
//! structure and runs an accept/reject corpus through the **parser**, but two
//! holes remained:
//!
//!   1. **No completeness requirement.** A production can be added to the
//!      grammar (or an existing one left untested) with *no* example anywhere —
//!      more than half of the ~119 productions had no corpus citation at all —
//!      so "the grammar says X is legal" was never mechanically tied to "the
//!      compiler accepts X".
//!   2. **Parser-only.** `grammar.rs` stops at lex+parse. A construct can parse
//!      yet be rejected by sema or fail AST→ZIR lowering, and nothing noticed
//!      (the `select` statement was exactly this gap until #1002).
//!
//! This gate closes both. For **every** nonterminal reachable from the `module`
//! start symbol it requires exactly one of:
//!
//!   * a `Compiles` example — a minimal, self-contained program that exercises
//!     the production and passes the whole **host front end**: lex → parse →
//!     `when target.*` resolution → sema (`zapc_sema::check`) → AST→ZIR lowering
//!     (`zapc_ir::lower_module`). Everything up to (but not including) LLVM
//!     codegen runs on the host with no toolchain (crate `codegen` is the only
//!     LLVM-gated member), so this rides CI's host `cargo test` job like the
//!     conformance and differential harnesses; or
//!   * a `TrackedGap` example — a program that is grammatical per
//!     `docs/GRAMMAR.ebnf` yet the front end rejects *today*, pinned with a
//!     reason and a tracking reference. The gate asserts the gap still diverges,
//!     so when the implementation catches up this test fails and the maintainer
//!     graduates the entry to `Compiles` (the same discipline as `grammar.rs`'s
//!     `KNOWN_DIVERGENCES`).
//!
//! Net effect: **adding a production to `docs/GRAMMAR.ebnf` forces a matching
//! coverage entry** (a compiling example or a tracked, reasoned gap) or CI goes
//! red — the grammar can no longer silently outrun the implementation. The
//! examples are inline Rust string literals rather than `tests/conformance/`
//! `.zap` fixtures on purpose: the `TrackedGap` entries are deliberately
//! unimplemented and must not be swept up by `zap test` / `zap fmt --check`, and
//! keeping the status beside each production documents the spec↔impl frontier in
//! one place. This is the production-indexed complement to the claim-indexed
//! conformance suite (I#376) and the Python-parity differential suite (I#375).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// EBNF reader — a tiny hand model (no regex / no extra deps), kept in step with
// the one in `crates/syntax/tests/grammar.rs` so the two load-bearing checks
// read the grammar identically.
// ---------------------------------------------------------------------------

fn grammar_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/driver has a two-level parent")
        .join("docs")
        .join("GRAMMAR.ebnf")
}

fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_comment = false;
    while i < bytes.len() {
        if !in_comment && bytes[i] == b'(' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_comment = true;
            i += 2;
            continue;
        }
        if in_comment && bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b')' {
            in_comment = false;
            i += 2;
            continue;
        }
        let c = bytes[i] as char;
        if !in_comment {
            out.push(c);
        } else if c == '\n' {
            out.push('\n');
        }
        i += 1;
    }
    out
}

fn strip_string_literals(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_str = false;
    for c in src.chars() {
        if c == '"' {
            in_str = !in_str;
            out.push(' ');
        } else if in_str {
            out.push(if c == '\n' { '\n' } else { ' ' });
        } else {
            out.push(c);
        }
    }
    out
}

fn is_token_class(tok: &str) -> bool {
    !tok.is_empty() && tok.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

fn identifiers(rhs: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in rhs.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Parse `docs/GRAMMAR.ebnf` into `name -> rhs`.
fn load_rules() -> BTreeMap<String, String> {
    let raw = std::fs::read_to_string(grammar_path()).expect("read GRAMMAR.ebnf");
    let stripped = strip_string_literals(&strip_comments(&raw));
    let mut rules = BTreeMap::new();
    for chunk in stripped.split(';') {
        if let Some((lhs, rhs)) = chunk.split_once("::=") {
            if let Some(name) = lhs.split_whitespace().next_back() {
                rules.insert(name.to_string(), rhs.to_string());
            }
        }
    }
    rules
}

const START: &str = "module";

/// Every nonterminal reachable from `module` — the set the gate must cover.
fn reachable_productions(rules: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut reached = BTreeSet::new();
    let mut stack = vec![START.to_string()];
    while let Some(name) = stack.pop() {
        if !reached.insert(name.clone()) {
            continue;
        }
        if let Some(rhs) = rules.get(&name) {
            for tok in identifiers(rhs) {
                if !is_token_class(&tok) && !reached.contains(&tok) {
                    stack.push(tok);
                }
            }
        }
    }
    reached
}

// ---------------------------------------------------------------------------
// The production → minimal-example corpus.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    /// Passes the whole host front end (parse + cfg-resolve + sema + lowering).
    Compiles,
    /// Grammatical per `docs/GRAMMAR.ebnf` but the front end rejects it today;
    /// pinned so it stays visible and graduates when the impl catches up.
    TrackedGap,
}
use Status::*;

struct Cov {
    production: &'static str,
    status: Status,
    src: &'static str,
}

const fn cov(production: &'static str, status: Status, src: &'static str) -> Cov {
    Cov {
        production,
        status,
        src,
    }
}

/// How a `TrackedGap` production diverges today, and where it is tracked. Keyed
/// by production name; every `TrackedGap` entry in [`COVERAGE`] must appear here
/// (checked by [`tracked_gaps_are_documented`]).
#[rustfmt::skip]
const GAP_NOTES: &[(&str, &str)] = &[
    // None. The `select` statement (D-0031) — the last tracked gap — now parses,
    // type-checks and lowers end-to-end (#1002), so `select_stmt`/`select_case`/
    // `select_op` moved to `Compiles` above.
];

/// One entry per nonterminal reachable from `module`. The example is a complete,
/// self-contained program that exercises the named production; keep each minimal
/// and (for `Compiles`) sema-/lowering-clean. Generated from the A2 production ×
/// minimal-example sweep (grammar-gap scan) and then maintained by hand.
#[rustfmt::skip]
const COVERAGE: &[Cov] = &[
    cov("module",            Compiles , r#"def main():
    pass
"#),
    cov("item",              Compiles , r#"struct S:
    int64 x
"#),
    cov("module_binding",    Compiles , r#"const int64 N = 10
"#),
    cov("decorators",        Compiles , r#"decorator audited:
    pass
@audited
def f():
    pass
"#),
    cov("decorator_name",    Compiles , r#"decorator audited:
    pass
@audited
def f():
    pass
"#),
    cov("function",          Compiles , r#"def f(int64 a) -> int64:
    return a
"#),
    cov("generics",          Compiles , r#"def id[T](T x) -> T:
    return x
"#),
    cov("type_param",        Compiles , r#"def pick[T: Equatable](T x) -> T:
    return x
"#),
    cov("type_args",         Compiles , r#"def id[T](T x) -> T:
    return x
def f():
    auto y = id[int64](5)
    print(str(y))
"#),
    cov("param_list",        Compiles , r#"def f(int64 a, int64 b) -> int64:
    return a + b
"#),
    cov("param",             Compiles , r#"def f(int64... xs) -> int64:
    return 0
"#),
    cov("struct_def",        Compiles , r#"struct Point:
    int64 x
    int64 y
"#),
    cov("field",             Compiles , r#"struct S:
    int64 a = 0
"#),
    cov("interface_def",     Compiles , r#"interface Shape:
    def area(self) -> f64
"#),
    cov("supertype_list",    Compiles , r#"interface H2: Equatable
    def h(self) -> int64
"#),
    cov("method_sig",        Compiles , r#"interface I:
    def m(self) -> int64
"#),
    cov("assoc_type",        Compiles , r#"interface P[In]:
    type Out
    def make(self, In seed) -> Out
"#),
    cov("enum_def",          Compiles , r#"enum Color:
    Red
    Green
"#),
    cov("type_alias",        Compiles , r#"type Row = (int64, str)
"#),
    cov("enum_member",       Compiles , r#"enum Opt:
    Some(int64 v)
    None
"#),
    cov("variant_name",      Compiles , r#"enum Opt:
    Some(int64 v)
    None
"#),
    cov("variant_field",     Compiles , r#"enum E:
    V(int64 x)
    W
"#),
    cov("annotation_def",    Compiles , r#"annotation Meta:
    int64 x
"#),
    cov("decorator_def",     Compiles , r#"decorator audited:
    pass
"#),
    cov("decorator_hook",    Compiles , r#"decorator traced:
    def before(ctx):
        print(ctx.name)
"#),
    cov("hook_params",       Compiles , r#"decorator traced:
    def before(ctx):
        print(ctx.name)
"#),
    cov("wrap_body",         Compiles , r#"decorator retry(int64 times):
    def wrap(call):
        def inner():
            return call()
        return inner
"#),
    cov("wrap_inner",        Compiles , r#"decorator retry(int64 times):
    def wrap(call):
        def inner():
            return call()
        return inner
"#),
    cov("wrap_inner_params", Compiles , r#"decorator retry(int64 times):
    def wrap(call):
        def inner(*args):
            return call(*args)
        return inner
"#),
    cov("wrap_inner_param",  Compiles , r#"decorator retry(int64 times):
    def wrap(call):
        def inner(*args):
            return call(*args)
        return inner
"#),
    cov("import",            Compiles , r#"import python "os" as os
"#),
    cov("import_list",       Compiles , r#"import a, b as c
"#),
    cov("import_item",       Compiles , r#"import a as b
"#),
    cov("type",              Compiles , r#"def f():
    int64 x = 0
    x += 1
"#),
    cov("named_type",        Compiles , r#"def f():
    list[int64] xs = []
    xs.append(1)
"#),
    cov("type_name",         Compiles , r#"def f():
    int64 x = 0
    x += 1
"#),
    cov("tuple_type",        Compiles , r#"def f():
    (int64, str) t = (1, "a")
    print("ok")
"#),
    cov("func_type",         Compiles , r#"def g(int64 a) -> int64:
    return a
def f():
    fn(int64) -> int64 h = g
    print(str(h(1)))
"#),
    cov("block",             Compiles , r#"def f():
    pass
"#),
    cov("statement",         Compiles , r#"def f():
    pass
"#),
    cov("simple_stmt",       Compiles , r#"def f():
    pass
"#),
    cov("decl",              Compiles , r#"def f():
    auto x = 1
    print(str(x))
"#),
    cov("assign",            Compiles , r#"def f():
    auto a = 1
    auto b = 2
    a = b = 5
    print(str(a))
"#),
    cov("aug_assign",        Compiles , r#"def f():
    auto x = 1
    x += 1
    print(str(x))
"#),
    cov("subst_assign",      Compiles , r#"def f():
    auto s = "aaa"
    s ~= s/a/b/
    print(s)
"#),
    cov("del_stmt",          Compiles , r#"def f():
    auto d = {1: 2}
    del d[1]
"#),
    cov("index_target",      Compiles , r#"def f():
    auto d = {1: 2}
    del d[1]
"#),
    cov("aug_op",            Compiles , r#"def f():
    auto x = 1
    x -= 1
    print(str(x))
"#),
    cov("target",            Compiles , r#"def f():
    auto x = 1
    x = 2
    print(str(x))
"#),
    cov("send_stmt",         Compiles , r#"def f():
    auto ch = channel[int64]()
    ch <- 1
"#),
    cov("bind_target",       Compiles , r#"def f():
    auto t = (1, 2)
    auto a, b = t
    print(str(a))
"#),
    cov("target_tuple",      Compiles , r#"def f():
    auto t = (1, 2)
    auto a, b = t
    print(str(a))
"#),
    cov("target_elem",       Compiles , r#"def f():
    auto t = (1, 2, 3)
    auto a, *rest = t
    print(str(a))
"#),
    cov("compound_stmt",     Compiles , r#"def f(bool b):
    if b:
        pass
"#),
    cov("if_stmt",           Compiles , r#"def f(bool b):
    if b:
        pass
    elif b:
        pass
    else:
        pass
"#),
    cov("while_stmt",        Compiles , r#"def f(bool b):
    while b:
        pass
"#),
    cov("do_while_stmt",     Compiles , r#"def f(bool b):
    do:
        pass
    while b
"#),
    cov("for_stmt",          Compiles , r#"def f():
    for x in range(3):
        print(str(x))
"#),
    cov("try_stmt",          Compiles , r#"def f():
    try:
        raise ValueError("x")
    except Exception as e:
        pass
    finally:
        pass
"#),
    cov("with_stmt",         Compiles , r#"struct CM:
    int64 x
    def __enter__(self) -> int64:
        return self.x
    def __exit__(self):
        pass
def f():
    with CM(1) as v:
        print(str(v))
"#),
    cov("with_item",         Compiles , r#"struct CM:
    int64 x
    def __enter__(self) -> int64:
        return self.x
    def __exit__(self):
        pass
def f():
    with CM(1) as v:
        print(str(v))
"#),
    cov("scope_stmt",        Compiles , r#"def g():
    pass
def f():
    scope:
        spawn g()
"#),
    cov("unsafe_stmt",       Compiles , r#"def f():
    unsafe:
        pass
"#),
    cov("cfg_item",          Compiles , r#"when target.os == .linux:
    def f():
        pass
else:
    def f():
        pass
"#),
    cov("cfg_stmt",          Compiles , r#"def f():
    when target.os == .linux:
        pass
    else:
        pass
"#),
    cov("item_block",        Compiles , r#"when target.os == .linux:
    def f():
        pass
else:
    def f():
        pass
"#),
    cov("cfg_pred",          Compiles , r#"def f():
    when target.has(.avx2):
        pass
    else:
        pass
"#),
    cov("arch_name",         Compiles , r#"when target.arch == .x86_64:
    def f():
        pass
else:
    def f():
        pass
"#),
    cov("os_name",           Compiles , r#"when target.os == .macos:
    def f():
        pass
else:
    def f():
        pass
"#),
    cov("select_stmt",       Compiles  , r#"def f():
    auto ch = channel[int64]()
    select:
        case v = <-ch:
            pass
        default:
            pass
"#),
    cov("select_case",       Compiles  , r#"def f():
    auto ch = channel[int64]()
    select:
        case v = <-ch:
            pass
        default:
            pass
"#),
    cov("select_op",         Compiles  , r#"def f():
    auto ch = channel[int64]()
    select:
        case v = <-ch:
            pass
        default:
            pass
"#),
    cov("match_stmt",        Compiles , r#"def f(int64 x):
    match x:
        case 1:
            pass
        case _:
            pass
"#),
    cov("case_clause",       Compiles , r#"def f(int64 x):
    match x:
        case 1:
            pass
        case _:
            pass
"#),
    cov("pattern",           Compiles , r#"def f(list[int64] xs):
    match xs:
        case [a, b] as pair:
            print(str(a))
        case _:
            pass
"#),
    cov("or_pattern",        Compiles , r#"def f(int64 x):
    match x:
        case 1 | 2:
            pass
        case _:
            pass
"#),
    cov("closed_pattern",    Compiles , r#"def f(int64 x):
    match x:
        case _:
            pass
"#),
    cov("seq_pat",           Compiles , r#"def f(list[int64] xs):
    match xs:
        case [a, *rest]:
            print(str(a))
        case _:
            pass
"#),
    cov("pat_arg",           Compiles , r#"enum Opt:
    Some(int64 v)
    None
def f(Opt o):
    match o:
        case Opt.Some(v):
            print(str(v))
        case _:
            pass
"#),
    cov("expr",              Compiles , r#"def f():
    auto y = 1 + 2
    print(str(y))
"#),
    cov("lambda",            Compiles , r#"def f():
    fn(int64) -> int64 g = lambda x: x + 1
    print(str(g(1)))
"#),
    cov("yield_expr",        Compiles , r#"def gen():
    yield 1
"#),
    cov("ternary",           Compiles , r#"def f(bool b):
    auto y = 1 if b else 2
    print(str(y))
"#),
    cov("or_test",           Compiles , r#"def f(bool a, bool b):
    auto y = a or b
    print(str(y))
"#),
    cov("and_test",          Compiles , r#"def f(bool a, bool b):
    auto y = a and b
    print(str(y))
"#),
    cov("not_test",          Compiles , r#"def f(bool a):
    auto y = not a
    print(str(y))
"#),
    cov("comparison",        Compiles , r#"def f(int64 a, int64 b):
    auto y = a < b
    print(str(y))
"#),
    cov("comp_op",           Compiles , r#"def f(int64 x, list[int64] xs):
    auto b = x in xs
    print(str(b))
"#),
    cov("bit_or",            Compiles , r#"def f(int64 a, int64 b):
    auto y = a | b
    print(str(y))
"#),
    cov("bit_xor",           Compiles , r#"def f(int64 a, int64 b):
    auto y = a ^ b
    print(str(y))
"#),
    cov("bit_and",           Compiles , r#"def f(int64 a, int64 b):
    auto y = a & b
    print(str(y))
"#),
    cov("shift",             Compiles , r#"def f(int64 a, int64 b):
    auto y = a << b
    print(str(y))
"#),
    cov("add",               Compiles , r#"def f(int64 a, int64 b):
    auto y = a + b
    print(str(y))
"#),
    cov("term",              Compiles , r#"def f(int64 a, int64 b):
    auto y = a * b
    print(str(y))
"#),
    cov("factor",            Compiles , r#"def f(int64 a):
    auto y = -a
    print(str(y))
"#),
    cov("power",             Compiles , r#"def f(int64 a, int64 b):
    auto y = a ** b
    print(str(y))
"#),
    cov("postfix",           Compiles , r#"def f(list[int64] xs):
    xs.append(1)
"#),
    cov("trailer",           Compiles , r#"def f(list[int64] xs):
    auto n = len(xs)
    print(str(n))
"#),
    cov("call_expr",         Compiles , r#"def g() -> int64:
    return 1
def f():
    auto y = g()
    print(str(y))
"#),
    cov("subscript",         Compiles , r#"def f(list[int64] xs):
    auto y = xs[0]
    print(str(y))
"#),
    cov("index_elem",        Compiles , r#"def f(list[int64] xs):
    auto ys = xs[0:2]
    print(str(len(ys)))
"#),
    cov("arg_list",          Compiles , r#"def g(int64 a, int64 b) -> int64:
    return a + b
def f():
    auto y = g(1, 2)
    print(str(y))
"#),
    cov("arg",               Compiles , r#"def g(int64 a) -> int64:
    return a
def f():
    auto y = g(a=1)
    print(str(y))
"#),
    cov("atom",              Compiles , r#"def f():
    auto y = (1 + 2)
    print(str(y))
"#),
    cov("gen_exp",           Compiles , r#"def f(list[int64] xs):
    auto s = sum(x for x in xs)
    print(str(s))
"#),
    cov("named_expression",  Compiles , r#"def f(list[int64] xs):
    if (n := len(xs)) > 0:
        print(str(n))
"#),
    cov("star_elem",         Compiles , r#"def f(list[int64] xs):
    auto ys = [1, *xs, 2]
    print(str(len(ys)))
"#),
    cov("list_display",      Compiles , r#"def f():
    auto xs = [1, 2, 3]
    print(str(len(xs)))
"#),
    cov("brace_display",     Compiles , r#"def f():
    auto d = {1: 2}
    print(str(len(d)))
"#),
    cov("dict_items",        Compiles , r#"def f():
    auto d = {1: 2, 3: 4}
    print(str(len(d)))
"#),
    cov("dict_item",         Compiles , r#"def f():
    auto d = {1: 2, 3: 4}
    print(str(len(d)))
"#),
    cov("dict_comp",         Compiles , r#"def f(list[int64] ks):
    auto d = {k: 0 for k in ks}
    print(str(len(d)))
"#),
    cov("set_items",         Compiles , r#"def f():
    auto s = {1, 2, 3}
    print(str(len(s)))
"#),
    cov("comp_clauses",      Compiles , r#"def f(list[int64] xs):
    auto ys = [x for x in xs if x > 0]
    print(str(len(ys)))
"#),
    cov("comp_for",          Compiles , r#"def f(list[int64] xs):
    auto ys = [x for x in xs]
    print(str(len(ys)))
"#),
    cov("comp_if",           Compiles , r#"def f(list[int64] xs):
    auto ys = [x for x in xs if x > 0]
    print(str(len(ys)))
"#),
    cov("literal",           Compiles , r#"def f():
    auto a = 42
    auto b = 3.14
    auto c = "s"
    auto d = True
    print(str(a))
"#),
    cov("match_regex",       Compiles , r#"def f(str s):
    if s =~ m/foo/:
        print("y")
"#),
    cov("subst_regex",       Compiles , r#"def f():
    auto s = "aaa"
    s ~= s/a/b/
    print(s)
"#),
    cov("dotted_name",       Compiles , r#"import a as b
"#),
];

// ---------------------------------------------------------------------------
// The host front end, run in-process, mirroring the driver's order (parse →
// `when target.*` resolution → sema → AST→ZIR lowering). Returns the first
// error string at whichever stage first rejects, or `None` if the whole front
// end accepts the program.
// ---------------------------------------------------------------------------

fn front_end_first_error(src: &str) -> Option<String> {
    let (mut module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    if let Some(e) = lerrs.first() {
        return Some(format!("LEX: {e:?}"));
    }
    if let Some(e) = perrs.first() {
        return Some(format!("PARSE: {}", e.message));
    }
    // Resolve `when target.*` for the host target before lowering, exactly as
    // the driver does; an unresolved cfg block otherwise ICEs at lowering.
    zapc_syntax::cfg::resolve_cfg(&mut module, &zapc_syntax::cfg::Target::host());
    let serrs = zapc_sema::check(&module);
    if let Some(e) = serrs.first() {
        return Some(format!("SEMA[{}]: {}", e.code, e.message));
    }
    let (_zir, lerrs) = zapc_ir::lower_module(&module);
    if let Some(e) = lerrs.first() {
        return Some(format!("LOWER[{}]: {}", e.code, e.message));
    }
    None
}

// ---------------------------------------------------------------------------
// The gate.
// ---------------------------------------------------------------------------

/// **Completeness.** Every production reachable from `module` has exactly one
/// coverage entry, and every coverage entry names a real reachable production.
/// Adding a production to `docs/GRAMMAR.ebnf` without a `Compiles` example or a
/// `TrackedGap` entry fails here — the grammar can no longer outrun the corpus.
#[test]
fn every_reachable_production_is_covered() {
    let rules = load_rules();
    let reachable = reachable_productions(&rules);

    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut duplicates: Vec<&str> = Vec::new();
    for c in COVERAGE {
        if !covered.insert(c.production.to_string()) {
            duplicates.push(c.production);
        }
    }
    assert!(
        duplicates.is_empty(),
        "COVERAGE lists these productions more than once: {duplicates:?}",
    );

    // Every cited production must exist and be reachable (catches a renamed or
    // removed production left behind in the corpus).
    let unknown: Vec<&String> = covered.difference(&reachable).collect();
    assert!(
        unknown.is_empty(),
        "COVERAGE cites productions that are not reachable from `{START}` in \
         GRAMMAR.ebnf (renamed or removed?): {unknown:?}",
    );

    // Every reachable production must be covered.
    let missing: Vec<&String> = reachable.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "these GRAMMAR.ebnf productions are reachable from `{START}` but have no \
         coverage entry — add a `Compiles` example that exercises each, or a \
         `TrackedGap` entry with a reason in GAP_NOTES: {missing:?}",
    );
}

/// **Examples hold.** Each `Compiles` example passes the whole host front end;
/// each `TrackedGap` example is still rejected somewhere in it (and is
/// documented in `GAP_NOTES`). A gap that starts compiling, or a `Compiles`
/// example that regresses, fails here.
#[test]
fn coverage_examples_match_their_status() {
    let gap_notes: BTreeMap<&str, &str> = GAP_NOTES.iter().copied().collect();
    let mut failures: Vec<String> = Vec::new();

    for c in COVERAGE {
        let err = front_end_first_error(c.src);
        match c.status {
            Compiles => {
                if let Some(e) = err {
                    failures.push(format!(
                        "[{}] marked Compiles but the host front end rejected it: {e}\n    src: {:?}",
                        c.production, c.src,
                    ));
                }
            }
            TrackedGap => {
                if err.is_none() {
                    failures.push(format!(
                        "[{}] marked TrackedGap but the host front end now ACCEPTS it — \
                         the implementation caught up. Change its status to `Compiles` \
                         and drop its GAP_NOTES entry.",
                        c.production,
                    ));
                }
                if !gap_notes.contains_key(c.production) {
                    failures.push(format!(
                        "[{}] is TrackedGap but has no GAP_NOTES entry explaining the gap.",
                        c.production,
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "grammar-production coverage gate found {} problem(s):\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// Every `GAP_NOTES` key is an actual `TrackedGap` production (no stale notes).
#[test]
fn tracked_gaps_are_documented() {
    let tracked: BTreeSet<&str> = COVERAGE
        .iter()
        .filter(|c| c.status == TrackedGap)
        .map(|c| c.production)
        .collect();
    let noted: BTreeSet<&str> = GAP_NOTES.iter().map(|(p, _)| *p).collect();
    let stale: Vec<&&str> = noted.difference(&tracked).collect();
    assert!(
        stale.is_empty(),
        "GAP_NOTES documents productions that are not marked TrackedGap in \
         COVERAGE (stale after a graduation?): {stale:?}",
    );
}

// ---------------------------------------------------------------------------
// Negative / boundary corpus.
//
// The `COVERAGE` corpus above proves every production is *accepted*; it says
// nothing about what the front end *rejects*. This table pins the other side:
// source that is either malformed, a construct the grammar deliberately does
// **not** describe (so it must stay rejected — a regression that started
// accepting it would be silent impl-ahead-of-grammar drift), or a form that is
// grammatical per `docs/GRAMMAR.ebnf` yet the implementation rejects today
// (a grammar-ahead-of-impl gap, tracked by an issue). Each entry must be
// rejected somewhere in the host front end (parse, sema, or AST→ZIR lowering);
// where the diagnostic is stable, `expect` pins a `ZAP####` code or message
// fragment so the *reason* for rejection can't silently change either.
//
// This is the complement to `crates/syntax/tests/grammar.rs`, whose
// accept/reject corpus covers pure parser boundaries; the cases here reach
// sema and lowering too (the depths the parser-only corpus cannot see).
// ---------------------------------------------------------------------------

struct Reject {
    /// What the snippet is (for failure messages).
    label: &'static str,
    src: &'static str,
    /// A stable substring of the first front-end error (a `ZAP####` code or a
    /// message fragment), or `None` where the diagnostic is deliberately not
    /// pinned (e.g. an incidental/indirect message we don't want to freeze).
    expect: Option<&'static str>,
    /// Why it is rejected / where it is tracked.
    reason: &'static str,
}

const fn rej(
    label: &'static str,
    src: &'static str,
    expect: Option<&'static str>,
    reason: &'static str,
) -> Reject {
    Reject {
        label,
        src,
        expect,
        reason,
    }
}

#[rustfmt::skip]
const REJECTS: &[Reject] = &[
    // --- grammatical shape, but rejected by sema (deferred/deliberate) ---
    rej("yield_bare",
        "def gen():\n    yield\n",
        Some("ZAP5064"),
        "`yield` grammar is `\"yield\" [expr]`, but v1 requires an operand (I#67)."),
    rej("mixed_except_and_except_star",
        "def f():\n    try:\n        raise ValueError(\"x\")\n    except ValueError as e:\n        pass\n    except* KeyError as eg:\n        pass\n",
        Some("mix"),
        "a `try` cannot mix `except` and `except*` clauses (#605, D-0129)."),
    rej("spawn_of_a_lambda",
        "def f():\n    spawn (lambda: 1)()\n",
        Some("ZAP5076"),
        "spawn requires a named function / method call / fn-value, not a bare lambda call (D-0044)."),
    rej("matmul_on_scalars",
        "def f(int64 a, int64 b):\n    auto y = a @ b\n    print(str(y))\n",
        Some("ZAP4012"),
        "`@` (matrix multiply) is array-only; scalars are a type error (#509)."),
    rej("mapping_pattern",
        "def f(dict[str, int64] d):\n    match d:\n        case {\"k\": v}:\n            pass\n        case _:\n            pass\n",
        None, // diagnostic is currently indirect ("unknown name 'v'"); do not freeze it
        "mapping patterns `case {k: v}:` are not in the grammar (deferred, #617)."),

    // --- absent from the grammar, so must not parse (impl-ahead-of-grammar guard) ---
    rej("yield_from",
        "def gen():\n    yield from range(3)\n",
        None,
        "`yield from` delegation is not in the grammar (deferred, #619)."),
    rej("ellipsis_literal",
        "def f():\n    auto x = ...\n    print(\"ok\")\n",
        None,
        "`...` (Ellipsis) is not a Zap literal (deferred, #616)."),
    rej("tuple_numeric_attribute",
        "def f():\n    auto t = (1, 2)\n    print(str(t.0))\n",
        None,
        "`t.0` positional tuple access is not in the grammar (`trailer` is `.` IDENT; tuples are destructured)."),
    rej("async_def",
        "async def f():\n    pass\n",
        None,
        "no `async`/`await`: concurrency is CSP coroutines + `spawn`/`scope` (SPEC §12, won't-do)."),
    rej("global_stmt",
        "def f():\n    global x\n    x = 1\n",
        None,
        "no `global`/`nonlocal`: static lexical scoping, no mutable-module-state rebind keyword (won't-do)."),
    rej("lambda_with_default",
        "def f():\n    fn(int64) -> int64 g = lambda x = 1: x + 1\n    print(str(g(1)))\n",
        None,
        "`lambda` params are bare IDENTs — no defaults/variadics (D-0026); promote to `def` for those."),

    // (The last grammar-ahead-of-impl reject, the sliced assignment target
    // `xs[0:2] = ys`, graduated when lowering gained slice assignment — #1005.
    // It is now exercised end-to-end by the `slice_assign` golden.)
];

/// Grammatical-but-malformed, grammar-absent, and grammar-ahead-of-impl forms
/// all stay rejected by the host front end, with their reason (code/message)
/// pinned where stable. A case that starts being *accepted* fails here — either
/// a real regression (an absent construct now parses) or a tracked gap that got
/// implemented (graduate it to `COVERAGE`).
#[test]
fn rejected_forms_stay_rejected() {
    let mut failures: Vec<String> = Vec::new();
    for r in REJECTS {
        match front_end_first_error(r.src) {
            None => failures.push(format!(
                "[{}] expected the front end to REJECT it, but it was accepted. {}\n    src: {:?}",
                r.label, r.reason, r.src,
            )),
            Some(err) => {
                if let Some(expect) = r.expect {
                    if !err.contains(expect) {
                        failures.push(format!(
                            "[{}] rejected (good) but the diagnostic changed: expected it to contain {:?}, \
                             got {:?}. If this is an intended diagnostic change, update `expect`.",
                            r.label, expect, err,
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "negative/boundary corpus found {} problem(s):\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

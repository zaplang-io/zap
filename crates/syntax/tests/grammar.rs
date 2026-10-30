//! Make `docs/GRAMMAR.ebnf` load-bearing (I#396, epic #374).
//!
//! The spec grammar is hand-maintained and nothing mechanically tied it to the
//! hand-written recursive-descent parser (`crates/syntax/src/parser.rs`), so it
//! could silently drift: a production could be renamed away, a reference could
//! dangle, or a new syntax feature could ship with no grammar entry. This test
//! closes both gaps, with **no compiler toolchain required** (pure lex+parse),
//! so it rides CI's host (no-LLVM) `cargo test` job alongside the other
//! default-member tests.
//!
//! Two independent checks:
//!
//! 1. **EBNF lint** ([`ebnf_*`] tests) — parse `docs/GRAMMAR.ebnf` itself and
//!    assert it is well-formed: every rule is `name ::= … ;`, no rule is
//!    defined twice, every nonterminal referenced on a right-hand side is
//!    defined, every defined nonterminal is reachable from the `module` start
//!    symbol, and the EBNF metasyntax brackets balance. This needs nothing but
//!    the grammar file, so it is the cheapest guard against rot.
//!
//! 2. **Grammar-conformance corpus** ([`grammar_corpus_matches_parser`]) — a
//!    table of syntax snippets, each tagged `Accept`/`Reject` and each citing
//!    the EBNF production it exercises, run through the real parser
//!    ([`zapc_syntax::parser::parse_str`]). An `Accept` snippet must lex and
//!    parse with zero errors; a `Reject` snippet must produce at least one lex
//!    or parse error. Every cited production name is also checked to exist in
//!    the grammar, so the corpus and the EBNF stay wired together: adding a
//!    syntax feature without a matching production (or citing one that was
//!    renamed) fails here.
//!
//! The corpus is kept as inline Rust string literals rather than `.zap` fixture
//! files on purpose: the `Reject` cases are *intentionally malformed* and must
//! not be swept up by `zap fmt --check`, and keeping accept+reject side by side
//! documents each production's boundary in one place. This mirrors how the
//! differential tester (`crates/driver/tests/diff_cpython.rs`, I#375) unit-tests
//! its pure-Rust pieces on every host.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use zapc_syntax::parser::parse_str;

/// Locate `docs/GRAMMAR.ebnf`. This crate lives at `<repo>/crates/syntax`, so
/// the repo root is two levels up (same trick as the golden harness).
fn grammar_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/syntax has a two-level parent")
        .join("docs")
        .join("GRAMMAR.ebnf")
}

fn read_grammar() -> String {
    let path = grammar_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// A tiny EBNF model, built by hand (no regex / no extra deps so the lint runs
// anywhere `cargo test` does).
// ---------------------------------------------------------------------------

/// Replace every `(* … *)` comment with spaces (newlines preserved so byte
/// offsets stay roughly aligned for diagnostics). Comments do not nest in this
/// grammar.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_comment = false;
    while i < bytes.len() {
        if !in_comment && bytes[i] == b'(' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_comment = true;
            out.push_str("  ");
            i += 2;
            continue;
        }
        if in_comment && bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b')' {
            in_comment = false;
            out.push_str("  ");
            i += 2;
            continue;
        }
        let c = bytes[i] as char;
        if in_comment {
            out.push(if c == '\n' { '\n' } else { ' ' });
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Replace every `"…"` literal terminal with spaces, so the only identifiers
/// left in a right-hand side are nonterminals and UPPERCASE token classes.
/// EBNF terminals here are simple (no embedded escapes).
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

/// Is `tok` a lexer token class (all uppercase / `_`, e.g. `NEWLINE`, `IDENT`)
/// rather than a nonterminal?
fn is_token_class(tok: &str) -> bool {
    !tok.is_empty() && tok.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

/// Pull the identifier-shaped tokens out of a right-hand side.
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

struct Grammar {
    /// Rule name -> stripped right-hand side (comments + literals removed).
    rules: BTreeMap<String, String>,
    /// Names defined more than once, in file order of first re-definition.
    duplicates: Vec<String>,
    /// Raw (comments + literals removed) text, for bracket-balance checks.
    stripped: String,
}

fn load_grammar() -> Grammar {
    let raw = read_grammar();
    let stripped = strip_string_literals(&strip_comments(&raw));
    let mut rules = BTreeMap::new();
    let mut duplicates = Vec::new();
    // A rule is `name ::= … ;`; `;` only terminates rules (never appears as a
    // literal in a production — see the EBNF notation header), so splitting on
    // it is sound after comments/strings are gone.
    for chunk in stripped.split(';') {
        let Some((lhs, rhs)) = chunk.split_once("::=") else {
            continue;
        };
        let name = match lhs.split_whitespace().next_back() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if rules.insert(name.clone(), rhs.to_string()).is_some() {
            duplicates.push(name);
        }
    }
    Grammar {
        rules,
        duplicates,
        stripped,
    }
}

/// The start symbol. `module` is the grammar's top production.
const START: &str = "module";

// ---------------------------------------------------------------------------
// EBNF lint tests
// ---------------------------------------------------------------------------

#[test]
fn ebnf_is_well_formed() {
    let g = load_grammar();
    assert!(
        g.rules.len() > 50,
        "expected a substantial grammar, found only {} rules — did GRAMMAR.ebnf parse?",
        g.rules.len()
    );
    for (name, rhs) in &g.rules {
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "rule name {name:?} is not a lowercase nonterminal identifier",
        );
        assert!(
            !rhs.trim().is_empty(),
            "rule {name:?} has an empty right-hand side",
        );
    }
    assert!(
        g.rules.contains_key(START),
        "grammar has no `{START}` start symbol",
    );
}

#[test]
fn ebnf_no_duplicate_rules() {
    let g = load_grammar();
    assert!(
        g.duplicates.is_empty(),
        "GRAMMAR.ebnf defines these nonterminals more than once: {:?}",
        g.duplicates,
    );
}

#[test]
fn ebnf_no_undefined_nonterminals() {
    let g = load_grammar();
    let mut undefined: Vec<(String, String)> = Vec::new();
    for (name, rhs) in &g.rules {
        for tok in identifiers(rhs) {
            if is_token_class(&tok) {
                continue; // lexer token class, not a nonterminal
            }
            if !g.rules.contains_key(&tok) {
                undefined.push((name.clone(), tok));
            }
        }
    }
    undefined.sort();
    undefined.dedup();
    assert!(
        undefined.is_empty(),
        "GRAMMAR.ebnf references undefined nonterminals (rule -> missing): {undefined:?}",
    );
}

#[test]
fn ebnf_all_rules_reachable_from_start() {
    let g = load_grammar();
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![START.to_string()];
    while let Some(name) = stack.pop() {
        if !reached.insert(name.clone()) {
            continue;
        }
        if let Some(rhs) = g.rules.get(&name) {
            for tok in identifiers(rhs) {
                if !is_token_class(&tok) && !reached.contains(&tok) {
                    stack.push(tok);
                }
            }
        }
    }
    let unreachable: Vec<&String> = g.rules.keys().filter(|k| !reached.contains(*k)).collect();
    assert!(
        unreachable.is_empty(),
        "GRAMMAR.ebnf has nonterminals unreachable from `{START}`: {unreachable:?} \
         — wire them into a production or remove them",
    );
}

#[test]
fn ebnf_metasyntax_brackets_balance() {
    // After comments and string literals are removed, every `(` `[` `{` is EBNF
    // metasyntax and must nest and balance.
    let g = load_grammar();
    let mut stack: Vec<char> = Vec::new();
    for c in g.stripped.chars() {
        match c {
            '(' | '[' | '{' => stack.push(c),
            ')' | ']' | '}' => {
                let want = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                assert_eq!(
                    stack.pop(),
                    Some(want),
                    "unbalanced EBNF metasyntax bracket near {c:?}",
                );
            }
            _ => {}
        }
    }
    assert!(
        stack.is_empty(),
        "unclosed EBNF metasyntax brackets: {stack:?}",
    );
}

// ---------------------------------------------------------------------------
// Grammar-conformance corpus
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// Must lex + parse with zero errors.
    Accept,
    /// Must produce at least one lex or parse error.
    Reject,
}
use Expect::*;

/// A corpus case: the EBNF production it exercises, the source, and whether the
/// real parser must accept or reject it.
struct Case {
    production: &'static str,
    src: &'static str,
    expect: Expect,
}

const fn case(production: &'static str, expect: Expect, src: &'static str) -> Case {
    Case {
        production,
        src,
        expect,
    }
}

/// Seeded from the syntax danger zones and recent syntax-affecting design
/// decisions (variadics D-0038, multi-import I#70, with-statement D-0027/D-0069,
/// first-class fn types D-0067, slicing D-0030, tuple destructuring D-0049,
/// match D-0040/D-0046, channels/select D-0031, generators D-0026/I#67,
/// conditional compilation I#139, bitfields I#173, regex D-0010).
#[rustfmt::skip]
const CORPUS: &[Case] = &[
    // ---- functions, params, decorators ----
    case("function", Accept, "def f(int64 a) -> int64:\n    return a\n"),
    case("function", Accept, "def f():\n    return\n"),
    case("param", Accept, "def s(int64... xs) -> int64:\n    return 0\n"),      // variadic, D-0038
    case("param", Accept, "def g(int64 a, str b = \"x\"):\n    return\n"),      // default, D-0038
    case("decorators", Accept, "@cache\ndef f():\n    return\n"),               // D-0036
    case("decorators", Accept, "@retry(3)\ndef f():\n    return\n"),
    case("generics", Accept, "def id[T](T x) -> T:\n    return x\n"),           // D-0017
    case("type_param", Accept, "def f[T: Shape](T x):\n    return\n"),          // bound, I#164
    case("function", Reject, "def (int64 a):\n    return\n"),                   // missing name
    case("block", Reject, "def f()\n    return\n"),                             // missing ':'
    case("block", Reject, "def f():\nreturn\n"),                                // body not indented
    case("function", Reject, "def if():\n    return\n"),                        // keyword as name

    // ---- types ----
    case("named_type", Accept, "list[int64] xs = []\n"),                        // type_args
    case("func_type", Accept, "fn(int64) -> int64 h = g\n"),                    // D-0067
    case("named_type", Accept, "dict[str, int64] d = x\n"),

    // ---- declarations / assignment ----
    case("decl", Accept, "auto x = 1\n"),
    case("decl", Accept, "const int64 N = 10\n"),
    case("decl", Reject, "auto x =\n"),                                         // missing initializer
    case("aug_assign", Accept, "def f():\n    x += 1\n"),
    case("subst_assign", Accept, "def f():\n    s ~= s/a/b/\n"),                // D-0010

    // ---- tuple destructuring (D-0049) ----
    case("bind_target", Accept, "auto a, b = t\n"),
    case("bind_target", Accept, "(a, b) = t\n"),
    case("target_elem", Accept, "auto a, *rest = t\n"),

    // ---- structs / interfaces / enums / annotations ----
    case("struct_def", Accept, "struct Point:\n    int64 x\n    int64 y\n"),
    case("struct_def", Accept, "value struct V:\n    int64 a\n"),
    case("field", Accept, "value struct Flags:\n    int64 a: 3\n    int64 b: 5\n"), // bitfields I#173
    case("interface_def", Accept, "interface Shape:\n    def area(self) -> f64\n"),
    case("enum_def", Accept, "enum Color:\n    Red\n    Green\n"),
    case("enum_member", Accept, "enum Opt:\n    Some(int64 v)\n    None\n"),     // D-0046 + None variant
    case("annotation_def", Accept, "annotation Meta:\n    int64 x\n"),
    case("struct_def", Reject, "struct P\n    int64 x\n"),                      // missing ':'

    // ---- imports (D-0012, D-0028, I#70) ----
    case("import", Accept, "import python \"os\" as os\n"),
    case("import_list", Accept, "import a, b as c\n"),
    case("import_list", Accept, "import (\n    a,\n    b as c,\n)\n"),          // grouped trailing comma

    // ---- expressions ----
    case("lambda", Accept, "auto f = lambda x: x + 1\n"),                       // D-0026
    case("ternary", Accept, "auto y = a if b else c\n"),
    case("comp_for", Accept, "auto xs = [x for x in ys]\n"),                    // comprehension
    case("dict_comp", Accept, "auto d = {k: v for k in ks}\n"),
    case("dict_items", Accept, "auto d = {1: 2, 3: 4}\n"),
    case("subscript", Accept, "auto s = xs[1:2:3]\n"),                          // slice, D-0030
    case("match_regex", Accept, "auto m = x =~ m/foo/\n"),                      // D-0010
    case("lambda", Reject, "auto f = lambda x x\n"),                            // missing ':'
    case("list_display", Reject, "auto xs = [1, 2\n"),                          // unmatched '['
    case("trailer", Reject, "auto y = f(1\n"),                                  // unmatched '('

    // ---- statements: control flow ----
    case("if_stmt", Accept, "if a:\n    return\nelif b:\n    return\nelse:\n    return\n"),
    case("while_stmt", Accept, "while x:\n    return\n"),
    case("do_while_stmt", Accept, "do:\n    return\nwhile x\n"),                // D-0039
    case("for_stmt", Accept, "for x in xs:\n    return\n"),
    case("try_stmt", Accept, "try:\n    return\nexcept E as e:\n    return\nfinally:\n    return\n"),
    case("with_stmt", Accept, "with open(p) as f, lock:\n    return\n"),        // D-0027/D-0069
    case("do_while_stmt", Reject, "do:\n    return\n"),                         // missing 'while'

    // ---- pattern matching (D-0040) ----
    case("match_stmt", Accept, "match x:\n    case 1:\n        return\n    case _:\n        return\n"),
    case("closed_pattern", Accept, "match x:\n    case [a, *rest]:\n        return\n"),
    case("case_clause", Accept, "match x:\n    case n if n > 0:\n        return\n"),

    // ---- concurrency (D-0031, D-0044) ----
    case("send_stmt", Accept, "def f():\n    ch <- 1\n"),
    case("atom", Accept, "def f():\n    auto v = <-ch\n"),                      // channel receive
    case("simple_stmt", Accept, "def f():\n    spawn detached g()\n"),          // spawn, D-0044
    case("scope_stmt", Accept, "scope(on_error=e):\n    spawn f()\n"),

    // ---- generators (I#67) ----
    case("yield_expr", Accept, "def gen():\n    yield 1\n"),

    // ---- conditional compilation (I#139) ----
    case("cfg_item", Accept, "when target.os == .linux:\n    import a\nelse:\n    import b\n"),
];

/// Productions that the EBNF fully specifies and that are *reachable* from the
/// start symbol, but that the hand-written parser does not implement yet. Each
/// snippet is grammatically valid per `docs/GRAMMAR.ebnf`, yet the parser
/// currently errors on it. We pin that gap so it is visible and tracked rather
/// than silent: when the parser gains support, [`known_divergences_still_diverge`]
/// fails, and the maintainer graduates the case into [`CORPUS`] as `Accept`.
///
/// Current entries:
/// * `select`/`select_stmt`/`select_case`/`select_op` (D-0031) — channel send
///   (`ch <- v`) and receive (`<-ch`) parse (see CORPUS), but the `select`
///   multiplexing statement has no dispatch arm in `parse_compound` and no
///   `Stmt::Select` AST node. Tracked as an EBNF-ahead-of-parser gap.
#[rustfmt::skip]
const KNOWN_DIVERGENCES: &[Case] = &[
    case("select_stmt", Reject,
        "select:\n    case v = <-ch:\n        return\n    default:\n        return\n"),
];

#[test]
fn known_divergences_still_diverge() {
    let g = load_grammar();
    for c in KNOWN_DIVERGENCES {
        assert!(
            g.rules.contains_key(c.production),
            "known-divergence production {:?} vanished from GRAMMAR.ebnf — \
             if the production was removed, drop this entry",
            c.production,
        );
        let (_m, parse_errors, lex_errors) = parse_str(c.src);
        let errored = !parse_errors.is_empty() || !lex_errors.is_empty();
        assert!(
            errored,
            "KNOWN DIVERGENCE RESOLVED: the parser now accepts {:?} ({}). \
             Move this case into CORPUS as `Accept` and delete it here.",
            c.src, c.production,
        );
    }
}

#[test]
fn grammar_corpus_matches_parser() {
    let g = load_grammar();
    let mut mismatches: Vec<String> = Vec::new();

    for c in CORPUS {
        // The corpus stays tied to the EBNF: a cited production must exist.
        if !g.rules.contains_key(c.production) {
            mismatches.push(format!(
                "cited production {:?} is not defined in GRAMMAR.ebnf (snippet: {:?})",
                c.production, c.src,
            ));
            continue;
        }
        let (_module, parse_errors, lex_errors) = parse_str(c.src);
        let errored = !parse_errors.is_empty() || !lex_errors.is_empty();
        let ok = match c.expect {
            Accept => !errored,
            Reject => errored,
        };
        if !ok {
            mismatches.push(format!(
                "[{}] expected {:?} but parser {} — src {:?}\n    parse_errors={:?}\n    lex_errors={:?}",
                c.production,
                c.expect,
                if errored { "errored" } else { "accepted" },
                c.src,
                parse_errors.iter().map(|e| &e.message).collect::<Vec<_>>(),
                lex_errors,
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "grammar-conformance corpus disagreed with the parser in {} case(s):\n{}",
        mismatches.len(),
        mismatches.join("\n"),
    );
}

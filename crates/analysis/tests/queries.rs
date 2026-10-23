//! Position → symbol / type / definition query tests for the analysis API
//! (I#228). Positions are computed by locating a token in the source so the
//! cases stay readable; each exercises a representative language construct.

use zapc_analysis::{Analysis, SymbolKind, Ty};

/// Byte offset of the start of the `n`-th (0-based) occurrence of `needle` —
/// the first byte of the token, which any covering span includes.
fn at(src: &str, needle: &str, n: usize) -> u32 {
    let mut from = 0usize;
    for _ in 0..n {
        let i = src[from..].find(needle).expect("occurrence") + from;
        from = i + needle.len();
    }
    (src[from..].find(needle).expect("occurrence") + from) as u32
}

/// The 1-based line a span starts on.
fn def_line(a: &Analysis, span: zapc_syntax::span::Span) -> u32 {
    a.line_col(span.start).0
}

// --- diagnostics ------------------------------------------------------------

#[test]
fn diagnostics_match_check_semantics() {
    let good = Analysis::analyze("def main():\n    print(1)\n");
    assert!(good.ok());
    assert!(good.diagnostics().is_empty());

    let bad = Analysis::analyze("def main():\n    nope()\n");
    assert!(!bad.ok());
    assert!(bad.diagnostics().iter().any(|d| d.phase == "type"));
    // Stable ZAPnnnn codes are carried through (#239).
    assert!(bad
        .diagnostics()
        .iter()
        .all(|d| !d.code.to_string().is_empty()));
}

// --- type_at ----------------------------------------------------------------

#[test]
fn type_at_local_and_literal() {
    let src = "def main():\n    int64 x = 5\n    auto y = x + 1\n";
    let a = Analysis::analyze(src);
    // The use of `x` in `x + 1` (2nd occurrence of "x").
    let pos = at(src, "x", 1);
    assert_eq!(a.type_at(pos), Some(Ty::Int64));
}

// --- symbol_at / definition_of ----------------------------------------------

#[test]
fn local_symbol_and_definition() {
    let src = "def main():\n    int64 x = 5\n    auto y = x + 1\n";
    let a = Analysis::analyze(src);
    let use_pos = at(src, "x", 1);
    let s = a.symbol_at(use_pos).expect("symbol");
    assert_eq!(s.name, "x");
    assert_eq!(s.kind, SymbolKind::Local);
    assert_eq!(s.ty, Ty::Int64);
    let def = a.definition_of(use_pos).expect("def");
    assert_eq!(def_line(&a, def), 2, "def points at the declaration line");
}

#[test]
fn parameter_symbol_and_definition() {
    let src = "def add(int64 a, int64 b) -> int64:\n    return a + b\n";
    let a = Analysis::analyze(src);
    let use_pos = at(src, "a + b", 0); // the `a` in `a + b`
    let s = a.symbol_at(use_pos).expect("symbol");
    assert_eq!(s.name, "a");
    assert_eq!(s.kind, SymbolKind::Param);
    assert_eq!(s.ty, Ty::Int64);
    assert_eq!(def_line(&a, a.definition_of(use_pos).unwrap()), 1);
}

#[test]
fn go_to_definition_of_global_function() {
    let src = "def helper() -> int64:\n    return 1\n\ndef main():\n    auto y = helper()\n";
    let a = Analysis::analyze(src);
    let call = at(src, "helper", 1); // the call site
    let s = a.symbol_at(call).expect("symbol");
    assert_eq!(s.name, "helper");
    assert_eq!(s.kind, SymbolKind::Function);
    assert_eq!(def_line(&a, a.definition_of(call).unwrap()), 1);
}

// --- references -------------------------------------------------------------

#[test]
fn references_of_global_include_all_uses_and_def() {
    let src = "def helper():\n    print(1)\n\ndef main():\n    helper()\n    helper()\n";
    let a = Analysis::analyze(src);
    let refs = a.references(at(src, "helper", 1));
    // Two call sites + the definition.
    assert_eq!(refs.len(), 3, "got {refs:?}");
}

#[test]
fn references_of_local_are_scope_accurate() {
    // Two distinct `x`s in two functions must not be conflated.
    let src =
        "def f():\n    int64 x = 1\n    print(x)\n\ndef g():\n    int64 x = 2\n    print(x)\n";
    let a = Analysis::analyze(src);
    let refs = a.references(at(src, "x", 1)); // f's `x` in `print(x)`
                                              // f's `x`: one use + its definition, both on f's lines (2 and 3).
    assert_eq!(refs.len(), 2, "got {refs:?}");
    for r in &refs {
        let line = a.line_col(r.start).0;
        assert!(line == 2 || line == 3, "ref on unexpected line {line}");
    }
}

// --- structs: fields + methods ----------------------------------------------

#[test]
fn struct_document_symbols_and_field_definition() {
    let src = "struct Point:\n    int64 x\n    int64 y\n\n    def mag(self) -> int64:\n        return self.x + self.y\n";
    let a = Analysis::analyze(src);

    let syms = a.document_symbols();
    let point = syms.iter().find(|s| s.name == "Point").expect("Point");
    assert_eq!(point.kind, SymbolKind::Struct);
    assert!(point
        .children
        .iter()
        .any(|c| c.name == "x" && c.kind == SymbolKind::Field));
    assert!(point
        .children
        .iter()
        .any(|c| c.name == "mag" && c.kind == SymbolKind::Method));

    // `self.x` member resolves to the field definition (line 2).
    let member = at(src, "self.x", 0) + "self.".len() as u32;
    let s = a.symbol_at(member).expect("member symbol");
    assert_eq!(s.name, "x");
    assert_eq!(s.kind, SymbolKind::Field);
    assert_eq!(def_line(&a, a.definition_of(member).unwrap()), 2);
}

#[test]
fn self_receiver_is_a_param_of_the_struct_type() {
    let src = "struct Point:\n    int64 x\n\n    def get(self) -> int64:\n        return self.x\n";
    let a = Analysis::analyze(src);
    let self_use = at(src, "self.x", 0); // on `self`
    let s = a.symbol_at(self_use).expect("self symbol");
    assert_eq!(s.name, "self");
    assert_eq!(s.kind, SymbolKind::Param);
    assert_eq!(s.ty, Ty::Named("Point".to_string()));
}

// --- enums ------------------------------------------------------------------

#[test]
fn enum_document_symbols() {
    let src = "enum Color:\n    RED\n    GREEN\n    BLUE\n";
    let a = Analysis::analyze(src);
    let color = a
        .document_symbols()
        .iter()
        .find(|s| s.name == "Color")
        .expect("Color");
    assert_eq!(color.kind, SymbolKind::Enum);
    let variants: Vec<&str> = color
        .children
        .iter()
        .filter(|c| c.kind == SymbolKind::EnumVariant)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(variants, vec!["RED", "GREEN", "BLUE"]);
}

// --- imports (D-0052 / D-0012) ----------------------------------------------

#[test]
fn python_import_alias_resolves() {
    let src = "import python \"numpy\" as np\n\ndef main():\n    auto a = np.array(1)\n";
    let a = Analysis::analyze(src);
    let use_pos = at(src, "np.array", 0); // on `np`
    let s = a.symbol_at(use_pos).expect("np symbol");
    assert_eq!(s.name, "np");
    assert_eq!(s.kind, SymbolKind::Import);
    assert_eq!(def_line(&a, a.definition_of(use_pos).unwrap()), 1);
}

// --- generics ---------------------------------------------------------------

#[test]
fn generic_function_param_and_call() {
    let src = "def first[T](list[T] xs) -> T:\n    return xs[0]\n\ndef main():\n    auto r = first([1, 2, 3])\n";
    let a = Analysis::analyze(src);
    // The parameter use `xs` inside `xs[0]`.
    let xs_use = at(src, "xs", 1);
    let s = a.symbol_at(xs_use).expect("xs symbol");
    assert_eq!(s.name, "xs");
    assert_eq!(s.kind, SymbolKind::Param);
    assert_eq!(def_line(&a, a.definition_of(xs_use).unwrap()), 1);

    // Go-to-def on the generic call.
    let call = at(src, "first", 1);
    assert_eq!(def_line(&a, a.definition_of(call).expect("first def")), 1);
}

// --- loop / comprehension / match bindings ----------------------------------

#[test]
fn for_loop_binding_resolves() {
    let src = "def main():\n    for i in range(3):\n        print(i)\n";
    let a = Analysis::analyze(src);
    let use_pos = at(src, "(i)", 0) + 1; // the `i` in `print(i)`
    let s = a.symbol_at(use_pos).expect("i symbol");
    assert_eq!(s.name, "i");
    assert_eq!(s.kind, SymbolKind::Local);
    // Definition is the loop variable (line 2).
    assert_eq!(def_line(&a, a.definition_of(use_pos).unwrap()), 2);
}

#[test]
fn match_capture_binding_resolves() {
    let src = "def f(int64 n) -> int64:\n    match n:\n        case x:\n            return x\n";
    let a = Analysis::analyze(src);
    let use_pos = at(src, "x", 1); // `x` in `return x`
    let s = a.symbol_at(use_pos).expect("x symbol");
    assert_eq!(s.name, "x");
    assert_eq!(s.kind, SymbolKind::Local);
    assert_eq!(def_line(&a, a.definition_of(use_pos).unwrap()), 3);
}

// --- position mapping -------------------------------------------------------

#[test]
fn line_index_round_trips() {
    let src = "ab\ncde\n\nf\n";
    let a = Analysis::analyze("def main():\n    print(1)\n");
    let idx = zapc_analysis::LineIndex::new(src);
    // (line, col) is 1-based, column in bytes.
    assert_eq!(idx.line_col(0), (1, 1)); // 'a'
    assert_eq!(idx.line_col(1), (1, 2)); // 'b'
    assert_eq!(idx.line_col(3), (2, 1)); // 'c'
    assert_eq!(idx.offset(2, 1), Some(3));
    assert_eq!(idx.offset(1, 1), Some(0));
    // And the Analysis exposes the same mapping.
    assert_eq!(a.offset_at(1, 1), Some(0));
}

// --- compile-path parity ----------------------------------------------------

#[test]
fn analysis_mode_does_not_change_diagnostics() {
    // The retained-state path must return byte-identical errors+warnings to the
    // ordinary compile path — the proof the normal path is unperturbed.
    let sources = [
        "def main():\n    print(1)\n",
        "def main():\n    nope()\n",
        "def f(int64 a):\n    auto b = a + 1\n    return b\n",
        "struct P:\n    int64 x\n\ndef main():\n    P p = P(x=1)\n    print(p.x)\n",
        "enum Color:\n    RED\n    GREEN\n\ndef main():\n    auto c = Color.RED\n",
        "def first[T](list[T] xs) -> T:\n    return xs[0]\n",
    ];
    for src in sources {
        let (m, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
        if !perrs.is_empty() || !lerrs.is_empty() {
            continue;
        }
        let plain = zapc_sema::analyze(&m);
        let (qe, qw, _facts) = zapc_sema::analyze_queryable(&m);
        assert_eq!(plain.0, qe, "errors differ for:\n{src}");
        assert_eq!(plain.1, qw, "warnings differ for:\n{src}");
    }
}

//! The `@test` harness: an AST→AST transform that turns a parsed test module
//! into a runnable program whose `main` drives every discovered test (D-0054).
//!
//! Generating Zap *AST* (rather than source text) keeps the whole thing inside
//! the real lowering/codegen path and lets `@parametrize` reuse the case
//! expression nodes directly. Each test's outcome is reported on stdout with a
//! small line-based sentinel protocol that [`crate::testing::report`] parses:
//!
//! ```text
//! @@ZT@@ BEGIN <name>
//! ...the test's own stdout...
//! @@ZT@@ MSGSTART
//! ...failure / skip detail (possibly multiline)...
//! @@ZT@@ MSGEND
//! @@ZT@@ RESULT <name> <PASS|FAIL|SKIP>
//! ```
//!
//! Test and hook functions keep their bodies but shed their decorators (lowering
//! rejects function decorators, which are deferred), and any user `main` is
//! dropped so the generated one is the entry point.

use zapc_syntax::ast::*;
use zapc_syntax::span::Span;

/// The sentinel word that marks every protocol line on stdout.
pub const MARKER: &str = "@@ZT@@";

const SP: Span = Span { start: 0, end: 0 };

/// How a single `@test` function is run.
#[derive(Clone, Debug, Default)]
struct Modifiers {
    /// `@skip` with an optional reason — always skipped.
    skip: Option<Option<String>>,
    /// `@skip_if(cond [, reason])` — skipped at runtime when `cond` is truthy.
    skip_if: Option<(Expr, Option<String>)>,
    /// `@expect_raises(ErrType)` — the test must raise (type name is display
    /// only; matching is catch-all until typed exceptions land).
    expect_raises: Option<String>,
    /// `@parametrize(cases=[...])` — one sub-test per case row.
    cases: Option<Vec<Expr>>,
}

/// A discovered test function and how to run it.
struct TestFn {
    name: String,
    modifiers: Modifiers,
}

/// The lifecycle hooks found in a module.
#[derive(Default)]
struct Hooks {
    before: Vec<String>,
    after: Vec<String>,
    before_all: Vec<String>,
    after_all: Vec<String>,
}

/// Build the runnable harness module from a parsed test module. Returns the
/// transformed module and the number of top-level `@test` functions found (0
/// means the file contributes no tests).
pub fn build_harness(module: &Module) -> (Module, usize) {
    let mut tests: Vec<TestFn> = Vec::new();
    let mut hooks = Hooks::default();

    for item in &module.items {
        if let Item::Function(f) = item {
            classify(f, &mut tests, &mut hooks);
        }
    }

    // Carry over every item except the user `main` (replaced) and top-level bare
    // statements (a script's main body, never part of the test harness), with
    // test/hook decorators stripped so the functions lower as plain functions.
    let mut items: Vec<Item> = Vec::new();
    for item in &module.items {
        match item {
            Item::Function(f) if f.name == "main" => {}
            Item::Stmt(_) => {}
            Item::Function(f) => {
                let mut f = f.clone();
                f.decorators.retain(|d| !is_test_decorator(&d.name));
                items.push(Item::Function(f));
            }
            other => items.push(other.clone()),
        }
    }

    let n = tests.len();
    let main = gen_main(&tests, &hooks);
    items.push(Item::Function(main));

    (
        Module {
            items,
            span: module.span,
        },
        n,
    )
}

fn is_test_decorator(name: &str) -> bool {
    zapc_sema::TEST_DECORATORS.contains(&name)
}

/// Sort a function into the test list or a hook list by its decorators.
fn classify(f: &Function, tests: &mut Vec<TestFn>, hooks: &mut Hooks) {
    let mut mods = Modifiers::default();
    let mut is_test = false;
    for d in &f.decorators {
        match d.name.as_str() {
            "test" => is_test = true,
            "before" => hooks.before.push(f.name.clone()),
            "after" => hooks.after.push(f.name.clone()),
            "before_all" => hooks.before_all.push(f.name.clone()),
            "after_all" => hooks.after_all.push(f.name.clone()),
            "skip" => mods.skip = Some(string_arg(d.args.first())),
            "skip_if" => {
                if let Some(a) = d.args.first() {
                    mods.skip_if = Some((a.value.clone(), string_arg(d.args.get(1))));
                }
            }
            "expect_raises" => {
                mods.expect_raises = d.args.first().map(|a| render_expr(&a.value));
            }
            "parametrize" => {
                if let Some(a) = d.args.iter().find(|a| a.name.as_deref() == Some("cases")) {
                    if let Expr::List(rows, _) = &a.value {
                        mods.cases = Some(rows.clone());
                    }
                }
            }
            _ => {}
        }
    }
    if is_test {
        tests.push(TestFn {
            name: f.name.clone(),
            modifiers: mods,
        });
    }
}

/// The string value of a positional literal argument, if it is a string.
fn string_arg(arg: Option<&Arg>) -> Option<String> {
    match arg.map(|a| &a.value) {
        Some(Expr::Str { value, .. }) => Some(value.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// main() generation
// ---------------------------------------------------------------------------

fn gen_main(tests: &[TestFn], hooks: &Hooks) -> Function {
    let mut stmts: Vec<Stmt> = Vec::new();

    for name in &hooks.before_all {
        stmts.push(expr_stmt(call(name, vec![])));
    }
    for t in tests {
        gen_test(t, hooks, &mut stmts);
    }
    for name in &hooks.after_all {
        stmts.push(expr_stmt(call(name, vec![])));
    }

    Function {
        decorators: vec![],
        name: "main".to_string(),
        generics: vec![],
        params: vec![],
        ret: None,
        body: block(stmts),
        span: SP,
    }
}

/// Emit the protocol for one `@test` function (expanding `@parametrize` into
/// one labelled sub-test per case).
fn gen_test(t: &TestFn, hooks: &Hooks, out: &mut Vec<Stmt>) {
    match &t.modifiers.cases {
        Some(rows) => {
            for (i, row) in rows.iter().enumerate() {
                let dname = format!("{}[{i}]", t.name);
                let args = spread_case(row);
                gen_case(&dname, &t.name, args, &t.modifiers, hooks, out);
            }
        }
        None => {
            gen_case(&t.name.clone(), &t.name, vec![], &t.modifiers, hooks, out);
        }
    }
}

/// A `@parametrize` case row is spread across the test's parameters: a tuple or
/// list row contributes one argument per element, anything else a single arg.
fn spread_case(row: &Expr) -> Vec<Expr> {
    match row {
        Expr::Tuple(xs, _) | Expr::List(xs, _) => xs.clone(),
        other => vec![other.clone()],
    }
}

/// Emit `BEGIN`, the skip/run logic, and the `RESULT` line for one sub-test.
fn gen_case(
    dname: &str,
    test_name: &str,
    args: Vec<Expr>,
    mods: &Modifiers,
    hooks: &Hooks,
    out: &mut Vec<Stmt>,
) {
    out.push(marker_print("BEGIN", Some(str_lit(dname))));

    // Unconditional skip.
    if let Some(reason) = &mods.skip {
        if let Some(r) = reason {
            out.extend(msg_block(r));
        }
        out.push(result_skip(dname));
        return;
    }

    let run = run_body(dname, test_name, args, mods, hooks);

    // Conditional skip: `if cond: SKIP else: <run>`.
    if let Some((cond, reason)) = &mods.skip_if {
        let mut skip_branch = Vec::new();
        if let Some(r) = reason {
            skip_branch.extend(msg_block(r));
        }
        skip_branch.push(result_skip(dname));
        out.push(Stmt::If {
            branches: vec![(cond.clone(), block(skip_branch))],
            else_block: Some(block(run)),
            span: SP,
        });
        return;
    }

    out.extend(run);
}

/// The status-tracking try/except/finally plus the `RESULT` line for one run.
fn run_body(
    dname: &str,
    test_name: &str,
    args: Vec<Expr>,
    mods: &Modifiers,
    hooks: &Hooks,
) -> Vec<Stmt> {
    let mut out: Vec<Stmt> = Vec::new();
    let expecting = mods.expect_raises.is_some();

    // `__zt_fail` is the outcome flag (0 = pass, 1 = fail). String comparison is
    // not lowered yet, so an integer flag keeps the generated code inside the
    // supported surface. An expected-raise case starts failed (flipped by the
    // handler); a normal test starts passing (flipped by the handler).
    out.push(let_auto(
        "__zt_fail",
        int_lit(if expecting { 1 } else { 0 }),
    ));

    // try body: before hooks, then the test call.
    let mut body: Vec<Stmt> = hooks
        .before
        .iter()
        .map(|h| expr_stmt(call(h, vec![])))
        .collect();
    body.push(expr_stmt(call(test_name, args)));

    // except handler.
    let handler: Vec<Stmt> = if expecting {
        vec![assign("__zt_fail", int_lit(0))]
    } else {
        vec![
            assign("__zt_fail", int_lit(1)),
            marker_print("MSGSTART", None),
            expr_stmt(call("print", vec![name("__zt_e")])),
            marker_print("MSGEND", None),
        ]
    };

    let finally: Vec<Stmt> = hooks
        .after
        .iter()
        .map(|h| expr_stmt(call(h, vec![])))
        .collect();

    out.push(Stmt::Try {
        body: block(body),
        excepts: vec![ExceptClause {
            ty: None,
            binding: Some("__zt_e".to_string()),
            body: block(handler),
            span: SP,
        }],
        finally: if finally.is_empty() {
            None
        } else {
            Some(block(finally))
        },
        span: SP,
    });

    // An expected raise that never came: explain the failure.
    if let Some(ty) = &mods.expect_raises {
        let detail = format!("expected {ty} to be raised, but the test returned normally");
        out.push(Stmt::If {
            branches: vec![(fail_flag_set(), block(msg_block(&detail)))],
            else_block: None,
            span: SP,
        });
    }

    // RESULT line, chosen by the flag (`if __zt_fail == 0: PASS else: FAIL`).
    out.push(Stmt::If {
        branches: vec![(fail_flag_clear(), block(vec![result_status(dname, "PASS")]))],
        else_block: Some(block(vec![result_status(dname, "FAIL")])),
        span: SP,
    });
    out
}

// ---------------------------------------------------------------------------
// AST construction helpers
// ---------------------------------------------------------------------------

/// `print("@@ZT@@", "RESULT", "<dname>", "<status>")` with a literal status.
fn result_status(dname: &str, status: &str) -> Stmt {
    expr_stmt(call(
        "print",
        vec![
            str_lit(MARKER),
            str_lit("RESULT"),
            str_lit(dname),
            str_lit(status),
        ],
    ))
}

/// The SKIP result line.
fn result_skip(dname: &str) -> Stmt {
    result_status(dname, "SKIP")
}

/// `__zt_fail == 0` (the test passed).
fn fail_flag_clear() -> Expr {
    Expr::Binary {
        op: BinOp::Eq,
        lhs: Box::new(name("__zt_fail")),
        rhs: Box::new(int_lit(0)),
        span: SP,
    }
}

/// `__zt_fail == 1` (the test failed).
fn fail_flag_set() -> Expr {
    Expr::Binary {
        op: BinOp::Eq,
        lhs: Box::new(name("__zt_fail")),
        rhs: Box::new(int_lit(1)),
        span: SP,
    }
}

/// `print("@@ZT@@", "<kind>"[, extra])`.
fn marker_print(kind: &str, extra: Option<Expr>) -> Stmt {
    let mut args = vec![str_lit(MARKER), str_lit(kind)];
    if let Some(e) = extra {
        args.push(e);
    }
    expr_stmt(call("print", args))
}

/// A `MSGSTART` / literal / `MSGEND` trio carrying a fixed detail string.
fn msg_block(detail: &str) -> Vec<Stmt> {
    vec![
        marker_print("MSGSTART", None),
        expr_stmt(call("print", vec![str_lit(detail)])),
        marker_print("MSGEND", None),
    ]
}

fn block(stmts: Vec<Stmt>) -> Block {
    Block { stmts, span: SP }
}

fn expr_stmt(e: Expr) -> Stmt {
    Stmt::Expr(e)
}

fn let_auto(name: &str, value: Expr) -> Stmt {
    Stmt::Let {
        is_const: false,
        ty: None,
        name: name.to_string(),
        value,
        span: SP,
    }
}

fn assign(target: &str, value: Expr) -> Stmt {
    Stmt::Assign {
        target: name(target),
        value,
        span: SP,
    }
}

fn name(n: &str) -> Expr {
    Expr::Name(n.to_string(), SP)
}

fn str_lit(s: &str) -> Expr {
    Expr::Str {
        value: s.to_string(),
        raw: false,
        bytes: false,
        fstring: false,
        span: SP,
    }
}

fn int_lit(n: i64) -> Expr {
    Expr::Int(n.to_string(), SP)
}

fn call(callee: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Box::new(name(callee)),
        args: args
            .into_iter()
            .map(|value| Arg {
                name: None,
                value,
                span: SP,
            })
            .collect(),
        span: SP,
    }
}

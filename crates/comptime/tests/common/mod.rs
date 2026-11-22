//! Shared harness for the comptime evaluator tests.
//!
//! The evaluator has **no `comptime` keyword surface** (that is wired by #1251),
//! so these helpers drive it the way E1 was driven: parse ordinary Zap, then
//! feed the resulting AST (an expression, or a module's functions) to the
//! evaluator host-side.

#![allow(dead_code)]

use zapc_comptime::{CtError, Env, Evaluator, Value};
use zapc_syntax::ast::{Expr, Function, Item, Stmt};
use zapc_syntax::span::Span;

/// Parse a module source into its free functions, asserting it is lex/parse
/// clean.
pub fn parse_fns(src: &str) -> Vec<Function> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    module
        .items
        .into_iter()
        .filter_map(|i| match i {
            Item::Function(f) => Some(f),
            _ => None,
        })
        .collect()
}

/// Parse a single expression by wrapping it as a function's `return` operand.
pub fn parse_expr(src: &str) -> Expr {
    let wrapped = format!("def __harness():\n    return {src}\n");
    let fns = parse_fns(&wrapped);
    for stmt in &fns[0].body.stmts {
        if let Stmt::Return { value: Some(e), .. } = stmt {
            return e.clone();
        }
    }
    panic!("harness produced no return expression for `{src}`");
}

/// Evaluate a standalone expression with an empty environment and no callable
/// functions.
pub fn eval(src: &str) -> Result<Value, CtError> {
    let e = parse_expr(src);
    let mut ev = Evaluator::new();
    ev.eval(&e, &mut Env::new())
}

/// Evaluate an expression expected to succeed, panicking with the error
/// otherwise.
pub fn ok(src: &str) -> Value {
    eval(src).unwrap_or_else(|e| panic!("eval `{src}` failed: {e:?}"))
}

/// Evaluate an expression expected to fail, returning the error.
pub fn err(src: &str) -> CtError {
    match eval(src) {
        Ok(v) => panic!("eval `{src}` unexpectedly succeeded with {v:?}"),
        Err(e) => e,
    }
}

/// Evaluate `expr_src` against the functions declared in `module_src`.
pub fn eval_in(module_src: &str, expr_src: &str) -> Result<Value, CtError> {
    let fns = parse_fns(module_src);
    let e = parse_expr(expr_src);
    let mut ev = Evaluator::with_functions(&fns);
    ev.eval(&e, &mut Env::new())
}

/// Evaluate an expression with the given names pre-bound in the environment
/// (used to seed reflection-record inputs).
pub fn eval_with(expr_src: &str, bindings: Vec<(&str, Value)>) -> Result<Value, CtError> {
    let e = parse_expr(expr_src);
    let mut env = Env::new();
    for (name, value) in bindings {
        env.define(name, value);
    }
    let mut ev = Evaluator::new();
    ev.eval(&e, &mut env)
}

/// A zero-width span, for constructed AST / records in tests.
pub fn sp() -> Span {
    Span::new(0, 0)
}

//! Fast checks without a build (D-0055 surface 2).
//!
//! Parses and type-checks a snippet or file using the front-end only — no LLVM
//! lowering, codegen, or link — and returns structured diagnostics. This is the
//! tight-loop surface: an agent produces Zap, calls `zap_check`, and gets back
//! precise, machine-readable errors in milliseconds.

use super::span_to_line_col;
use crate::json::Json;
use zapc_diag::ErrorCode;

/// The severity of a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One diagnostic, carrying its stable error code, the raw byte span, and a
/// resolved line/column (1-based) so agents can map it back to their source
/// without re-deriving it. The `code` (e.g. `"ZAP4001"`) is the machine-stable
/// identifier; `message` is prose that may change between releases (#239).
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// The producing phase: `lex`, `parse`, `type`, or `warning`.
    pub phase: &'static str,
    /// The stable `ZAPnnnn` error code.
    pub code: ErrorCode,
    pub message: String,
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub col: u32,
}

impl Diagnostic {
    fn to_json(&self) -> Json {
        Json::obj(vec![
            ("severity", Json::str(self.severity.as_str())),
            ("phase", Json::str(self.phase)),
            ("code", Json::str(self.code.to_string())),
            ("message", Json::str(self.message.clone())),
            ("start", Json::int(self.start as i64)),
            ("end", Json::int(self.end as i64)),
            ("line", Json::int(self.line as i64)),
            ("col", Json::int(self.col as i64)),
        ])
    }
}

/// The result of checking one source: its diagnostics and an `ok` flag (true
/// when there are no errors — warnings alone do not fail a check).
#[derive(Clone, Debug)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    pub ok: bool,
}

/// Parse and type-check `src`, collecting diagnostics in source order.
pub fn check_source(src: &str) -> CheckResult {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    let mut diags = Vec::new();

    for e in &lerrs {
        diags.push(make(
            src,
            Severity::Error,
            "lex",
            e.code,
            &e.message,
            e.span,
        ));
    }
    for e in &perrs {
        diags.push(make(
            src,
            Severity::Error,
            "parse",
            e.code,
            &e.message,
            e.span,
        ));
    }

    // Only run the type checker when the program parsed cleanly: sema assumes a
    // well-formed AST, and errors against a broken parse would be noise.
    if lerrs.is_empty() && perrs.is_empty() {
        let (errors, warnings) = zapc_sema::analyze(&module);
        for e in &errors {
            diags.push(make(
                src,
                Severity::Error,
                "type",
                e.code,
                &e.message,
                e.span,
            ));
        }
        for w in &warnings {
            diags.push(make(
                src,
                Severity::Warning,
                "warning",
                w.code,
                &w.message,
                w.span,
            ));
        }
    }

    // Report diagnostics in source position order, errors stable within a span.
    diags.sort_by_key(|d| (d.start, d.end));

    let ok = !diags.iter().any(|d| d.severity == Severity::Error);
    CheckResult {
        diagnostics: diags,
        ok,
    }
}

#[allow(clippy::too_many_arguments)]
fn make(
    src: &str,
    severity: Severity,
    phase: &'static str,
    code: ErrorCode,
    message: &str,
    span: zapc_syntax::span::Span,
) -> Diagnostic {
    let (line, col) = span_to_line_col(src, span.start);
    Diagnostic {
        severity,
        phase,
        code,
        message: message.to_string(),
        start: span.start,
        end: span.end,
        line,
        col,
    }
}

/// Render a [`CheckResult`] as the JSON payload returned by the `zap_check`
/// tool.
pub fn result_to_json(result: &CheckResult) -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(result.ok)),
        (
            "diagnostics",
            Json::Arr(result.diagnostics.iter().map(Diagnostic::to_json).collect()),
        ),
        (
            "counts",
            Json::obj(vec![
                (
                    "errors",
                    Json::int(
                        result
                            .diagnostics
                            .iter()
                            .filter(|d| d.severity == Severity::Error)
                            .count() as i64,
                    ),
                ),
                (
                    "warnings",
                    Json::int(
                        result
                            .diagnostics
                            .iter()
                            .filter(|d| d.severity == Severity::Warning)
                            .count() as i64,
                    ),
                ),
            ]),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_program_is_ok() {
        let r = check_source("def main():\n    print(1)\n");
        assert!(r.ok, "unexpected diagnostics: {:?}", r.diagnostics);
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn parse_error_is_reported_with_location() {
        // A dangling operator — a parse error.
        let r = check_source("def main():\n    x = 1 +\n");
        assert!(!r.ok);
        assert!(r.diagnostics.iter().any(|d| d.phase == "parse"));
        // Line/col are resolved, not left at zero.
        assert!(r.diagnostics[0].line >= 1);
    }

    #[test]
    fn type_error_is_reported() {
        // Calling an undefined function is a type/sema error.
        let r = check_source("def main():\n    nonexistent_fn()\n");
        assert!(!r.ok);
        assert!(
            r.diagnostics.iter().any(|d| d.phase == "type"),
            "expected a type error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn json_payload_has_ok_and_counts() {
        let r = check_source("def main():\n    1 +\n");
        let j = result_to_json(&r).to_string();
        assert!(j.contains("\"ok\":false"));
        assert!(j.contains("\"diagnostics\""));
        assert!(j.contains("\"errors\""));
        // The stable error code is part of the structured payload (#239).
        assert!(j.contains("\"code\":\"ZAP"));
    }
}

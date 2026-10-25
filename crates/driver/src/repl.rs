//! `zap repl` — the interactive REPL (I#159, design/0025-repl.md).
//!
//! Each entry is compiled and executed through the real `AST -> ZIR -> LLVM`
//! backend in one long-lived process via an in-process MCJIT (the
//! [`zapc_codegen::jit`] engine), so REPL code has identical semantics to
//! compiled code. `let`/binding **values** persist across entries in runtime
//! slots; each entry sees prior bindings by re-declaring them from their slot in
//! a generated prelude, and writes new/updated values back on success. A type
//! error or an uncaught runtime exception is reported and the session continues.
//!
//! The engine behind this (the [`Session`] type) is the shared core the doctest
//! runner (#234) and the web playground (#238) are intended to reuse.
//!
//! Executing Zap needs the LLVM backend, so the real REPL is gated on the
//! `codegen` feature; without it `zap repl` explains how to get a build that
//! has it, mirroring `zap run`.

use std::process::ExitCode;

/// Run `zap repl`: read entries from stdin, evaluate each, loop until EOF.
#[cfg(feature = "codegen")]
pub fn serve() -> ExitCode {
    use std::io::Write;

    let mut session = match Session::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zap repl: {e}");
            return ExitCode::from(1);
        }
    };

    let stdin = std::io::stdin();
    let is_tty = std::io::IsTerminal::is_terminal(&stdin);
    if is_tty {
        eprintln!("Zap REPL (I#159). Type `:help` for commands, Ctrl-D to exit.");
    }

    let mut buffer = String::new();
    loop {
        // Prompt on stderr so stdout carries only program/eval output.
        if is_tty {
            eprint!("{}", if buffer.is_empty() { "zap> " } else { "...  " });
            let _ = std::io::stderr().flush();
        }

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("zap repl: input error: {e}");
                break;
            }
        }

        // A meta-command (`:...`) is only recognized at the start of an entry.
        if buffer.is_empty() {
            let t = line.trim();
            match meta_command(t) {
                Some(MetaCommand::Quit) => break,
                Some(MetaCommand::Help) => {
                    print_help();
                    continue;
                }
                Some(MetaCommand::Reset) => {
                    session.reset();
                    if is_tty {
                        eprintln!("session reset");
                    }
                    continue;
                }
                Some(MetaCommand::Type(expr)) => {
                    match session.type_of(&expr) {
                        Ok(ty) => println!("{ty}"),
                        Err(diags) => {
                            for d in diags {
                                eprintln!("{d}");
                            }
                        }
                    }
                    continue;
                }
                None => {}
            }
        }

        buffer.push_str(&line);
        if !entry_complete(&buffer, &line) {
            continue;
        }

        let entry = std::mem::take(&mut buffer);
        if entry.trim().is_empty() {
            continue;
        }
        let outcome = session.eval(&entry);
        for d in &outcome.diagnostics {
            eprintln!("{d}");
        }
        if let Some(err) = &outcome.runtime_error {
            eprintln!("Uncaught exception: {err}");
        }
    }

    // A trailing incomplete entry at EOF is evaluated best-effort.
    if !buffer.trim().is_empty() {
        let outcome = session.eval(&buffer);
        for d in &outcome.diagnostics {
            eprintln!("{d}");
        }
        if let Some(err) = &outcome.runtime_error {
            eprintln!("Uncaught exception: {err}");
        }
    }
    ExitCode::SUCCESS
}

/// Without the `codegen` feature the toolchain cannot execute Zap, so the REPL
/// is unavailable — the same situation as `zap run` on a no-LLVM build.
#[cfg(not(feature = "codegen"))]
pub fn serve() -> ExitCode {
    eprintln!("zap repl: this build has no codegen, so it cannot execute Zap");
    eprintln!("(rebuild with --features zapc/llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}

// ----------------------------------------------------------------------------
// Meta-commands
// ----------------------------------------------------------------------------

#[cfg(feature = "codegen")]
enum MetaCommand {
    Quit,
    Help,
    Reset,
    Type(String),
}

#[cfg(feature = "codegen")]
fn meta_command(line: &str) -> Option<MetaCommand> {
    match line {
        ":quit" | ":q" | ":exit" => Some(MetaCommand::Quit),
        ":help" | ":h" | ":?" => Some(MetaCommand::Help),
        ":reset" => Some(MetaCommand::Reset),
        _ => line
            .strip_prefix(":type ")
            .or_else(|| line.strip_prefix(":t "))
            .map(|e| MetaCommand::Type(e.trim().to_string())),
    }
}

#[cfg(feature = "codegen")]
fn print_help() {
    eprintln!(
        "Commands:\n  \
         :type <expr>  show the inferred type of an expression\n  \
         :reset        clear all session bindings and imports\n  \
         :help         show this help\n  \
         :quit         exit the REPL (also Ctrl-D)"
    );
}

/// Decide whether the accumulated `buffer` forms a complete entry, given the
/// line just read (Python-ish continuation). An entry continues while brackets
/// are unbalanced, and while an indented block is open (a logical line ended in
/// `:`) until a blank line closes it.
#[cfg(feature = "codegen")]
fn entry_complete(buffer: &str, last_line: &str) -> bool {
    if bracket_depth(buffer) > 0 {
        return false;
    }
    if last_line.trim_end().ends_with(':') {
        return false; // just opened a block header
    }
    if has_open_block(buffer) && !last_line.trim().is_empty() {
        return false; // inside a block; wait for a blank line
    }
    true
}

/// Net bracket nesting depth, ignoring string contents crudely (good enough for
/// deciding continuation; a precise lexer-based check is a follow-up).
#[cfg(feature = "codegen")]
fn bracket_depth(s: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut prev = '\0';
    for c in s.chars() {
        match in_str {
            Some(q) => {
                if c == q && prev != '\\' {
                    in_str = None;
                }
            }
            None => match c {
                '"' | '\'' => in_str = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                '#' => {
                    // Line comment: skip to end of line.
                    in_str = None;
                }
                _ => {}
            },
        }
        prev = c;
    }
    depth.max(0)
}

/// True if any logical line in the buffer ends with `:` (an indented block was
/// opened), so the entry needs a blank line to close.
#[cfg(feature = "codegen")]
fn has_open_block(buffer: &str) -> bool {
    buffer
        .lines()
        .any(|l| l.split('#').next().unwrap_or("").trim_end().ends_with(':'))
}

// ----------------------------------------------------------------------------
// The session engine (shared with #234 doctests and #238 playground)
// ----------------------------------------------------------------------------

#[cfg(feature = "codegen")]
pub use engine::{EvalOutcome, Session};

#[cfg(feature = "codegen")]
mod engine {
    use std::collections::HashMap;

    use zapc_analysis::{Analysis, Ty};
    use zapc_codegen::jit::ReplJit;
    use zapc_syntax::ast::{self, Expr, Item, Module, Stmt};
    use zapc_syntax::span::Span;

    /// The `[start, end)` byte range of a span as a slice index.
    fn rng(s: Span) -> std::ops::Range<usize> {
        s.start as usize..s.end as usize
    }

    /// How a binding's value is represented in its runtime slot.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Rep {
        /// An `i64`-class scalar (`int64`, `int32`, `bool`, `char`).
        I64,
        /// A GC pointer (`str`, `bytes`, `list`, `dict`, `set`).
        Ptr,
    }

    impl Rep {
        fn load_fn(self) -> &'static str {
            match self {
                Rep::I64 => "__repl_load_i64",
                Rep::Ptr => "__repl_load_ptr",
            }
        }
        fn store_fn(self) -> &'static str {
            match self {
                Rep::I64 => "__repl_store_i64",
                Rep::Ptr => "__repl_store_ptr",
            }
        }
    }

    /// A persisted session binding.
    #[derive(Clone)]
    struct Binding {
        slot: i64,
        rep: Rep,
        /// The Zap type source used to re-declare the binding in later entries,
        /// e.g. `int64`, `str`, `list[int64]`.
        decl: String,
    }

    /// The outcome of evaluating one entry.
    #[derive(Debug, Default)]
    pub struct EvalOutcome {
        /// Formatted compile-time diagnostics (`error[ZAPnnnn]: ...`). Non-empty
        /// means the entry was rejected and the session is unchanged.
        pub diagnostics: Vec<String>,
        /// The message of an uncaught runtime exception, if one was caught (the
        /// session survives).
        pub runtime_error: Option<String>,
    }

    impl EvalOutcome {
        /// True if the entry compiled and ran without a runtime error.
        pub fn ok(&self) -> bool {
            self.diagnostics.is_empty() && self.runtime_error.is_none()
        }
    }

    /// An interactive Zap session: accumulated imports + persistent bindings,
    /// executed incrementally on one JIT.
    pub struct Session {
        imports: Vec<String>,
        /// Binding names in insertion order (drives the prelude order).
        order: Vec<String>,
        bindings: HashMap<String, Binding>,
        next_slot: i64,
        gen: u32,
        jit: ReplJit,
    }

    impl Session {
        /// Start a session (loads the runtime + creates the JIT engine).
        pub fn new() -> Result<Self, String> {
            Ok(Self {
                imports: Vec::new(),
                order: Vec::new(),
                bindings: HashMap::new(),
                next_slot: 0,
                gen: 0,
                jit: ReplJit::new()?,
            })
        }

        /// Forget all bindings and imports (the `:reset` meta-command). Slots are
        /// not reused, so stale slot values simply become unreferenced.
        pub fn reset(&mut self) {
            self.imports.clear();
            self.order.clear();
            self.bindings.clear();
        }

        /// The inferred type of `expr` in the current session (`:type`).
        pub fn type_of(&self, expr: &str) -> Result<String, Vec<String>> {
            let probe = format!(
                "{}\ndef __repl_type_probe():\n{}    auto __t = ({expr})\n",
                self.imports.join("\n"),
                self.prelude(&[]),
            );
            let analysis = Analysis::analyze(&probe);
            let errs = error_diagnostics(&analysis);
            if !errs.is_empty() {
                return Err(errs);
            }
            let pos = probe.rfind("__t = (").map(|p| (p + 7) as u32);
            let ty = pos.and_then(|p| analysis.type_at(p)).unwrap_or(Ty::Unknown);
            Ok(render_ty(&ty).unwrap_or_else(|| ty.display()))
        }

        /// Evaluate one entry: type-check it in the session context, then (if it
        /// compiles) JIT-run it, persisting new/updated bindings on success.
        pub fn eval(&mut self, src: &str) -> EvalOutcome {
            let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
            if !lerrs.is_empty() || !perrs.is_empty() {
                let mut diagnostics = Vec::new();
                for e in &lerrs {
                    diagnostics.push(format!("error[{}]: {}", e.code, e.message));
                }
                for e in &perrs {
                    diagnostics.push(format!("error[{}]: {}", e.code, e.message));
                }
                return EvalOutcome {
                    diagnostics,
                    runtime_error: None,
                };
            }

            let plan = match self.classify(&module, src) {
                Ok(p) => p,
                Err(d) => {
                    return EvalOutcome {
                        diagnostics: vec![d],
                        runtime_error: None,
                    }
                }
            };

            // Probe: analyze the entry in context to surface diagnostics and
            // recover the types of new bindings / the trailing expression.
            let shadowed: Vec<String> = plan.new_lets.iter().map(|l| l.name.clone()).collect();
            let probe = self.build_probe(&plan, src, &shadowed);
            let analysis = Analysis::analyze(&probe);
            let errs = error_diagnostics(&analysis);
            if !errs.is_empty() {
                return EvalOutcome {
                    diagnostics: errs,
                    runtime_error: None,
                };
            }

            // Resolve each new binding's representation + declaration type. An
            // explicit declared type (including a `fn(...)` closure type) is used
            // verbatim; otherwise the inferred type is read from the probe.
            let mut commits: Vec<(String, Binding)> = Vec::new();
            for (i, l) in plan.new_lets.iter().enumerate() {
                let resolved = match l.ty_span {
                    Some(span) => {
                        let decl = src[rng(span)].trim().to_string();
                        rep_from_src(&decl).map(|rep| (rep, decl))
                    }
                    None => {
                        let needle = format!("__probe_{i} = ");
                        let ty = probe
                            .rfind(&needle)
                            .and_then(|p| analysis.type_at((p + needle.len()) as u32))
                            .unwrap_or(Ty::Unknown);
                        match (rep_of(&ty), render_ty(&ty)) {
                            (Some(rep), Some(decl)) => Some((rep, decl)),
                            _ => None,
                        }
                    }
                };
                // An unsupported binding type (float, user struct, bare closure,
                // ...) is usable within this entry but not persisted (first cut).
                if let Some((rep, decl)) = resolved {
                    let slot = self.alloc_slot();
                    commits.push((l.name.clone(), Binding { slot, rep, decl }));
                }
            }

            // Does the trailing expression carry a printable value?
            let auto_print = plan.trailing_expr.is_some_and(|span| {
                let needle = "__retprobe = (";
                probe
                    .rfind(needle)
                    .and_then(|p| analysis.type_at((p + needle.len()) as u32))
                    .map(printable)
                    .unwrap_or(false)
                    && (span.end as usize) <= src.len()
            });

            // Build and run the executable entry.
            self.gen += 1;
            let entry = format!("__repl_entry_{}", self.gen);
            let exec = self.build_exec(&plan, src, &shadowed, &commits, auto_print, &entry);
            let exec_mod = match parse_ok(&exec) {
                Ok(m) => m,
                Err(d) => {
                    return EvalOutcome {
                        diagnostics: vec![d],
                        runtime_error: None,
                    }
                }
            };
            let outcome = match self.jit.run_entry(&exec_mod, &entry) {
                Ok(o) => o,
                Err(e) => {
                    return EvalOutcome {
                        diagnostics: vec![format!("error: {e}")],
                        runtime_error: None,
                    }
                }
            };

            if outcome.runtime_error.is_some() {
                // A failed entry commits nothing (bindings are not persisted).
                return EvalOutcome {
                    diagnostics: Vec::new(),
                    runtime_error: outcome.runtime_error,
                };
            }

            // Commit: imports + new bindings (shadowing replaces the old slot).
            for imp in &plan.imports {
                if !self.imports.contains(imp) {
                    self.imports.push(imp.clone());
                }
            }
            for (name, binding) in commits {
                if !self.bindings.contains_key(&name) {
                    self.order.push(name.clone());
                }
                self.bindings.insert(name, binding);
            }
            EvalOutcome::default()
        }

        fn alloc_slot(&mut self) -> i64 {
            let s = self.next_slot;
            self.next_slot += 1;
            s
        }

        /// The prelude lines that re-declare prior bindings from their slots,
        /// skipping any name shadowed by a new `let` this entry.
        fn prelude(&self, shadowed: &[String]) -> String {
            let mut out = String::new();
            for name in &self.order {
                if shadowed.iter().any(|s| s == name) {
                    continue;
                }
                let b = &self.bindings[name];
                out.push_str(&format!(
                    "    {} {} = {}({})\n",
                    b.decl,
                    name,
                    b.rep.load_fn(),
                    b.slot
                ));
            }
            out
        }

        /// Build the analyze-only probe: imports + a function whose body is the
        /// prelude, the user's statements, a `__probe_i` line per new binding
        /// (so `type_at` can read each binding's type at a position we control),
        /// and a `__retprobe` line for the trailing expression.
        fn build_probe(&self, plan: &EntryPlan, src: &str, shadowed: &[String]) -> String {
            let mut body = self.prelude(shadowed);
            body.push_str(&indent(&plan.body_src(src)));
            if body.trim().is_empty() {
                body.push_str("    pass\n");
            }
            for (i, l) in plan.new_lets.iter().enumerate() {
                body.push_str(&format!("    auto __probe_{i} = {}\n", l.name));
            }
            if let Some(span) = plan.trailing_expr {
                body.push_str(&format!("    auto __retprobe = ({})\n", &src[rng(span)]));
            }
            format!(
                "{}\ndef __repl_probe():\n{}",
                self.imports_and(&plan.imports),
                body
            )
        }

        /// Build the executable entry: imports + the `__repl_entry_N` thunk whose
        /// body is the prelude (slot loads), the user's statements (with the
        /// trailing expression wrapped in `print` when printable), and the
        /// epilogue (slot stores for every live binding).
        fn build_exec(
            &self,
            plan: &EntryPlan,
            src: &str,
            shadowed: &[String],
            commits: &[(String, Binding)],
            auto_print: bool,
            entry: &str,
        ) -> String {
            let mut body = self.prelude(shadowed);
            let user = if auto_print {
                plan.body_src_autoprint(src)
            } else {
                plan.body_src(src)
            };
            body.push_str(&indent(&user));

            // Epilogue: persist every live binding (prior non-shadowed + new).
            for name in &self.order {
                if shadowed.iter().any(|s| s == name) {
                    continue;
                }
                let b = &self.bindings[name];
                body.push_str(&format!("    {}({}, {})\n", b.rep.store_fn(), b.slot, name));
            }
            for (name, b) in commits {
                body.push_str(&format!("    {}({}, {})\n", b.rep.store_fn(), b.slot, name));
            }
            if body.trim().is_empty() {
                body.push_str("    pass\n");
            }
            format!(
                "{}\ndef {entry}():\n{}",
                self.imports_and(&plan.imports),
                body
            )
        }

        fn imports_and(&self, extra: &[String]) -> String {
            let mut all = self.imports.clone();
            for e in extra {
                if !all.contains(e) {
                    all.push(e.clone());
                }
            }
            all.join("\n")
        }

        /// Split an entry into imports, body statements, new `let` bindings, and
        /// a trailing printable expression. Rejects top-level type/decorator
        /// declarations (not supported in the first cut).
        fn classify(&self, module: &Module, src: &str) -> Result<EntryPlan, String> {
            let mut imports = Vec::new();
            let mut body_spans = Vec::new();
            let mut new_lets = Vec::new();
            let mut trailing_expr = None;

            let n = module.items.len();
            for (idx, item) in module.items.iter().enumerate() {
                match item {
                    Item::Import(i) => imports.push(src[rng(import_span(i))].trim().to_string()),
                    Item::Function(f) => body_spans.push(f.span),
                    Item::Stmt(s) => {
                        if let Stmt::Let { name, ty, .. } = s {
                            new_lets.push(LetInfo {
                                name: name.clone(),
                                ty_span: ty.as_ref().map(type_span),
                            });
                        }
                        let span = stmt_span(s);
                        if idx + 1 == n {
                            if let Stmt::Expr(e) = s {
                                trailing_expr = Some(expr_span(e));
                            }
                        }
                        body_spans.push(span);
                    }
                    _ => {
                        return Err(
                            "error: defining types/structs/enums/decorators at the REPL is not \
                             supported yet (I#159 first cut); bind values with `let` instead"
                                .to_string(),
                        )
                    }
                }
            }
            Ok(EntryPlan {
                imports,
                body_spans,
                new_lets,
                trailing_expr,
            })
        }
    }

    struct LetInfo {
        name: String,
        /// The span of an explicit declared type, if the binding wrote one
        /// (`str x = ...`, `fn(int64) -> int64 f = ...`). `None` for `auto`.
        ty_span: Option<Span>,
    }

    /// The parsed shape of one entry (spans into the entry source).
    struct EntryPlan {
        imports: Vec<String>,
        body_spans: Vec<Span>,
        new_lets: Vec<LetInfo>,
        trailing_expr: Option<Span>,
    }

    impl EntryPlan {
        /// The entry's non-import source (its statements + nested defs), verbatim.
        fn body_src(&self, src: &str) -> String {
            let mut out = String::new();
            for span in &self.body_spans {
                out.push_str(&src[rng(*span)]);
                out.push('\n');
            }
            out
        }

        /// Like [`body_src`], but wraps the trailing bare expression in `print`.
        fn body_src_autoprint(&self, src: &str) -> String {
            let Some(te) = self.trailing_expr else {
                return self.body_src(src);
            };
            let mut out = String::new();
            for span in &self.body_spans {
                if span.start == te.start && span.end == te.end {
                    out.push_str(&format!("print({})", &src[rng(te)]));
                } else {
                    out.push_str(&src[rng(*span)]);
                }
                out.push('\n');
            }
            out
        }
    }

    /// Indent every non-empty line by four spaces (block body under `def`).
    fn indent(s: &str) -> String {
        let mut out = String::new();
        for line in s.lines() {
            if line.trim().is_empty() {
                out.push('\n');
            } else {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// The representation class implied by an explicit declared-type source,
    /// or `None` if the first cut does not persist that type across entries.
    /// Keyed off the leading type token: integer/bool/char classes are `i64`;
    /// `str`/`bytes`/collections and `fn(...)` closure types are pointers.
    fn rep_from_src(decl: &str) -> Option<Rep> {
        let head: String = decl
            .trim()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        match head.as_str() {
            "int64" | "int32" | "int" | "bool" | "char" => Some(Rep::I64),
            "str" | "bytes" | "list" | "dict" | "set" => Some(Rep::Ptr),
            // A function type `fn(...) -> ...` is a GC pointer (closure record).
            "fn" if decl.trim_start().starts_with("fn") => Some(Rep::Ptr),
            _ => None,
        }
    }

    /// The representation class of a binding type, or `None` if the first cut
    /// does not persist it across entries (floats, user types, closures, ...).
    fn rep_of(ty: &Ty) -> Option<Rep> {
        match ty {
            Ty::Int64 | Ty::Int32 | Ty::Bool | Ty::Char | Ty::UntypedInt => Some(Rep::I64),
            Ty::Str | Ty::Bytes | Ty::List(_) | Ty::Dict(_, _) | Ty::Set(_) => Some(Rep::Ptr),
            _ => None,
        }
    }

    /// Render a binding type to Zap source for re-declaration, or `None` if it is
    /// not a persistable first-cut type.
    fn render_ty(ty: &Ty) -> Option<String> {
        Some(match ty {
            Ty::Int64 | Ty::UntypedInt => "int64".to_string(),
            Ty::Int32 => "int32".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Char => "char".to_string(),
            Ty::Str => "str".to_string(),
            Ty::Bytes => "bytes".to_string(),
            Ty::List(t) => format!("list[{}]", render_ty(t)?),
            Ty::Set(t) => format!("set[{}]", render_ty(t)?),
            Ty::Dict(k, v) => format!("dict[{}, {}]", render_ty(k)?, render_ty(v)?),
            _ => return None,
        })
    }

    /// True if a trailing expression's type is worth auto-printing (a value,
    /// not unit).
    fn printable(ty: Ty) -> bool {
        !matches!(ty, Ty::Unit | Ty::Unknown)
    }

    /// Error-severity diagnostics, formatted `error[ZAPnnnn]: message`.
    fn error_diagnostics(analysis: &Analysis) -> Vec<String> {
        analysis
            .diagnostics()
            .iter()
            .filter(|d| matches!(d.severity, zapc_analysis::Severity::Error))
            .map(|d| format!("error[{}]: {}", d.code, d.message))
            .collect()
    }

    fn parse_ok(src: &str) -> Result<Module, String> {
        let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
        if let Some(e) = lerrs.first() {
            return Err(format!(
                "internal REPL error: generated code lex error: {}",
                e.message
            ));
        }
        if let Some(e) = perrs.first() {
            return Err(format!(
                "internal REPL error: generated code parse error: {}",
                e.message
            ));
        }
        Ok(module)
    }

    fn import_span(i: &ast::Import) -> Span {
        match i {
            ast::Import::Module { span, .. } | ast::Import::Python { span, .. } => *span,
        }
    }

    fn type_span(t: &ast::Type) -> Span {
        match t {
            ast::Type::Named { span, .. }
            | ast::Type::Tuple { span, .. }
            | ast::Type::Func { span, .. } => *span,
        }
    }

    fn stmt_span(s: &Stmt) -> Span {
        match s {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::AugAssign { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::DoWhile { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Match { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Raise { span, .. }
            | Stmt::Assert { span, .. }
            | Stmt::Try { span, .. } => *span,
            Stmt::Expr(e) => expr_span(e),
            // Any remaining statement kinds fall back to their inner expression.
            _ => Span::empty_at(0),
        }
    }

    fn expr_span(e: &Expr) -> Span {
        match e {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Imaginary(_, s)
            | Expr::Bool(_, s)
            | Expr::None(s)
            | Expr::Name(_, s)
            | Expr::Tuple(_, s)
            | Expr::List(_, s)
            | Expr::Dict(_, s)
            | Expr::Set(_, s) => *s,
            Expr::Str { span, .. }
            | Expr::ListComp { span, .. }
            | Expr::DictComp { span, .. }
            | Expr::SetComp { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Index { span, .. }
            | Expr::Slice { span, .. }
            | Expr::Attr { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::RegexMatch { span, .. }
            | Expr::RegexSubst { span, .. }
            | Expr::ChanRecv { span, .. }
            | Expr::Yield { span, .. }
            | Expr::IfElse { span, .. } => *span,
        }
    }
}

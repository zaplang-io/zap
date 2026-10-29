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

/// Run `zap repl`: read entries, evaluate each, loop until EOF. An interactive
/// terminal gets line editing + persistent history (I#252, via `rustyline`); a
/// piped/non-interactive stdin (the scripted-stdin test harness, `|`, `<`) uses
/// a plain line reader so input is byte-for-byte predictable.
#[cfg(feature = "codegen")]
pub fn serve() -> ExitCode {
    let mut session = match Session::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zap repl: {e}");
            return ExitCode::from(1);
        }
    };

    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        serve_interactive(&mut session)
    } else {
        serve_piped(&mut session)
    }
}

/// What feeding one input line to the session did.
#[cfg(feature = "codegen")]
enum Feed {
    /// `:quit` / `:exit` — leave the loop.
    Quit,
    /// The entry is incomplete; read another line (continuation).
    More,
    /// The line was handled (a meta-command, or a complete entry evaluated).
    Done,
}

/// Feed one newline-free input `line` into the pending `buffer`, evaluating a
/// complete entry. `interactive` gates terminal-only chatter (the reset notice).
#[cfg(feature = "codegen")]
fn feed_line(session: &mut Session, buffer: &mut String, line: &str, interactive: bool) -> Feed {
    // A meta-command (`:...`) is only recognized at the start of an entry.
    if buffer.is_empty() {
        match meta_command(line.trim()) {
            Some(MetaCommand::Quit) => return Feed::Quit,
            Some(MetaCommand::Help) => {
                print_help();
                return Feed::Done;
            }
            Some(MetaCommand::Reset) => {
                session.reset();
                if interactive {
                    eprintln!("session reset");
                }
                return Feed::Done;
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
                return Feed::Done;
            }
            None => {}
        }
    }

    buffer.push_str(line);
    buffer.push('\n');
    if !entry_complete(buffer, line) {
        return Feed::More;
    }

    let entry = std::mem::take(buffer);
    if entry.trim().is_empty() {
        return Feed::Done;
    }
    eval_and_report(session, &entry);
    Feed::Done
}

/// Evaluate one entry and print its diagnostics / uncaught-exception message.
#[cfg(feature = "codegen")]
fn eval_and_report(session: &mut Session, entry: &str) {
    let outcome = session.eval(entry);
    for d in &outcome.diagnostics {
        eprintln!("{d}");
    }
    if let Some(err) = &outcome.runtime_error {
        eprintln!("Uncaught exception: {err}");
    }
}

/// The non-interactive path: a plain line reader over piped stdin. Keeps the
/// scripted-stdin test harness trivial — no terminal, no editing, no history.
#[cfg(feature = "codegen")]
fn serve_piped(session: &mut Session) -> ExitCode {
    let stdin = std::io::stdin();
    let mut buffer = String::new();
    loop {
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("zap repl: input error: {e}");
                break;
            }
        }
        // `read_line` keeps the newline; `feed_line` works on one logical line.
        let line = line.trim_end_matches(['\n', '\r']);
        if let Feed::Quit = feed_line(session, &mut buffer, line, false) {
            break;
        }
    }
    flush_trailing(session, &buffer);
    ExitCode::SUCCESS
}

/// The interactive path: `rustyline` line editing + persistent history, loaded
/// from and saved to `~/.zap_history`. Ctrl-C abandons the current entry;
/// Ctrl-D (at an empty prompt) exits.
#[cfg(feature = "codegen")]
fn serve_interactive(session: &mut Session) -> ExitCode {
    use rustyline::error::ReadlineError;

    let mut rl = match rustyline::DefaultEditor::new() {
        Ok(rl) => rl,
        // Falling back keeps the REPL usable even if a terminal denies raw mode.
        Err(e) => {
            eprintln!("zap repl: line editor unavailable ({e}); using a plain reader");
            return serve_piped(session);
        }
    };
    let history = history_path();
    if let Some(p) = &history {
        let _ = rl.load_history(p);
    }
    eprintln!("Zap REPL (I#252). Type `:help` for commands, Ctrl-D to exit.");

    let mut buffer = String::new();
    loop {
        let prompt = if buffer.is_empty() { "zap> " } else { "...  " };
        match rl.readline(prompt) {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str());
                if let Feed::Quit = feed_line(session, &mut buffer, &line, true) {
                    break;
                }
            }
            // Ctrl-C abandons whatever entry is being typed.
            Err(ReadlineError::Interrupted) => buffer.clear(),
            // Ctrl-D exits.
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("zap repl: input error: {e}");
                break;
            }
        }
    }
    flush_trailing(session, &buffer);
    if let Some(p) = &history {
        let _ = rl.save_history(p);
    }
    ExitCode::SUCCESS
}

/// Evaluate a trailing incomplete entry left in the buffer at EOF (best-effort).
#[cfg(feature = "codegen")]
fn flush_trailing(session: &mut Session, buffer: &str) {
    if !buffer.trim().is_empty() {
        eval_and_report(session, buffer);
    }
}

/// The persistent-history file (`~/.zap_history`). `None` if `$HOME` is unset.
/// The REPL is POSIX-only (the JIT does not yet support Windows, I#159), so
/// `$HOME` suffices and no extra dependency is needed to find it.
#[cfg(feature = "codegen")]
fn history_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".zap_history"))
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
    if last_line.trim_start().starts_with('@') {
        return false; // a decorator line attaches to the declaration that follows
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

    /// Filler for a generated function body that would otherwise be empty (an
    /// imports-only or pure-declaration entry). Zap has no `pass`; a throwaway
    /// `let` is an always-valid no-op statement.
    const NOOP_STMT: &str = "    auto __repl_nop = 0\n";

    /// How a binding's value is represented in its runtime slot.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Rep {
        /// An `i64`-class scalar (`int64`, `int32`, `bool`, `char`).
        I64,
        /// A `double`-class scalar (`float64`, `float32`), reinterpreted into
        /// the shared 64-bit slot cell (I#252).
        F64,
        /// A GC pointer (`str`, `bytes`, `list`, `dict`, `set`).
        Ptr,
    }

    impl Rep {
        fn load_fn(self) -> &'static str {
            match self {
                Rep::I64 => "__repl_load_i64",
                Rep::F64 => "__repl_load_f64",
                Rep::Ptr => "__repl_load_ptr",
            }
        }
        fn store_fn(self) -> &'static str {
            match self {
                Rep::I64 => "__repl_store_i64",
                Rep::F64 => "__repl_store_f64",
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
        /// Top-level declaration names in insertion order (drives the order the
        /// accumulated `def`/`type`/`enum`/interface decls are re-emitted).
        decl_order: Vec<String>,
        /// Accumulated top-level declarations by name → source (I#252). Re-fed at
        /// module top level in every later entry so a `def`/`type` defined once
        /// is type-checked and callable afterwards; a redefinition replaces the
        /// entry (newest wins) while keeping its position.
        decls: HashMap<String, String>,
        /// User types (struct/enum) that carry a value renderer (`__str__`,
        /// `__repr__`, or `@derive(Repr)`), so a bare instance auto-prints
        /// through `print(...)` rather than the `<TypeName>` placeholder (I#252).
        displayable: std::collections::HashSet<String>,
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
                decl_order: Vec::new(),
                decls: HashMap::new(),
                displayable: std::collections::HashSet::new(),
                order: Vec::new(),
                bindings: HashMap::new(),
                next_slot: 0,
                gen: 0,
                jit: ReplJit::new()?,
            })
        }

        /// Forget all bindings, declarations, and imports (the `:reset`
        /// meta-command). Slots are not reused, so stale slot values simply
        /// become unreferenced.
        pub fn reset(&mut self) {
            self.imports.clear();
            self.decl_order.clear();
            self.decls.clear();
            self.displayable.clear();
            self.order.clear();
            self.bindings.clear();
        }

        /// The inferred type of `expr` in the current session (`:type`).
        pub fn type_of(&self, expr: &str) -> Result<String, Vec<String>> {
            let probe = format!(
                "{}\n{}def __repl_type_probe():\n{}    auto __t = ({expr})\n",
                self.imports.join("\n"),
                self.decls_src(&[]),
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

            // Decide how (and whether) to auto-print the trailing expression.
            let auto_print = match plan.trailing_expr {
                Some(span) if (span.end as usize) <= src.len() => {
                    // The last `__retprobe` occurrence is the bare reference line
                    // (see `build_probe`), whose narrowest covering span is the
                    // binding itself — so this is the whole expression's type.
                    let ty = probe
                        .rfind("__retprobe")
                        .and_then(|p| analysis.type_at(p as u32))
                        .unwrap_or(Ty::Unknown);
                    self.auto_print_for(&ty, &plan.new_decls)
                }
                _ => AutoPrint::None,
            };

            // Build and run the executable entry.
            self.gen += 1;
            let entry = format!("__repl_entry_{}", self.gen);
            let exec = self.build_exec(&plan, src, &shadowed, &commits, &auto_print, &entry);
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

            // Commit: imports + accumulated declarations + new bindings
            // (shadowing / redefinition replaces the old entry).
            for imp in &plan.imports {
                if !self.imports.contains(imp) {
                    self.imports.push(imp.clone());
                }
            }
            for d in &plan.new_decls {
                if !self.decls.contains_key(&d.name) {
                    self.decl_order.push(d.name.clone());
                }
                self.decls.insert(d.name.clone(), d.src.clone());
                match d.displayable {
                    Some(true) => {
                        self.displayable.insert(d.name.clone());
                    }
                    Some(false) => {
                        self.displayable.remove(&d.name);
                    }
                    None => {}
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
                body.push_str(NOOP_STMT);
            }
            for (i, l) in plan.new_lets.iter().enumerate() {
                body.push_str(&format!("    auto __probe_{i} = {}\n", l.name));
            }
            if let Some(span) = plan.trailing_expr {
                // Bind the trailing expression, then reference it on its own: a
                // bare name's recorded span is exactly the binding, so `type_at`
                // reports the whole expression's type (not an inner sub-expr like
                // the receiver of `recv.method()`).
                body.push_str(&format!("    auto __retprobe = ({})\n", &src[rng(span)]));
                body.push_str("    __retprobe\n");
            }
            format!(
                "{}\n{}def __repl_probe():\n{}",
                self.imports_and(&plan.imports),
                self.decls_src(&plan.new_decls),
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
            auto_print: &AutoPrint,
            entry: &str,
        ) -> String {
            let mut body = self.prelude(shadowed);
            let user = match auto_print {
                AutoPrint::Value => plan.body_src_autoprint(src),
                _ => plan.body_src(src),
            };
            body.push_str(&indent(&user));
            if let AutoPrint::Placeholder(name) = auto_print {
                // A user value with no renderer (`__str__`/`__repr__`/
                // `@derive(Repr)`) prints a type-name placeholder (I#252); the
                // trailing expression above still evaluated for its effects.
                body.push_str(&format!("    print(\"<{name}>\")\n"));
            }

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
                body.push_str(NOOP_STMT);
            }
            format!(
                "{}\n{}def {entry}():\n{}",
                self.imports_and(&plan.imports),
                self.decls_src(&plan.new_decls),
                body
            )
        }

        /// The accumulated top-level declarations re-emitted at module scope:
        /// every prior decl whose name is not redefined by this entry, followed
        /// by this entry's new decls (so a redefinition replaces the old one).
        fn decls_src(&self, new: &[DeclInfo]) -> String {
            let mut out = String::new();
            for name in &self.decl_order {
                if new.iter().any(|d| &d.name == name) {
                    continue;
                }
                out.push_str(&self.decls[name]);
                out.push('\n');
            }
            for d in new {
                out.push_str(&d.src);
                out.push('\n');
            }
            out
        }

        /// How a trailing expression of type `ty` should be auto-printed. A user
        /// type (struct/enum) renders through `print(...)` only when it carries a
        /// value renderer; otherwise it falls back to a `<TypeName>` placeholder
        /// (I#252). A general `repr`/Display protocol is a separate ticket.
        fn auto_print_for(&self, ty: &Ty, new_decls: &[DeclInfo]) -> AutoPrint {
            match ty {
                Ty::Unit | Ty::Unknown => AutoPrint::None,
                Ty::Named(name) => {
                    let displayable = self.displayable.contains(name)
                        || new_decls
                            .iter()
                            .any(|d| &d.name == name && d.displayable == Some(true));
                    if displayable {
                        AutoPrint::Value
                    } else {
                        AutoPrint::Placeholder(name.clone())
                    }
                }
                _ => AutoPrint::Value,
            }
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

        /// Split an entry into imports, accumulated top-level declarations
        /// (`def`/`type`/`enum`/interface — persisted across entries, I#252),
        /// body statements, new `let` bindings, and a trailing expression.
        fn classify(&self, module: &Module, src: &str) -> Result<EntryPlan, String> {
            let mut imports = Vec::new();
            let mut new_decls = Vec::new();
            let mut body_spans = Vec::new();
            let mut new_lets = Vec::new();
            let mut trailing_expr = None;

            // Source for a declaration, extended to cover any decorator lines
            // above it so the re-emitted copy keeps them.
            let decl_src = |own: Span, decs: &[ast::Decorator]| -> String {
                let start = decs
                    .iter()
                    .map(|d| d.span.start as usize)
                    .min()
                    .unwrap_or(own.start as usize)
                    .min(own.start as usize);
                src[start..own.end as usize].to_string()
            };

            let n = module.items.len();
            for (idx, item) in module.items.iter().enumerate() {
                match item {
                    Item::Import(i) => imports.push(src[rng(import_span(i))].trim().to_string()),
                    Item::Function(f) => new_decls.push(DeclInfo {
                        name: f.name.clone(),
                        src: decl_src(f.span, &f.decorators),
                        displayable: None,
                    }),
                    Item::Struct(s) => new_decls.push(DeclInfo {
                        name: s.name.clone(),
                        src: decl_src(s.span, &s.decorators),
                        displayable: Some(has_renderer(&s.decorators, &s.methods)),
                    }),
                    Item::Enum(e) => new_decls.push(DeclInfo {
                        name: e.name.clone(),
                        src: decl_src(e.span, &e.decorators),
                        displayable: Some(has_renderer(&e.decorators, &e.methods)),
                    }),
                    Item::Interface(i) => new_decls.push(DeclInfo {
                        name: i.name.clone(),
                        src: decl_src(i.span, &[]),
                        displayable: None,
                    }),
                    Item::Annotation(a) => new_decls.push(DeclInfo {
                        name: a.name.clone(),
                        src: decl_src(a.span, &[]),
                        displayable: None,
                    }),
                    Item::Decorator(d) => new_decls.push(DeclInfo {
                        name: d.name.clone(),
                        src: decl_src(d.span, &[]),
                        displayable: None,
                    }),
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
                    Item::Cfg(_) => {
                        return Err(
                            "error: `when target.*` conditional-compilation blocks are not \
                             supported at the REPL (I#252); use them in a compiled module"
                                .to_string(),
                        )
                    }
                }
            }
            Ok(EntryPlan {
                imports,
                new_decls,
                body_spans,
                new_lets,
                trailing_expr,
            })
        }
    }

    /// How a trailing expression should be rendered automatically.
    enum AutoPrint {
        /// Do not print (unit / unknown).
        None,
        /// Print the value itself via `print(expr)`.
        Value,
        /// Print a `<TypeName>` placeholder for a user value with no renderer.
        Placeholder(String),
    }

    /// A top-level declaration an entry introduced (I#252).
    struct DeclInfo {
        name: String,
        /// The declaration's verbatim source, re-emitted at module scope in each
        /// later entry.
        src: String,
        /// For a struct/enum, whether it carries a value renderer (`Some(bool)`);
        /// `None` for declarations that are never auto-printed.
        displayable: Option<bool>,
    }

    /// True if a struct/enum carries a value renderer: a `__str__`/`__repr__`
    /// method, or a `@derive(Repr)` that synthesizes one (D-0047).
    fn has_renderer(decorators: &[ast::Decorator], methods: &[ast::Function]) -> bool {
        if methods
            .iter()
            .any(|m| m.name == "__str__" || m.name == "__repr__")
        {
            return true;
        }
        decorators.iter().any(|d| {
            d.name == "derive"
                && d.args
                    .iter()
                    .any(|a| matches!(&a.value, Expr::Name(n, _) if n == "Repr"))
        })
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
        /// Top-level declarations this entry introduces (persisted, I#252).
        new_decls: Vec<DeclInfo>,
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
            "float64" | "float32" | "float" => Some(Rep::F64),
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
            Ty::Float64 | Ty::Float32 | Ty::UntypedFloat => Some(Rep::F64),
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
            Ty::Float64 | Ty::UntypedFloat => "float64".to_string(),
            Ty::Float32 => "float32".to_string(),
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
            | Expr::Compare { span, .. }
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

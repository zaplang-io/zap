//! The `zap` command surface (D-0053).
//!
//! Parsing is separated from execution: [`parse`] turns argv into a [`Command`]
//! (pure, unit-tested), and [`execute`] runs it. Verbs that depend on systems
//! not yet built — the package manager (issue #69), the `@test` framework
//! (issue #73), and the `zap mcp` server (issue #72) — are wired as stubs that
//! print a clear message naming the owning ticket, so the CLI surface is stable
//! while those land.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use zapc_pkg::Source;

/// The output form for `zap doc` (I#233).
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub enum DocFormat {
    /// A self-contained static HTML site (the default).
    #[default]
    Html,
    /// A single Markdown page.
    Md,
    /// The serializable doc model as JSON (the #233 contract; #232 behaviour).
    Json,
}

/// A parsed `zap` invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `zap run [file] [-o out] [--target-feature f]` — compile and run a file
    /// or the project.
    Run {
        file: Option<String>,
        out: Option<String>,
        /// Compile-time CPU features for `when target.has(.f)` (I#139).
        features: Vec<String>,
    },
    /// `zap build [file] [-o out] [--target-feature f]` — build a native binary
    /// into `target/`.
    Build {
        file: Option<String>,
        out: Option<String>,
        /// Compile-time CPU features for `when target.has(.f)` (I#139).
        features: Vec<String>,
    },
    /// `zap ir [file] [--target-feature f]` — dump ZIR (diagnostic).
    Ir {
        file: Option<String>,
        /// Compile-time CPU features for `when target.has(.f)` (I#139).
        features: Vec<String>,
    },
    /// `zap expand [file]` — show decorator/`@derive`-generated code (D-0036).
    Expand { file: Option<String> },
    /// `zap fmt [--check] [paths...]` — canonical formatter (D-0037). With no
    /// paths, formats stdin to stdout. `--check` writes nothing and exits
    /// non-zero if any input is not already formatted (CI / fmt-gate mode).
    Fmt { paths: Vec<String>, check: bool },
    /// `zap doc [--format html|md|json] [--out <dir>] [--private] <file>` —
    /// render API documentation (I#232 model, I#233 renderers). `--format html`
    /// (the default) writes a self-contained static site into `--out` (default
    /// `doc`); `--format md` writes `<out>/<module>.md` or prints to stdout;
    /// `--format json` prints the serializable model (the #233 contract) to
    /// stdout. `--private` includes `_`-prefixed declarations (D-0028).
    Doc {
        file: Option<String>,
        format: DocFormat,
        out: Option<String>,
        private: bool,
    },
    /// `zap new <name>` — scaffold a new project directory.
    New { name: String },
    /// `zap init` — scaffold `zap.toml` in the current directory.
    Init,
    /// `zap add <name> (--git <url> --version <v> | --path <dir>)` — add a
    /// dependency to `zap.toml` and re-lock (package manager, issue #69).
    Add { name: String, source: Source },
    /// `zap remove <name>` — drop a dependency and re-lock (issue #69).
    Remove { name: String },
    /// `zap zenv <create|status|path|clean>` — manage the project environment
    /// (issue #69, D-0052).
    Zenv(ZenvCmd),
    /// `zap test [file] [--json] [--watch]` — run the `@test` suite (D-0054).
    /// With `--doc`, instead run the docstring examples (doctests, #234, D-0074)
    /// under the path: every ```` ```zap ```` block in a docstring is executed
    /// through the REPL engine and checked against its `# =>`/`# error:`/
    /// `# raises:` expectations. `--doc` needs codegen (like `zap run`).
    Test {
        file: Option<String>,
        json: bool,
        watch: bool,
        doc: bool,
    },
    /// `zap mcp [--root <dir>]` — the agent MCP server (D-0055, issue #72).
    /// Serves over stdio. `root` is the codebase root for the code-graph and
    /// test surfaces (default: the current directory).
    Mcp { root: Option<String> },
    /// `zap repl` — the interactive REPL (I#159). Reads entries from stdin and
    /// executes each through the in-process JIT, persisting session bindings.
    Repl,
    /// `zap lsp` — the language server (I#157/B2, issue #229). Serves the LSP
    /// over a `Content-Length`-framed JSON-RPC stdio transport: live
    /// diagnostics on open/change/save. No options in #229.
    Lsp,
    /// `zap playground [--host H] [--port N] [--open]` — a local web playground
    /// (I#157/E1, issue #238). Serves an editor page and a `POST /run` endpoint
    /// that executes a snippet server-side through the REPL `Session` engine, in
    /// an isolated worker process per request (see design/0026-playground.md).
    /// Binds `127.0.0.1` by default; needs codegen (like `zap run`).
    Playground { host: String, port: u16, open: bool },
    /// `zap playground-worker` — the hidden per-request worker the playground
    /// spawns: it reads one snippet from stdin, runs it through a fresh
    /// `Session`, and prints a JSON result. Not for direct use.
    PlaygroundWorker,
    /// `zap inspect <pid> [--gc | --profile [--seconds N]]` — signal a running
    /// Zap process to dump diagnostics (ticket #77/#78, D-0058), so operators
    /// need not hand-run `kill`. Default: SIGQUIT (coroutine stacks +
    /// runtime/scheduler summary). `--gc`: SIGUSR1 (force a GC + print memory
    /// stats). `--profile`: SIGUSR2 to start CPU profiling, wait `--seconds N`
    /// (default 10), then SIGUSR2 again to stop and flush the folded profile.
    Inspect {
        pid: String,
        gc: bool,
        profile: bool,
        seconds: u64,
    },
    /// `zap explain <ZAPnnnn>` — print the meaning of a stable error code
    /// (I#157/F1, #239). The code, not the message text, is the stable surface.
    Explain { code: String },
    /// `zap publish [--index <id> | --registry <url>] [--dry-run]` — package the
    /// current project and publish it to a static registry index (I#237,
    /// D-0083, Experimental). Writes to a local/`file://` index; an `https://`
    /// index needs the hosted write API (gated on the org move).
    Publish {
        index: Option<String>,
        registry: Option<String>,
        dry_run: bool,
    },
    /// `zap login [--index <id> | --registry <url>] [--token <t>]` — store a
    /// bearer token for a registry in the out-of-repo credential store (I#237).
    /// Without `--token`, the token is read from stdin (so it is not in argv).
    Login {
        index: Option<String>,
        registry: Option<String>,
        token: Option<String>,
    },
    /// `zap search <query> [--index <id> | --registry <url>]` — list packages
    /// in a local/`file://` static index whose name matches `query` (I#237).
    Search {
        query: String,
        index: Option<String>,
        registry: Option<String>,
    },
    /// `zap info <name> [--index <id> | --registry <url>]` — show a package's
    /// published versions (checksums, yank status, deps) from any static index,
    /// local or remote (I#237).
    Info {
        name: String,
        index: Option<String>,
        registry: Option<String>,
    },
    /// `zap version`.
    Version,
    /// `zap help`.
    Help,
}

/// A `zap zenv` subcommand (issue #69, D-0052).
#[derive(Debug, PartialEq, Eq)]
pub enum ZenvCmd {
    /// `zap zenv create [--no-venv]` — create `zenv/{pkg,venv,activate}`.
    Create { no_venv: bool },
    /// `zap zenv status` — report whether a zenv exists / is active.
    Status,
    /// `zap zenv path` — print the active Zap module search path.
    Path,
    /// `zap zenv clean` — remove the `zenv/` directory.
    Clean,
}

/// Entry point used by the `zap` binary.
pub fn run(args: &[String]) -> ExitCode {
    match parse(args) {
        Ok(cmd) => execute(cmd),
        Err(msg) => {
            eprintln!("zap: {msg}");
            eprintln!("run `zap help` for usage");
            ExitCode::from(2)
        }
    }
}

/// Turn argv (without the program name) into a [`Command`].
///
/// Returns `Err(message)` for a usage error (missing verb, unknown verb,
/// missing argument), which the caller reports with exit code 2.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some(verb) = args.first().map(String::as_str) else {
        // No subcommand: show help, but signal misuse via the exit code.
        return Err("no subcommand given".to_string());
    };
    let rest = &args[1..];
    match verb {
        "run" => {
            let CompileArgs {
                file,
                out,
                features,
            } = parse_compile(rest)?;
            Ok(Command::Run {
                file,
                out,
                features,
            })
        }
        "build" => {
            let CompileArgs {
                file,
                out,
                features,
            } = parse_compile(rest)?;
            Ok(Command::Build {
                file,
                out,
                features,
            })
        }
        "ir" => {
            let CompileArgs { file, features, .. } = parse_compile(rest)?;
            Ok(Command::Ir { file, features })
        }
        "expand" => Ok(Command::Expand {
            file: first_positional(rest),
        }),
        "fmt" => Ok(Command::Fmt {
            paths: positionals(rest),
            check: rest.iter().any(|a| a == "--check"),
        }),
        "doc" => parse_doc(rest),
        "new" => match first_positional(rest) {
            Some(name) => Ok(Command::New { name }),
            None => Err("`zap new` needs a project name: `zap new <name>`".to_string()),
        },
        "init" => Ok(Command::Init),
        "add" => parse_add(rest),
        "remove" => match first_positional(rest) {
            Some(name) => Ok(Command::Remove { name }),
            None => Err("`zap remove` needs a dependency name: `zap remove <name>`".to_string()),
        },
        "zenv" => parse_zenv(rest),
        "test" => {
            let mut file = None;
            let mut json = false;
            let mut watch = false;
            let mut doc = false;
            for a in rest {
                match a.as_str() {
                    "--json" => json = true,
                    "--watch" => watch = true,
                    "--doc" => doc = true,
                    flag if flag.starts_with('-') => {
                        return Err(format!("unknown flag `{flag}`"));
                    }
                    f => {
                        if file.is_some() {
                            return Err(format!("unexpected extra argument `{f}`"));
                        }
                        file = Some(f.to_string());
                    }
                }
            }
            if doc && watch {
                return Err("`zap test --doc` does not support `--watch` yet".to_string());
            }
            Ok(Command::Test {
                file,
                json,
                watch,
                doc,
            })
        }
        "mcp" => parse_mcp(rest),
        "repl" => parse_repl(rest),
        "lsp" => parse_lsp(rest),
        "playground" => parse_playground(rest),
        "playground-worker" => parse_playground_worker(rest),
        "inspect" => parse_inspect(rest),
        "explain" => match first_positional(rest) {
            Some(code) => Ok(Command::Explain { code }),
            None => Err("`zap explain` needs an error code: `zap explain ZAP4001`".to_string()),
        },
        "publish" => parse_publish(rest),
        "login" => parse_login(rest),
        "search" => parse_search(rest),
        "info" => parse_info(rest),
        "version" | "--version" | "-V" => Ok(Command::Version),
        "help" | "--help" | "-h" => Ok(Command::Help),
        other => Err(format!("unknown subcommand `{other}`")),
    }
}

/// The parsed arguments shared by `run`, `build`, and `ir`.
struct CompileArgs {
    file: Option<String>,
    out: Option<String>,
    /// `--target-feature` values for `when target.has(.f)` (I#139).
    features: Vec<String>,
}

/// Parse the `[-o <out>] [--target-feature f]... [file]` form shared by `run`,
/// `build`, and `ir`.
fn parse_compile(args: &[String]) -> Result<CompileArgs, String> {
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    let mut features: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                out = Some(
                    args.get(i + 1)
                        .cloned()
                        .ok_or_else(|| "`-o` needs an output path".to_string())?,
                );
                i += 2;
            }
            // `--target-feature <name>` (repeatable): a compile-time CPU feature
            // assumed present in this build, consulted by `when target.has(.f)`
            // (I#139, D-0079). Distinct from runtime detection.
            "--target-feature" => {
                let f = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "`--target-feature` needs a feature name".to_string())?;
                features.push(f);
                i += 2;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown flag `{flag}`"));
            }
            f => {
                if file.is_some() {
                    return Err(format!("unexpected extra argument `{f}`"));
                }
                file = Some(f.to_string());
                i += 1;
            }
        }
    }
    Ok(CompileArgs {
        file,
        out,
        features,
    })
}

/// Parse `mcp [--root <dir>]`. Transport is stdio (D-0055); a socket transport
/// is a documented follow-up, so `--socket` is rejected with a clear message
/// rather than silently ignored.
fn parse_mcp(args: &[String]) -> Result<Command, String> {
    let mut root: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                root = Some(
                    args.get(i + 1)
                        .cloned()
                        .ok_or_else(|| "`--root` needs a directory".to_string())?,
                );
                i += 2;
            }
            "--stdio" => i += 1, // the default and only transport today
            "--socket" => {
                return Err(
                    "`zap mcp` currently serves over stdio only; a socket transport is a \
                     follow-up (D-0055)"
                        .to_string(),
                );
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(Command::Mcp { root })
}

/// Parse `repl` (I#159). The REPL takes no options yet; it reads entries from
/// stdin and serves over the terminal.
fn parse_repl(args: &[String]) -> Result<Command, String> {
    if let Some(a) = args.first() {
        return Err(format!("unexpected argument `{a}`"));
    }
    Ok(Command::Repl)
}

/// Parse `lsp`. The language server (#229) takes no options yet; the transport
/// is `Content-Length`-framed JSON-RPC over stdio, so `--stdio` is accepted as a
/// no-op and any other flag is rejected with a clear message.
fn parse_lsp(args: &[String]) -> Result<Command, String> {
    for a in args {
        match a.as_str() {
            "--stdio" => {} // the default and only transport today
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(Command::Lsp)
}

/// Parse `playground [--host H] [--port N] [--open]` (I#238). Defaults to
/// `127.0.0.1:8238`, loopback only; a non-loopback host is accepted but the
/// server warns (it runs untrusted code).
fn parse_playground(args: &[String]) -> Result<Command, String> {
    let mut host = "127.0.0.1".to_string();
    let mut port: u16 = 8238;
    let mut open = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                i += 1;
                host = args.get(i).ok_or("`--host` needs a value")?.clone();
            }
            "--port" => {
                i += 1;
                let v = args.get(i).ok_or("`--port` needs a value")?;
                port = v.parse().map_err(|_| format!("invalid port `{v}`"))?;
            }
            "--open" => open = true,
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    Ok(Command::Playground { host, port, open })
}

/// Parse `playground-worker` (hidden). Takes no arguments; it is driven by the
/// playground server over stdin/stdout.
fn parse_playground_worker(args: &[String]) -> Result<Command, String> {
    if let Some(a) = args.first() {
        return Err(format!("unexpected argument `{a}`"));
    }
    Ok(Command::PlaygroundWorker)
}

/// Parse `zap doc [--format html|md|json] [--out <dir>] [--private] <file>`
/// (I#232 model, I#233 renderers). Exactly one positional (the source file).
/// `--format` selects the output form (default `html`); `--json` is kept as a
/// back-compat alias for `--format json`; `--out <dir>` sets the output
/// directory (HTML/Markdown); `--private` includes `_`-prefixed declarations.
fn parse_doc(args: &[String]) -> Result<Command, String> {
    let mut file: Option<String> = None;
    let mut format: Option<DocFormat> = None;
    let mut out: Option<String> = None;
    let mut private = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => format = Some(DocFormat::Json),
            "--private" => private = true,
            "--format" | "-f" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "`--format` needs a value (html|md|json)".to_string())?;
                format = Some(match v.as_str() {
                    "html" => DocFormat::Html,
                    "md" | "markdown" => DocFormat::Md,
                    "json" => DocFormat::Json,
                    other => return Err(format!("unknown doc format `{other}` (html|md|json)")),
                });
                i += 1;
            }
            "--out" | "-o" => {
                out = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "`--out` needs a directory".to_string())?
                        .clone(),
                );
                i += 1;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            f => {
                if file.is_some() {
                    return Err(format!("unexpected extra argument `{f}`"));
                }
                file = Some(f.to_string());
            }
        }
        i += 1;
    }
    Ok(Command::Doc {
        file,
        format: format.unwrap_or_default(),
        out,
        private,
    })
}

/// Parse `zap inspect <pid> [--gc | --profile [--seconds N]]` (ticket #77/#78,
/// D-0058). Exactly one positional (the pid); `--gc`/`-g` selects the SIGUSR1
/// (GC-stats) action, `--profile` the SIGUSR2 (CPU-profile toggle) action, over
/// the default SIGQUIT (stack/runtime dump). `--gc` and `--profile` are mutually
/// exclusive. `--seconds`/`-s N` (profile only) sets the profiling window.
fn parse_inspect(args: &[String]) -> Result<Command, String> {
    let mut pid: Option<String> = None;
    let mut gc = false;
    let mut profile = false;
    let mut seconds: u64 = 10;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--gc" | "-g" => gc = true,
            "--profile" | "-p" => profile = true,
            "--seconds" | "-s" => {
                let v = it
                    .next()
                    .ok_or_else(|| "`--seconds` needs a value (e.g. `--seconds 10`)".to_string())?;
                seconds = v
                    .parse::<u64>()
                    .map_err(|_| format!("`--seconds` expects a positive integer, got `{v}`"))?;
                if seconds == 0 {
                    return Err("`--seconds` must be at least 1".to_string());
                }
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => {
                if pid.is_some() {
                    return Err(format!("unexpected extra argument `{other}`"));
                }
                pid = Some(other.to_string());
            }
        }
    }
    if gc && profile {
        return Err("`--gc` and `--profile` are mutually exclusive".to_string());
    }
    match pid {
        Some(pid) => Ok(Command::Inspect {
            pid,
            gc,
            profile,
            seconds,
        }),
        None => Err(
            "`zap inspect` needs a process id: `zap inspect <pid> [--gc | --profile [--seconds N]]`"
                .to_string(),
        ),
    }
}

/// The first non-flag argument, if any.
fn first_positional(args: &[String]) -> Option<String> {
    args.iter().find(|a| !a.starts_with('-')).cloned()
}

/// All non-flag arguments.
fn positionals(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect()
}

/// `zap fmt` (D-0037). With no paths, formats stdin to stdout. With paths,
/// rewrites `.zap` files in place (directories are walked recursively). `--check`
/// writes nothing and exits non-zero if any input is not already formatted.
fn cmd_fmt(paths: Vec<String>, check: bool) -> ExitCode {
    use std::io::Read;

    if paths.is_empty() {
        let mut src = String::new();
        if std::io::stdin().read_to_string(&mut src).is_err() {
            eprintln!("zap fmt: failed to read stdin");
            return ExitCode::from(2);
        }
        return match zapc_fmt::format_source(&src) {
            Ok(out) if check => {
                if out == src {
                    ExitCode::SUCCESS
                } else {
                    eprintln!("zap fmt: stdin is not formatted");
                    ExitCode::from(1)
                }
            }
            Ok(out) => {
                print!("{out}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("zap fmt: {e}");
                ExitCode::from(2)
            }
        };
    }

    let mut files = Vec::new();
    for p in &paths {
        collect_zap_files(Path::new(p), &mut files);
    }

    let mut unformatted = Vec::new();
    let mut had_error = false;
    for file in &files {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("zap fmt: {}: {e}", file.display());
                had_error = true;
                continue;
            }
        };
        match zapc_fmt::format_source(&src) {
            Ok(out) => {
                if out == src {
                    continue;
                }
                if check {
                    unformatted.push(file.display().to_string());
                } else if let Err(e) = fs::write(file, &out) {
                    eprintln!("zap fmt: {}: {e}", file.display());
                    had_error = true;
                }
            }
            Err(e) => {
                eprintln!("zap fmt: {}: {e}", file.display());
                had_error = true;
            }
        }
    }

    if had_error {
        return ExitCode::from(2);
    }
    if check && !unformatted.is_empty() {
        eprintln!("the following files are not formatted (run `zap fmt`):");
        for f in &unformatted {
            eprintln!("  {f}");
        }
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// `zap doc` (I#232 model, I#233 renderers). Extracts the doc model from a
/// source file and renders it: `Html` writes a self-contained static site into
/// the output directory (default `doc`), `Md` writes a Markdown page there (or
/// to stdout when no `--out`), and `Json` prints the serializable model to
/// stdout (the #233 contract). `--private` includes `_`-prefixed (private,
/// D-0028) declarations.
fn cmd_doc(
    file: Option<String>,
    format: DocFormat,
    out: Option<String>,
    private: bool,
) -> ExitCode {
    let Some(file) = file else {
        eprintln!("zap doc: needs a source file: `zap doc <file.zap>`");
        return ExitCode::from(2);
    };
    let src = match fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zap doc: {file}: {e}");
            return ExitCode::from(2);
        }
    };
    // The module's logical name is its file stem (D-0028: one file = one module).
    let stem = Path::new(&file).file_stem().and_then(|s| s.to_str());
    let module_name = stem.unwrap_or("module");
    let opts = zapc_doc::DocOptions {
        include_private: private,
    };
    let model = zapc_doc::DocModel::from_source(&src, Some(module_name), &opts);

    match format {
        DocFormat::Json => {
            println!("{}", model.to_json_string());
            ExitCode::SUCCESS
        }
        DocFormat::Md => {
            let md = model.to_markdown();
            match out {
                Some(dir) => write_doc_file(&dir, &format!("{module_name}.md"), &md),
                None => {
                    print!("{md}");
                    ExitCode::SUCCESS
                }
            }
        }
        DocFormat::Html => {
            let dir = out.unwrap_or_else(|| "doc".to_string());
            write_doc_file(&dir, "index.html", &model.to_html())
        }
    }
}

/// Write one rendered doc file into `dir`, creating the directory. Reports the
/// path written to stderr.
fn write_doc_file(dir: &str, name: &str, contents: &str) -> ExitCode {
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("zap doc: {dir}: {e}");
        return ExitCode::from(2);
    }
    let path = Path::new(dir).join(name);
    match fs::write(&path, contents) {
        Ok(()) => {
            eprintln!("zap doc: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap doc: {}: {e}", path.display());
            ExitCode::from(2)
        }
    }
}

/// Collect `.zap` files under `path` (a single file, or a directory walked
/// recursively) into `out`.
fn collect_zap_files(path: &Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = match fs::read_dir(path) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
            Err(_) => return,
        };
        entries.sort();
        for e in entries {
            collect_zap_files(&e, out);
        }
    } else if path.extension().map(|x| x == "zap").unwrap_or(false) {
        out.push(path.to_path_buf());
    }
}

/// Parse `zap add <name>` with a `--git`/`--path`/`--registry` source.
///
/// `zap add json --registry jsonlib --version 1.4.0 [--index <id>]` adds a
/// registry dependency (D-0027 §3.1); `--version` is optional for a registry
/// source (it is a minimum).
fn parse_add(args: &[String]) -> Result<Command, String> {
    let mut name: Option<String> = None;
    let mut git: Option<String> = None;
    let mut path: Option<String> = None;
    let mut registry: Option<String> = None;
    let mut index: Option<String> = None;
    let mut version: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let take = |i: usize, what: &str| {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("`{what}` needs a value"))
        };
        match args[i].as_str() {
            "--git" => {
                git = Some(take(i, "--git")?);
                i += 2;
            }
            "--path" => {
                path = Some(take(i, "--path")?);
                i += 2;
            }
            "--registry" => {
                registry = Some(take(i, "--registry")?);
                i += 2;
            }
            "--index" => {
                index = Some(take(i, "--index")?);
                i += 2;
            }
            "--version" | "-v" => {
                version = Some(take(i, "--version")?);
                i += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            pos => {
                if name.is_some() {
                    return Err(format!("unexpected extra argument `{pos}`"));
                }
                name = Some(pos.to_string());
                i += 1;
            }
        }
    }
    let name = name.ok_or_else(|| {
        "`zap add` needs a dependency name: `zap add <name> --path <dir>`".to_string()
    })?;
    if index.is_some() && registry.is_none() {
        return Err("`--index` is only valid with `--registry`".to_string());
    }
    let sources = [git.is_some(), path.is_some(), registry.is_some()];
    if sources.iter().filter(|p| **p).count() > 1 {
        return Err("give exactly one of `--git`, `--path`, or `--registry`".to_string());
    }
    if let Some(url) = git {
        let version = version.ok_or_else(|| {
            "`--git` needs `--version <semver>` (the minimum version)".to_string()
        })?;
        return Ok(Command::Add {
            name,
            source: Source::Git { url, version },
        });
    }
    if let Some(dir) = path {
        return Ok(Command::Add {
            name,
            source: Source::Path { dir },
        });
    }
    if let Some(reg) = registry {
        return Ok(Command::Add {
            name,
            source: Source::Registry {
                name: reg,
                version,
                index,
            },
        });
    }
    Err(
        "`zap add <name>` needs a source: `--git <url> --version <v>`, `--path <dir>`, or \
         `--registry <name> [--version <v>] [--index <id>]`"
            .to_string(),
    )
}

/// Shared `--index <id>` / `--registry <url>` target flags for the registry
/// verbs. At most one of the two may be given (they are two ways to name the
/// same thing).
#[derive(Default)]
struct RegistryTarget {
    index: Option<String>,
    registry: Option<String>,
}

impl RegistryTarget {
    fn check(self) -> Result<RegistryTarget, String> {
        if self.index.is_some() && self.registry.is_some() {
            return Err("give at most one of `--index <id>` or `--registry <url>`".to_string());
        }
        Ok(self)
    }
}

/// Parse `zap publish [--index <id> | --registry <url>] [--dry-run]` (I#237).
fn parse_publish(args: &[String]) -> Result<Command, String> {
    let mut t = RegistryTarget::default();
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                t.index = Some(take_value(args, i, "--index")?);
                i += 2;
            }
            "--registry" => {
                t.registry = Some(take_value(args, i, "--registry")?);
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    let t = t.check()?;
    Ok(Command::Publish {
        index: t.index,
        registry: t.registry,
        dry_run,
    })
}

/// Parse `zap login [--index <id> | --registry <url>] [--token <t>]` (I#237).
fn parse_login(args: &[String]) -> Result<Command, String> {
    let mut t = RegistryTarget::default();
    let mut token = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                t.index = Some(take_value(args, i, "--index")?);
                i += 2;
            }
            "--registry" => {
                t.registry = Some(take_value(args, i, "--registry")?);
                i += 2;
            }
            "--token" => {
                token = Some(take_value(args, i, "--token")?);
                i += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    let t = t.check()?;
    Ok(Command::Login {
        index: t.index,
        registry: t.registry,
        token,
    })
}

/// Parse `zap search <query> [--index <id> | --registry <url>]` (I#237).
fn parse_search(args: &[String]) -> Result<Command, String> {
    let mut t = RegistryTarget::default();
    let mut query: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                t.index = Some(take_value(args, i, "--index")?);
                i += 2;
            }
            "--registry" => {
                t.registry = Some(take_value(args, i, "--registry")?);
                i += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => {
                if query.is_some() {
                    return Err(format!("unexpected extra argument `{other}`"));
                }
                query = Some(other.to_string());
                i += 1;
            }
        }
    }
    let t = t.check()?;
    let query =
        query.ok_or_else(|| "`zap search` needs a query: `zap search <query>`".to_string())?;
    Ok(Command::Search {
        query,
        index: t.index,
        registry: t.registry,
    })
}

/// Parse `zap info <name> [--index <id> | --registry <url>]` (I#237).
fn parse_info(args: &[String]) -> Result<Command, String> {
    let mut t = RegistryTarget::default();
    let mut name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                t.index = Some(take_value(args, i, "--index")?);
                i += 2;
            }
            "--registry" => {
                t.registry = Some(take_value(args, i, "--registry")?);
                i += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => {
                if name.is_some() {
                    return Err(format!("unexpected extra argument `{other}`"));
                }
                name = Some(other.to_string());
                i += 1;
            }
        }
    }
    let t = t.check()?;
    let name =
        name.ok_or_else(|| "`zap info` needs a package name: `zap info <name>`".to_string())?;
    Ok(Command::Info {
        name,
        index: t.index,
        registry: t.registry,
    })
}

/// Fetch the value following the flag at `args[i]`, or a clear error.
fn take_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("`{flag}` needs a value"))
}

/// Parse `zap zenv [create [--no-venv] | status | path | clean]`.
/// Bare `zap zenv` prints status (read-only, the safe default).
fn parse_zenv(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None | Some("status") => Ok(Command::Zenv(ZenvCmd::Status)),
        Some("create") => {
            let mut no_venv = false;
            for a in &args[1..] {
                match a.as_str() {
                    "--no-venv" => no_venv = true,
                    other => {
                        return Err(format!("unexpected argument `{other}` for `zenv create`"))
                    }
                }
            }
            Ok(Command::Zenv(ZenvCmd::Create { no_venv }))
        }
        Some("path") => Ok(Command::Zenv(ZenvCmd::Path)),
        Some("clean") => Ok(Command::Zenv(ZenvCmd::Clean)),
        Some(other) => Err(format!(
            "unknown `zap zenv` subcommand `{other}` (use create|status|path|clean)"
        )),
    }
}

/// Run a parsed command.
pub fn execute(cmd: Command) -> ExitCode {
    match cmd {
        Command::Run {
            file,
            out,
            features,
        } => match resolve_entry(file) {
            Ok(f) => crate::compile_for(&f, out, true, &features),
            Err(code) => code,
        },
        Command::Build {
            file,
            out,
            features,
        } => cmd_build(file, out, &features),
        Command::Ir { file, features } => match resolve_entry(file) {
            Ok(f) => crate::emit_ir_for(&f, &features),
            Err(code) => code,
        },
        Command::Expand { file } => match resolve_entry(file) {
            Ok(f) => crate::emit_expand(&f),
            Err(code) => code,
        },
        Command::Fmt { paths, check } => cmd_fmt(paths, check),
        Command::Doc {
            file,
            format,
            out,
            private,
        } => cmd_doc(file, format, out, private),
        Command::New { name } => cmd_new(&name),
        Command::Init => cmd_init(),
        Command::Add { name, source } => cmd_add(&name, source),
        Command::Remove { name } => cmd_remove(&name),
        Command::Zenv(sub) => cmd_zenv(sub),
        Command::Test {
            file,
            json,
            watch,
            doc,
        } => cmd_test(file, json, watch, doc),
        Command::Mcp { root } => crate::mcp::serve(root),
        Command::Repl => crate::repl::serve(),
        Command::Lsp => crate::lsp::serve(),
        Command::Playground { host, port, open } => crate::playground::serve(host, port, open),
        Command::PlaygroundWorker => crate::playground::worker_main(),
        Command::Inspect {
            pid,
            gc,
            profile,
            seconds,
        } => cmd_inspect(&pid, gc, profile, seconds),
        Command::Explain { code } => cmd_explain(&code),
        Command::Publish {
            index,
            registry,
            dry_run,
        } => cmd_publish(index.as_deref(), registry.as_deref(), dry_run),
        Command::Login {
            index,
            registry,
            token,
        } => cmd_login(index.as_deref(), registry.as_deref(), token.as_deref()),
        Command::Search {
            query,
            index,
            registry,
        } => cmd_search(&query, index.as_deref(), registry.as_deref()),
        Command::Info {
            name,
            index,
            registry,
        } => cmd_info(&name, index.as_deref(), registry.as_deref()),
        Command::Version => {
            println!("zap {}", crate::VERSION);
            ExitCode::SUCCESS
        }
        Command::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
    }
}

/// `zap build` — like `run` but stops at the binary, defaulting output to
/// `target/<stem>`.
fn cmd_build(file: Option<String>, out: Option<String>, features: &[String]) -> ExitCode {
    let entry = match resolve_entry(file) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let out = match out {
        Some(o) => o,
        None => {
            if let Err(e) = fs::create_dir_all("target") {
                eprintln!("zap: cannot create target/: {e}");
                return ExitCode::from(1);
            }
            let stem = Path::new(&entry)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("a");
            format!("target/{stem}")
        }
    };
    crate::compile_for(&entry, Some(out), false, features)
}

/// Resolve the input for `run`/`build`/`ir`/`expand`.
///
/// An explicit path wins. Otherwise, in project mode, a `zap.toml` in the
/// current directory selects `src/main.zap` as the entry point. (Dependency
/// resolution is the package manager's job, issue #69, and is not done here.)
fn resolve_entry(file: Option<String>) -> Result<String, ExitCode> {
    if let Some(f) = file {
        return Ok(f);
    }
    if Path::new("zap.toml").exists() {
        let entry = "src/main.zap";
        if Path::new(entry).exists() {
            return Ok(entry.to_string());
        }
        eprintln!("zap: found zap.toml but its entry point `{entry}` is missing");
        return Err(ExitCode::from(2));
    }
    eprintln!("zap: no input file given and no zap.toml in the current directory");
    eprintln!("give a file (`zap run path/to/file.zap`) or run inside a project");
    Err(ExitCode::from(2))
}

/// `zap new <name>` — create `<name>/` with a `zap.toml` and `src/main.zap`.
fn cmd_new(name: &str) -> ExitCode {
    let root = Path::new(name);
    if root.exists() {
        eprintln!("zap: `{name}` already exists");
        return ExitCode::from(1);
    }
    let project = root.file_name().and_then(|s| s.to_str()).unwrap_or(name);
    if let Err(e) = fs::create_dir_all(root.join("src")) {
        eprintln!("zap: cannot create {name}: {e}");
        return ExitCode::from(1);
    }
    if let Err(code) = write_scaffold(root, project) {
        return code;
    }
    println!("created project `{project}` in {name}/");
    println!("  cd {name} && zap run");
    ExitCode::SUCCESS
}

/// `zap init` — scaffold `zap.toml` (and `src/main.zap`) in the current dir.
fn cmd_init() -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("zap: cannot read the current directory: {e}");
            return ExitCode::from(1);
        }
    };
    if Path::new("zap.toml").exists() {
        eprintln!("zap: zap.toml already exists here");
        return ExitCode::from(1);
    }
    let project = cwd.file_name().and_then(|s| s.to_str()).unwrap_or("app");
    if let Err(e) = fs::create_dir_all("src") {
        eprintln!("zap: cannot create src/: {e}");
        return ExitCode::from(1);
    }
    if let Err(code) = write_scaffold(Path::new("."), project) {
        return code;
    }
    println!("initialized project `{project}`");
    println!("  zap run");
    ExitCode::SUCCESS
}

/// Write `zap.toml` and `src/main.zap` under `root`. `src/main.zap` is only
/// written when it does not already exist (so `zap init` never clobbers code).
fn write_scaffold(root: &Path, project: &str) -> Result<(), ExitCode> {
    let manifest = root.join("zap.toml");
    if let Err(e) = fs::write(&manifest, manifest_template(project)) {
        eprintln!("zap: cannot write {}: {e}", manifest.display());
        return Err(ExitCode::from(1));
    }
    let main = root.join("src").join("main.zap");
    if !main.exists() {
        if let Err(e) = fs::write(&main, main_template(project)) {
            eprintln!("zap: cannot write {}: {e}", main.display());
            return Err(ExitCode::from(1));
        }
    }
    Ok(())
}

/// The `zap.toml` manifest template (file shape only; D-0052). No dependency
/// resolution happens here — that is the package manager's job (issue #69).
fn manifest_template(project: &str) -> String {
    format!(
        "# Zap project manifest (zap.toml). TOML, Cargo-style (D-0052).\n\
         # Dependencies are managed by `zap add` / `zap remove` (issue #69) and\n\
         # resolved to a committed `zap.lock`; this file records metadata only.\n\
         [package]\n\
         name = \"{project}\"\n\
         version = \"0.1.0\"\n\
         \n\
         [dependencies]\n\
         # Logical name -> VCS source + semver (resolved by `zap add`, issue #69):\n\
         # jsonlib = {{ git = \"https://example.com/jsonlib.git\", version = \"1.4\" }}\n"
    )
}

/// The starter `src/main.zap`.
fn main_template(project: &str) -> String {
    format!("def main():\n    print(\"Hello from {project}!\")\n")
}

/// `zap test [file] [--json] [--watch]` — run the `@test` suite (D-0054).
#[cfg(feature = "codegen")]
fn cmd_test(file: Option<String>, json: bool, watch: bool, doc: bool) -> ExitCode {
    if doc {
        return crate::testing::doctest::run(file, json);
    }
    crate::testing::run(file, json, watch)
}

/// Without a backend the runner cannot compile and execute tests.
#[cfg(not(feature = "codegen"))]
fn cmd_test(_file: Option<String>, _json: bool, _watch: bool, _doc: bool) -> ExitCode {
    eprintln!("zap test: running tests needs codegen, which this build lacks");
    eprintln!("(rebuild with --features zapc/llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}

/// `zap add <name> <source>` — add a dependency and re-lock (issue #69).
fn cmd_add(name: &str, source: Source) -> ExitCode {
    match zapc_pkg::ops::add(Path::new("."), name, &source) {
        Ok(warnings) => {
            println!("added `{name}` to zap.toml and updated zap.lock");
            for w in &warnings {
                eprintln!("zap: {w}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap: {e}");
            ExitCode::from(1)
        }
    }
}

/// `zap remove <name>` — drop a dependency and re-lock (issue #69).
fn cmd_remove(name: &str) -> ExitCode {
    match zapc_pkg::ops::remove(Path::new("."), name) {
        Ok(()) => {
            println!("removed `{name}` from zap.toml and updated zap.lock");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap: {e}");
            ExitCode::from(1)
        }
    }
}

/// `zap publish [--index <id> | --registry <url>] [--dry-run]` — package the
/// current project and publish it to a static registry index (I#237, D-0083).
fn cmd_publish(index: Option<&str>, registry: Option<&str>, dry_run: bool) -> ExitCode {
    match zapc_pkg::ops::publish(Path::new("."), index, registry, dry_run) {
        Ok(p) => {
            for w in &p.warnings {
                eprintln!("zap: {w}");
            }
            let verb = if p.dry_run {
                "would publish"
            } else {
                "published"
            };
            println!("{verb} {} {}", p.name, p.version);
            println!("  checksum {}", p.cksum);
            println!("  archive  {} ({} bytes)", p.acksum, p.archive_len);
            if let Some(idx) = &p.index_file {
                println!("  index    {}", idx.display());
            }
            if let Some(arc) = &p.archive_file {
                println!("  package  {}", arc.display());
            }
            if p.dry_run && p.index_file.is_none() {
                println!("  (no target resolved; validated and packaged only)");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap: {e}");
            ExitCode::from(1)
        }
    }
}

/// `zap login [--index <id> | --registry <url>] [--token <t>]` — store a
/// registry token outside the repo (I#237). Without `--token`, read it from
/// stdin so it never lands in argv/shell history.
fn cmd_login(index: Option<&str>, registry: Option<&str>, token: Option<&str>) -> ExitCode {
    use std::io::Read;
    let token = match token {
        Some(t) => t.to_string(),
        None => {
            eprintln!("zap login: reading token from stdin (input not echoed by your shell)...");
            let mut s = String::new();
            if std::io::stdin().read_to_string(&mut s).is_err() {
                eprintln!("zap login: failed to read the token from stdin");
                return ExitCode::from(2);
            }
            s.trim().to_string()
        }
    };
    match zapc_pkg::ops::login(Some(Path::new(".")), index, registry, &token) {
        Ok(path) => {
            println!("stored registry token in {}", path.display());
            eprintln!(
                "note: tokens are only used by the hosted write API, which is not yet deployed \
                 (gated on the org move); a local/file:// index needs none"
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap: {e}");
            ExitCode::from(1)
        }
    }
}

/// `zap search <query> [--index <id> | --registry <url>]` — list matching
/// packages in a local/`file://` static index (I#237).
fn cmd_search(query: &str, index: Option<&str>, registry: Option<&str>) -> ExitCode {
    let root = project_root();
    match zapc_pkg::ops::search(root.as_deref(), index, registry, query) {
        Ok(hits) => {
            if hits.is_empty() {
                println!("no packages match `{query}`");
                return ExitCode::SUCCESS;
            }
            for h in &hits {
                let yank = if h.all_yanked {
                    "  (all versions yanked)"
                } else {
                    ""
                };
                println!(
                    "{}  {}  ({} version{}){yank}",
                    h.name,
                    h.latest,
                    h.versions,
                    if h.versions == 1 { "" } else { "s" }
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap: {e}");
            ExitCode::from(1)
        }
    }
}

/// `zap info <name> [--index <id> | --registry <url>]` — show a package's
/// published versions from any static index (I#237).
fn cmd_info(name: &str, index: Option<&str>, registry: Option<&str>) -> ExitCode {
    let root = project_root();
    match zapc_pkg::ops::info(root.as_deref(), index, registry, name) {
        Ok(entries) => {
            if entries.is_empty() {
                println!("`{name}` has no published versions");
                return ExitCode::SUCCESS;
            }
            let latest = zapc_pkg::discovery::latest(&entries);
            println!("{}", entries[0].name);
            if let Some((v, _)) = &latest {
                println!("  latest: {v}");
            }
            println!("  versions:");
            for e in &entries {
                let mut tags = Vec::new();
                if e.yanked {
                    tags.push("yanked".to_string());
                }
                if let Some(mz) = &e.min_zap {
                    tags.push(format!("needs zap>={mz}"));
                }
                let tagstr = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", tags.join(", "))
                };
                println!("    {}  {}{tagstr}", e.vers, e.cksum);
                for d in &e.deps {
                    println!("      -> {} {}", d.name, d.req);
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("zap: {e}");
            ExitCode::from(1)
        }
    }
}

/// The current directory as a project root, if it holds a `zap.toml` (so a
/// `--index <id>` can resolve against `[registries]`); `None` otherwise, which
/// is fine when a `--registry <url>` or `ZAP_REGISTRY` names the index.
fn project_root() -> Option<std::path::PathBuf> {
    let cwd = Path::new(".");
    if cwd.join("zap.toml").exists() {
        Some(cwd.to_path_buf())
    } else {
        None
    }
}

/// Shell out to the standard `kill` utility to deliver `signame` to `pid`.
/// Dependency-free (no libc crate). Returns true iff the signal was delivered.
fn send_signal(pid: &str, signame: &str) -> Result<bool, std::io::Error> {
    Ok(std::process::Command::new("kill")
        .arg("-s")
        .arg(signame)
        .arg(pid)
        .status()?
        .success())
}

/// `zap explain <ZAPnnnn>` — print the stable meaning of an error code
/// (I#157/F1, #239). The code and its documented meaning are the machine-stable
/// surface an agent keys off; the message text a build prints is not stable.
fn cmd_explain(code: &str) -> ExitCode {
    let Some(ec) = zapc_diag::ErrorCode::parse(code) else {
        eprintln!(
            "zap explain: `{code}` is not a known error code (expected e.g. `ZAP4001`).\n\
             Error codes look like `ZAPnnnn`; the one a diagnostic prints appears in \
             brackets, as in `error[ZAP4001]: ...`."
        );
        return ExitCode::from(2);
    };
    // `parse` only returns registered codes, so `info()` is always `Some`.
    let info = ec.info().expect("registered code has registry info");
    println!("{ec} [{}]: {}", info.category().slug(), info.title);
    println!();
    println!("{}", info.explanation);
    ExitCode::SUCCESS
}

/// `zap inspect <pid> [--gc | --profile [--seconds N]]` — signal a running Zap
/// process to print its runtime diagnostics (ticket #77/#78, D-0058). Sends
/// SIGQUIT (coroutine stacks + runtime/scheduler summary) by default, SIGUSR1
/// (force GC + memory stats) with `--gc`, or — with `--profile` — SIGUSR2 to
/// start CPU profiling, waits `seconds`, then SIGUSR2 again to stop and flush
/// the folded profile. The diagnostics/profile appear on the TARGET process's
/// stderr / its `ZAP_PROFILE_OUT` file; this just delivers the signal (so
/// operators need not hand-run `kill`). Dependency-free: shells out to `kill`.
fn cmd_inspect(pid: &str, gc: bool, profile: bool, seconds: u64) -> ExitCode {
    // Validate the pid is a positive integer so we never pass a process-group
    // (negative) or garbage target to `kill`.
    match pid.parse::<u32>() {
        Ok(p) if p > 0 => {}
        _ => {
            eprintln!("zap inspect: invalid pid `{pid}` (expected a positive integer)");
            return ExitCode::from(2);
        }
    }
    if profile {
        // SIGUSR2 toggles the profiler: start, wait the window, stop + flush.
        match send_signal(pid, "USR2") {
            Ok(true) => {}
            Ok(false) => {
                eprintln!(
                    "zap inspect: could not signal {pid} (no such process, or not permitted)"
                );
                return ExitCode::from(1);
            }
            Err(e) => {
                eprintln!("zap inspect: failed to run `kill`: {e}");
                return ExitCode::from(1);
            }
        }
        eprintln!("zap inspect: started CPU profiling on {pid} for {seconds}s (SIGUSR2)");
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        match send_signal(pid, "USR2") {
            Ok(true) => {
                eprintln!(
                    "zap inspect: stopped CPU profiling on {pid}; folded profile written \
                     (see its stderr for the output path)"
                );
                ExitCode::SUCCESS
            }
            Ok(false) => {
                eprintln!("zap inspect: could not stop profiling on {pid} (process gone?)");
                ExitCode::from(1)
            }
            Err(e) => {
                eprintln!("zap inspect: failed to run `kill`: {e}");
                ExitCode::from(1)
            }
        }
    } else {
        let signame = if gc { "USR1" } else { "QUIT" };
        let what = if gc {
            "GC + memory stats"
        } else {
            "coroutine stacks + runtime dump"
        };
        match send_signal(pid, signame) {
            Ok(true) => {
                eprintln!("zap inspect: sent SIG{signame} to {pid} ({what}); see its stderr");
                ExitCode::SUCCESS
            }
            Ok(false) => {
                eprintln!(
                    "zap inspect: could not signal {pid} (no such process, or not permitted)"
                );
                ExitCode::from(1)
            }
            Err(e) => {
                eprintln!("zap inspect: failed to run `kill`: {e}");
                ExitCode::from(1)
            }
        }
    }
}

/// `zap zenv <create|status|path|clean>` — the project environment (issue #69).
fn cmd_zenv(sub: ZenvCmd) -> ExitCode {
    let root = Path::new(".");
    match sub {
        ZenvCmd::Create { no_venv } => match zapc_pkg::ops::zenv_create(root, no_venv) {
            Ok(report) => {
                println!("created {}", report.zenv_dir.display());
                println!(
                    "  materialized {} path + {} registry module(s) into zenv/pkg",
                    report.modules_materialized, report.registry_modules_materialized
                );
                if report.git_modules_skipped > 0 {
                    println!(
                        "  {} git module(s) recorded; run `zap zenv create` with network to fetch",
                        report.git_modules_skipped
                    );
                }
                for w in &report.warnings {
                    eprintln!("zap: {w}");
                }
                match (report.venv_created, &report.venv_skipped_reason) {
                    (true, _) => println!("  Python venv ready at zenv/venv"),
                    (false, Some(reason)) => println!("  Python venv skipped ({reason})"),
                    (false, None) => {}
                }
                println!("activate it with:  source zenv/activate");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("zap: {e}");
                ExitCode::from(1)
            }
        },
        ZenvCmd::Status => cmd_zenv_status(root),
        ZenvCmd::Path => {
            let paths = zapc_pkg::zenv::module_search_paths_from_env();
            let joined = paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(":");
            println!("{joined}");
            ExitCode::SUCCESS
        }
        ZenvCmd::Clean => match zapc_pkg::ops::zenv_clean(root) {
            Ok(true) => {
                println!("removed zenv/");
                ExitCode::SUCCESS
            }
            Ok(false) => {
                println!("no zenv/ to remove");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("zap: {e}");
                ExitCode::from(1)
            }
        },
    }
}

/// `zap zenv status` — report whether a zenv exists and whether one is active.
fn cmd_zenv_status(root: &Path) -> ExitCode {
    let zdir = zapc_pkg::zenv::zenv_dir(root);
    if zdir.exists() {
        println!("zenv: present at {}", zdir.display());
        println!(
            "  Python venv: {}",
            if zapc_pkg::zenv::venv_dir(root).exists() {
                "present"
            } else {
                "absent"
            }
        );
    } else {
        println!("zenv: none here (create one with `zap zenv create`)");
    }
    match std::env::var(zapc_pkg::zenv::ZAP_ZENV) {
        Ok(active) if !active.is_empty() => println!("  active: yes ({active})"),
        _ => println!("  active: no (run `source zenv/activate`)"),
    }
    ExitCode::SUCCESS
}

/// `zap help` text.
const HELP: &str = "\
zap — the Zap toolchain (D-0053)

usage: zap <command> [arguments]

compile & run:
  run [file] [-o out]   compile and run a file, or the project (via zap.toml)
  build [file] [-o out] build a native binary (default: target/<name>)
  ir [file]             dump ZIR (diagnostic)
  expand [file]         show decorator/@derive-generated code (D-0036)
       --target-feature <name>   (run/build/ir; repeatable) assume a compile-time
                        CPU feature is present, for `when target.has(.name)`
                        conditional compilation (I#139, D-0079, experimental)

project:
  new <name>            scaffold a new project directory (writes zap.toml)
  init                  scaffold zap.toml in the current directory

dependencies (package manager, issue #69, D-0052):
  add <name> --git <url> --version <v>   add a VCS dependency, re-lock
  add <name> --path <dir>                add a local dependency, re-lock
  remove <name>                          drop a dependency, re-lock
  zenv create [--no-venv]                create the project environment
  zenv status | path | clean             inspect / clean the environment

registry (publish & discovery, issue #237, D-0027/D-0083, experimental):
  publish [--index id|--registry url]    package the project and publish it to a
          [--dry-run]                    static index (local/file://; an https
                                         index needs the hosted write API, which
                                         is gated on the org move). --dry-run
                                         validates and packages, writing nothing
  login [--index id|--registry url]      store a registry token outside the repo
        [--token t]                      (read from stdin if --token is omitted)
  search <query> [--index id|            list packages matching <query> in a
                 --registry url]         local/file:// index
  info <name> [--index id|               show a package's published versions,
              --registry url]            checksums, yank status and deps

test:
  test [file]           run the @test suite (D-0054); a file, or the project
       --json           emit structured results (for `zap mcp`, issue #72)
       --watch          re-run on change, targeting affected files
       --doc            instead run docstring examples (doctests, #234): every
                        ```zap block in a docstring is executed and checked
                        against its # => / # error: / # raises: expectations

agent interface:
  mcp [--root dir]      start the MCP server for coding agents (D-0055): serves
                        the language reference, parse+type-check without a build,
                        a token-efficient code graph, and targeted tests (stdio)

editor integration:
  lsp                   start the language server (LSP, issue #229): live
                        diagnostics over a JSON-RPC stdio transport, for editors
  repl                  start the interactive REPL (I#159): JIT-executes each
                        entry, persisting session bindings across entries
  playground            start the local web playground (I#238): an editor page
       [--host H]        and POST /run that executes a snippet server-side in an
       [--port N]        isolated worker process (default 127.0.0.1:8238);
       [--open]          --open launches a browser. Needs codegen.

diagnostics:
  inspect <pid>         signal a running Zap process to dump coroutine stacks +
                        a runtime/scheduler summary to its stderr (D-0058)
          --gc          force a GC and print memory stats instead (SIGUSR1)
          --profile     CPU-profile it (SIGUSR2 start/stop), writing a folded
                        profile; --seconds N sets the window (default 10)
  explain <ZAPnnnn>     explain a stable error code (e.g. `zap explain ZAP4001`)
  version               print the toolchain version
  help                  show this help

format:
  fmt [paths...]        rewrite files in place in the canonical style (D-0037);
                        a directory is formatted recursively; no paths formats
                        stdin to stdout
      --check           write nothing; exit non-zero and list any files that
                        are not already formatted (CI / fmt-gate mode)

documentation:
  doc <file>            render API documentation for a source file (I#232/#233)
      --format <fmt>    html (default: a self-contained static site), md, or
                        json (the serializable doc model)
      --out <dir>       output directory for html/md (default: doc)
      --json            alias for --format json (emit the model to stdout)
      --private         include _-prefixed (private, D-0028) declarations
";

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn explain_parses_a_code_and_requires_one() {
        assert_eq!(
            p(&["explain", "ZAP4001"]),
            Ok(Command::Explain {
                code: "ZAP4001".to_string()
            })
        );
        assert!(p(&["explain"]).is_err());
    }

    #[test]
    fn no_subcommand_is_an_error() {
        assert!(p(&[]).is_err());
    }

    #[test]
    fn unknown_subcommand_is_an_error() {
        assert_eq!(
            p(&["frobnicate"]).unwrap_err(),
            "unknown subcommand `frobnicate`"
        );
    }

    #[test]
    fn run_file_and_output() {
        assert_eq!(
            p(&["run", "a.zap"]).unwrap(),
            Command::Run {
                file: Some("a.zap".into()),
                out: None,
                features: vec![],
            }
        );
        assert_eq!(
            p(&["run", "-o", "bin/a", "a.zap"]).unwrap(),
            Command::Run {
                file: Some("a.zap".into()),
                out: Some("bin/a".into()),
                features: vec![],
            }
        );
    }

    #[test]
    fn run_without_file_is_project_mode() {
        assert_eq!(
            p(&["run"]).unwrap(),
            Command::Run {
                file: None,
                out: None,
                features: vec![],
            }
        );
    }

    #[test]
    fn build_long_out_flag() {
        assert_eq!(
            p(&["build", "--out", "x", "a.zap"]).unwrap(),
            Command::Build {
                file: Some("a.zap".into()),
                out: Some("x".into()),
                features: vec![],
            }
        );
    }

    #[test]
    fn target_feature_flag_is_collected() {
        // `--target-feature` is repeatable and feeds `when target.has(.f)`
        // (I#139, D-0079).
        assert_eq!(
            p(&[
                "run",
                "a.zap",
                "--target-feature",
                "sse4_2",
                "--target-feature",
                "avx2",
            ])
            .unwrap(),
            Command::Run {
                file: Some("a.zap".into()),
                out: None,
                features: vec!["sse4_2".into(), "avx2".into()],
            }
        );
        assert!(p(&["run", "a.zap", "--target-feature"]).is_err());
    }

    #[test]
    fn dangling_output_flag_errors() {
        assert!(p(&["build", "-o"]).is_err());
    }

    #[test]
    fn unknown_flag_errors() {
        assert_eq!(p(&["run", "--nope"]).unwrap_err(), "unknown flag `--nope`");
    }

    #[test]
    fn extra_positional_errors() {
        assert!(p(&["run", "a.zap", "b.zap"]).is_err());
    }

    #[test]
    fn ir_and_expand_take_a_file() {
        assert_eq!(
            p(&["ir", "a.zap"]).unwrap(),
            Command::Ir {
                file: Some("a.zap".into()),
                features: vec![],
            }
        );
        assert_eq!(
            p(&["expand", "a.zap"]).unwrap(),
            Command::Expand {
                file: Some("a.zap".into())
            }
        );
    }

    #[test]
    fn new_needs_a_name() {
        assert!(p(&["new"]).is_err());
        assert_eq!(
            p(&["new", "proj"]).unwrap(),
            Command::New {
                name: "proj".into()
            }
        );
    }

    #[test]
    fn test_flags_and_file() {
        assert_eq!(
            p(&["test"]).unwrap(),
            Command::Test {
                file: None,
                json: false,
                watch: false,
                doc: false
            }
        );
        assert_eq!(
            p(&["test", "--watch"]).unwrap(),
            Command::Test {
                file: None,
                json: false,
                watch: true,
                doc: false
            }
        );
        assert_eq!(
            p(&["test", "math_test.zap", "--json"]).unwrap(),
            Command::Test {
                file: Some("math_test.zap".into()),
                json: true,
                watch: false,
                doc: false
            }
        );
        assert_eq!(
            p(&["test", "--doc", "src"]).unwrap(),
            Command::Test {
                file: Some("src".into()),
                json: false,
                watch: false,
                doc: true
            }
        );
        // Doctests don't support `--watch` yet.
        assert!(p(&["test", "--doc", "--watch"]).is_err());
        assert!(p(&["test", "--nope"]).is_err());
    }

    #[test]
    fn version_and_help_aliases() {
        for a in ["version", "--version", "-V"] {
            assert_eq!(p(&[a]).unwrap(), Command::Version);
        }
        for a in ["help", "--help", "-h"] {
            assert_eq!(p(&[a]).unwrap(), Command::Help);
        }
    }

    #[test]
    fn init_verb_parses() {
        assert_eq!(p(&["init"]).unwrap(), Command::Init);
    }

    #[test]
    fn mcp_parses_with_optional_root() {
        assert_eq!(p(&["mcp"]).unwrap(), Command::Mcp { root: None });
        assert_eq!(
            p(&["mcp", "--root", "myproj"]).unwrap(),
            Command::Mcp {
                root: Some("myproj".into())
            }
        );
        assert!(p(&["mcp", "--root"]).is_err());
        assert!(p(&["mcp", "--socket"]).is_err());
        assert!(p(&["mcp", "bogus"]).is_err());
    }

    #[test]
    fn lsp_parses_and_rejects_args() {
        assert_eq!(p(&["lsp"]).unwrap(), Command::Lsp);
        assert_eq!(p(&["lsp", "--stdio"]).unwrap(), Command::Lsp);
        assert!(p(&["lsp", "--socket"]).is_err());
        assert!(p(&["lsp", "bogus"]).is_err());
    }

    #[test]
    fn playground_parses_defaults_and_flags() {
        assert_eq!(
            p(&["playground"]).unwrap(),
            Command::Playground {
                host: "127.0.0.1".to_string(),
                port: 8238,
                open: false,
            }
        );
        assert_eq!(
            p(&[
                "playground",
                "--host",
                "0.0.0.0",
                "--port",
                "9000",
                "--open"
            ])
            .unwrap(),
            Command::Playground {
                host: "0.0.0.0".to_string(),
                port: 9000,
                open: true,
            }
        );
        assert!(p(&["playground", "--port", "nope"]).is_err());
        assert!(p(&["playground", "--bogus"]).is_err());
        assert!(p(&["playground", "extra"]).is_err());
        // The hidden worker verb takes no arguments.
        assert_eq!(
            p(&["playground-worker"]).unwrap(),
            Command::PlaygroundWorker
        );
        assert!(p(&["playground-worker", "x"]).is_err());
    }

    #[test]
    fn doc_parses_file_and_flags() {
        // Default format is HTML.
        assert_eq!(
            p(&["doc", "m.zap"]).unwrap(),
            Command::Doc {
                file: Some("m.zap".into()),
                format: DocFormat::Html,
                out: None,
                private: false,
            }
        );
        // `--json` is the back-compat alias for `--format json`.
        assert_eq!(
            p(&["doc", "--json", "--private", "m.zap"]).unwrap(),
            Command::Doc {
                file: Some("m.zap".into()),
                format: DocFormat::Json,
                out: None,
                private: true,
            }
        );
        assert_eq!(
            p(&["doc", "--format", "md", "--out", "site", "m.zap"]).unwrap(),
            Command::Doc {
                file: Some("m.zap".into()),
                format: DocFormat::Md,
                out: Some("site".into()),
                private: false,
            }
        );
        assert_eq!(
            p(&["doc", "-f", "json", "m.zap"]).unwrap(),
            Command::Doc {
                file: Some("m.zap".into()),
                format: DocFormat::Json,
                out: None,
                private: false,
            }
        );
        assert!(p(&["doc", "--format", "pdf", "m.zap"]).is_err());
        assert!(p(&["doc", "--format"]).is_err());
        assert!(p(&["doc", "--out"]).is_err());
        assert!(p(&["doc", "--bogus", "m.zap"]).is_err());
        assert!(p(&["doc", "a.zap", "b.zap"]).is_err());
    }

    #[test]
    fn inspect_parses_pid_and_gc_flag() {
        assert_eq!(
            p(&["inspect", "1234"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: false,
                profile: false,
                seconds: 10
            }
        );
        assert_eq!(
            p(&["inspect", "1234", "--gc"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: true,
                profile: false,
                seconds: 10
            }
        );
        assert_eq!(
            p(&["inspect", "-g", "1234"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: true,
                profile: false,
                seconds: 10
            }
        );
    }

    #[test]
    fn inspect_parses_profile_flags() {
        assert_eq!(
            p(&["inspect", "1234", "--profile"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: false,
                profile: true,
                seconds: 10
            }
        );
        assert_eq!(
            p(&["inspect", "--profile", "--seconds", "5", "1234"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: false,
                profile: true,
                seconds: 5
            }
        );
        assert_eq!(
            p(&["inspect", "-p", "-s", "30", "1234"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: false,
                profile: true,
                seconds: 30
            }
        );
        // --gc and --profile are mutually exclusive.
        assert!(p(&["inspect", "1234", "--gc", "--profile"]).is_err());
        // --seconds needs a positive integer value.
        assert!(p(&["inspect", "1234", "--profile", "--seconds"]).is_err());
        assert!(p(&["inspect", "1234", "--profile", "--seconds", "0"]).is_err());
        assert!(p(&["inspect", "1234", "--profile", "--seconds", "x"]).is_err());
    }

    #[test]
    fn inspect_needs_a_pid() {
        assert!(p(&["inspect"]).is_err());
        assert!(p(&["inspect", "1", "2"]).is_err());
        assert!(p(&["inspect", "--bogus"]).is_err());
    }

    #[test]
    fn add_parses_path_and_git_sources() {
        assert_eq!(
            p(&["add", "mathx", "--path", "../mathx"]).unwrap(),
            Command::Add {
                name: "mathx".into(),
                source: Source::Path {
                    dir: "../mathx".into()
                }
            }
        );
        assert_eq!(
            p(&[
                "add",
                "jsonlib",
                "--git",
                "https://x/j.git",
                "--version",
                "1.4.0"
            ])
            .unwrap(),
            Command::Add {
                name: "jsonlib".into(),
                source: Source::Git {
                    url: "https://x/j.git".into(),
                    version: "1.4.0".into()
                }
            }
        );
    }

    #[test]
    fn add_rejects_bad_source_combinations() {
        // No source.
        assert!(p(&["add", "foo"]).is_err());
        // git without version.
        assert!(p(&["add", "foo", "--git", "u"]).is_err());
        // both sources.
        assert!(p(&[
            "add",
            "foo",
            "--git",
            "u",
            "--version",
            "1.0.0",
            "--path",
            "p"
        ])
        .is_err());
        // no name.
        assert!(p(&["add", "--path", "p"]).is_err());
    }

    #[test]
    fn remove_needs_a_name() {
        assert!(p(&["remove"]).is_err());
        assert_eq!(
            p(&["remove", "foo"]).unwrap(),
            Command::Remove { name: "foo".into() }
        );
    }

    #[test]
    fn zenv_subcommands_parse() {
        assert_eq!(p(&["zenv"]).unwrap(), Command::Zenv(ZenvCmd::Status));
        assert_eq!(
            p(&["zenv", "status"]).unwrap(),
            Command::Zenv(ZenvCmd::Status)
        );
        assert_eq!(
            p(&["zenv", "create"]).unwrap(),
            Command::Zenv(ZenvCmd::Create { no_venv: false })
        );
        assert_eq!(
            p(&["zenv", "create", "--no-venv"]).unwrap(),
            Command::Zenv(ZenvCmd::Create { no_venv: true })
        );
        assert_eq!(p(&["zenv", "path"]).unwrap(), Command::Zenv(ZenvCmd::Path));
        assert_eq!(
            p(&["zenv", "clean"]).unwrap(),
            Command::Zenv(ZenvCmd::Clean)
        );
        assert!(p(&["zenv", "bogus"]).is_err());
    }

    #[test]
    fn publish_parses_targets_and_dry_run() {
        assert_eq!(
            p(&["publish"]).unwrap(),
            Command::Publish {
                index: None,
                registry: None,
                dry_run: false
            }
        );
        assert_eq!(
            p(&["publish", "--index", "test", "--dry-run"]).unwrap(),
            Command::Publish {
                index: Some("test".into()),
                registry: None,
                dry_run: true
            }
        );
        assert_eq!(
            p(&["publish", "--registry", "file:///tmp/idx"]).unwrap(),
            Command::Publish {
                index: None,
                registry: Some("file:///tmp/idx".into()),
                dry_run: false
            }
        );
        // --index and --registry are mutually exclusive.
        assert!(p(&["publish", "--index", "a", "--registry", "b"]).is_err());
        assert!(p(&["publish", "--index"]).is_err());
        assert!(p(&["publish", "extra"]).is_err());
    }

    #[test]
    fn login_parses_token_and_targets() {
        assert_eq!(
            p(&["login", "--registry", "file:///i", "--token", "abc"]).unwrap(),
            Command::Login {
                index: None,
                registry: Some("file:///i".into()),
                token: Some("abc".into())
            }
        );
        // Token optional (read from stdin otherwise).
        assert_eq!(
            p(&["login", "--index", "corp"]).unwrap(),
            Command::Login {
                index: Some("corp".into()),
                registry: None,
                token: None
            }
        );
        assert!(p(&["login", "--token"]).is_err());
    }

    #[test]
    fn search_and_info_parse() {
        assert_eq!(
            p(&["search", "json", "--index", "test"]).unwrap(),
            Command::Search {
                query: "json".into(),
                index: Some("test".into()),
                registry: None
            }
        );
        assert!(p(&["search"]).is_err());
        assert_eq!(
            p(&["info", "jsonlib", "--registry", "file:///i"]).unwrap(),
            Command::Info {
                name: "jsonlib".into(),
                index: None,
                registry: Some("file:///i".into())
            }
        );
        assert!(p(&["info"]).is_err());
        assert!(p(&["info", "a", "b"]).is_err());
    }

    #[test]
    fn manifest_template_names_the_project() {
        let m = manifest_template("widgets");
        assert!(m.contains("name = \"widgets\""));
        assert!(m.contains("[dependencies]"));
    }
}

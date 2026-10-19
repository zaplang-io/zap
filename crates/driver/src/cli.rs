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

/// A parsed `zap` invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `zap run [file] [-o out]` — compile and run a file or the project.
    Run {
        file: Option<String>,
        out: Option<String>,
    },
    /// `zap build [file] [-o out]` — build a native binary into `target/`.
    Build {
        file: Option<String>,
        out: Option<String>,
    },
    /// `zap ir [file]` — dump ZIR (diagnostic).
    Ir { file: Option<String> },
    /// `zap expand [file]` — show decorator/`@derive`-generated code (D-0036).
    Expand { file: Option<String> },
    /// `zap fmt [paths...]` — canonical formatter (D-0037; not yet implemented).
    Fmt { paths: Vec<String> },
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
    Test {
        file: Option<String>,
        json: bool,
        watch: bool,
    },
    /// `zap mcp [--root <dir>]` — the agent MCP server (D-0055, issue #72).
    /// Serves over stdio. `root` is the codebase root for the code-graph and
    /// test surfaces (default: the current directory).
    Mcp { root: Option<String> },
    /// `zap inspect <pid> [--gc]` — signal a running Zap process to dump its
    /// runtime diagnostics (ticket #77, D-0058), so operators need not hand-run
    /// `kill`. Default: SIGQUIT (goroutine stacks + runtime/scheduler summary).
    /// `--gc`: SIGUSR1 (force a GC + print memory stats) instead.
    Inspect { pid: String, gc: bool },
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
            let (file, out) = parse_compile(rest)?;
            Ok(Command::Run { file, out })
        }
        "build" => {
            let (file, out) = parse_compile(rest)?;
            Ok(Command::Build { file, out })
        }
        "ir" => Ok(Command::Ir {
            file: first_positional(rest),
        }),
        "expand" => Ok(Command::Expand {
            file: first_positional(rest),
        }),
        "fmt" => Ok(Command::Fmt {
            paths: positionals(rest),
        }),
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
            for a in rest {
                match a.as_str() {
                    "--json" => json = true,
                    "--watch" => watch = true,
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
            Ok(Command::Test { file, json, watch })
        }
        "mcp" => parse_mcp(rest),
        "inspect" => parse_inspect(rest),
        "version" | "--version" | "-V" => Ok(Command::Version),
        "help" | "--help" | "-h" => Ok(Command::Help),
        other => Err(format!("unknown subcommand `{other}`")),
    }
}

/// Parse the `[-o <out>] [file]` form shared by `run` and `build`.
fn parse_compile(args: &[String]) -> Result<(Option<String>, Option<String>), String> {
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
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
    Ok((file, out))
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

/// Parse `zap inspect <pid> [--gc]` (ticket #77, D-0058). Exactly one positional
/// (the pid); `--gc`/`-g` selects the SIGUSR1 (GC-stats) action over the default
/// SIGQUIT (stack/runtime dump).
fn parse_inspect(args: &[String]) -> Result<Command, String> {
    let mut pid: Option<String> = None;
    let mut gc = false;
    for a in args {
        match a.as_str() {
            "--gc" | "-g" => gc = true,
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            other => {
                if pid.is_some() {
                    return Err(format!("unexpected extra argument `{other}`"));
                }
                pid = Some(other.to_string());
            }
        }
    }
    match pid {
        Some(pid) => Ok(Command::Inspect { pid, gc }),
        None => Err("`zap inspect` needs a process id: `zap inspect <pid> [--gc]`".to_string()),
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

/// Parse `zap add <name> (--git <url> --version <v> | --path <dir>)`.
fn parse_add(args: &[String]) -> Result<Command, String> {
    let mut name: Option<String> = None;
    let mut git: Option<String> = None;
    let mut path: Option<String> = None;
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
    match (git, path) {
        (Some(_), Some(_)) => Err("give either `--git` or `--path`, not both".to_string()),
        (Some(url), None) => {
            let version = version.ok_or_else(|| {
                "`--git` needs `--version <semver>` (the minimum version)".to_string()
            })?;
            Ok(Command::Add {
                name,
                source: Source::Git { url, version },
            })
        }
        (None, Some(dir)) => Ok(Command::Add {
            name,
            source: Source::Path { dir },
        }),
        (None, None) => Err(
            "`zap add <name>` needs a source: `--git <url> --version <v>` or `--path <dir>`"
                .to_string(),
        ),
    }
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
        Command::Run { file, out } => match resolve_entry(file) {
            Ok(f) => crate::compile(&f, out, true),
            Err(code) => code,
        },
        Command::Build { file, out } => cmd_build(file, out),
        Command::Ir { file } => match resolve_entry(file) {
            Ok(f) => crate::emit_ir(&f),
            Err(code) => code,
        },
        Command::Expand { file } => match resolve_entry(file) {
            Ok(f) => crate::emit_expand(&f),
            Err(code) => code,
        },
        Command::Fmt { .. } => {
            eprintln!("zap fmt: the canonical formatter is not yet implemented (D-0037).");
            ExitCode::from(2)
        }
        Command::New { name } => cmd_new(&name),
        Command::Init => cmd_init(),
        Command::Add { name, source } => cmd_add(&name, source),
        Command::Remove { name } => cmd_remove(&name),
        Command::Zenv(sub) => cmd_zenv(sub),
        Command::Test { file, json, watch } => cmd_test(file, json, watch),
        Command::Mcp { root } => crate::mcp::serve(root),
        Command::Inspect { pid, gc } => cmd_inspect(&pid, gc),
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
fn cmd_build(file: Option<String>, out: Option<String>) -> ExitCode {
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
    crate::compile(&entry, Some(out), false)
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
#[cfg(feature = "llvm")]
fn cmd_test(file: Option<String>, json: bool, watch: bool) -> ExitCode {
    crate::testing::run(file, json, watch)
}

/// Without a backend the runner cannot compile and execute tests.
#[cfg(not(feature = "llvm"))]
fn cmd_test(_file: Option<String>, _json: bool, _watch: bool) -> ExitCode {
    eprintln!("zap test: running tests needs codegen, which this build lacks");
    eprintln!("(rebuild with --features zapc/llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}

/// `zap add <name> <source>` — add a dependency and re-lock (issue #69).
fn cmd_add(name: &str, source: Source) -> ExitCode {
    match zapc_pkg::ops::add(Path::new("."), name, &source) {
        Ok(()) => {
            println!("added `{name}` to zap.toml and updated zap.lock");
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

/// `zap inspect <pid> [--gc]` — signal a running Zap process to print its
/// runtime diagnostics (ticket #77, D-0058). Sends SIGQUIT (goroutine stacks +
/// runtime/scheduler summary) by default, or SIGUSR1 (force GC + memory stats)
/// with `--gc`. The diagnostics appear on the TARGET process's stderr; this just
/// delivers the signal (so operators need not hand-run `kill`). Dependency-free:
/// shells out to the standard `kill` utility on the Unix target.
fn cmd_inspect(pid: &str, gc: bool) -> ExitCode {
    // Validate the pid is a positive integer so we never pass a process-group
    // (negative) or garbage target to `kill`.
    match pid.parse::<u32>() {
        Ok(p) if p > 0 => {}
        _ => {
            eprintln!("zap inspect: invalid pid `{pid}` (expected a positive integer)");
            return ExitCode::from(2);
        }
    }
    let signame = if gc { "USR1" } else { "QUIT" };
    let what = if gc {
        "GC + memory stats"
    } else {
        "goroutine stacks + runtime dump"
    };
    match std::process::Command::new("kill")
        .arg("-s")
        .arg(signame)
        .arg(pid)
        .status()
    {
        Ok(status) if status.success() => {
            eprintln!("zap inspect: sent SIG{signame} to {pid} ({what}); see its stderr");
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("zap inspect: could not signal {pid} (no such process, or not permitted)");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("zap inspect: failed to run `kill`: {e}");
            ExitCode::from(1)
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
                    "  materialized {} module(s) into zenv/pkg",
                    report.modules_materialized
                );
                if report.git_modules_skipped > 0 {
                    println!(
                        "  {} git module(s) recorded; run `zap zenv create` with network to fetch",
                        report.git_modules_skipped
                    );
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

project:
  new <name>            scaffold a new project directory (writes zap.toml)
  init                  scaffold zap.toml in the current directory

dependencies (package manager, issue #69, D-0052):
  add <name> --git <url> --version <v>   add a VCS dependency, re-lock
  add <name> --path <dir>                add a local dependency, re-lock
  remove <name>                          drop a dependency, re-lock
  zenv create [--no-venv]                create the project environment
  zenv status | path | clean             inspect / clean the environment

test:
  test [file]           run the @test suite (D-0054); a file, or the project
       --json           emit structured results (for `zap mcp`, issue #72)
       --watch          re-run on change, targeting affected files

agent interface:
  mcp [--root dir]      start the MCP server for coding agents (D-0055): serves
                        the language reference, parse+type-check without a build,
                        a token-efficient code graph, and targeted tests (stdio)

diagnostics:
  inspect <pid>         signal a running Zap process to dump goroutine stacks +
                        a runtime/scheduler summary to its stderr (D-0058)
          --gc          force a GC and print memory stats instead (SIGUSR1)
  version               print the toolchain version
  help                  show this help

not yet implemented / pending their own tickets:
  fmt                   canonical formatter (D-0037)
";

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
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
                out: None
            }
        );
        assert_eq!(
            p(&["run", "-o", "bin/a", "a.zap"]).unwrap(),
            Command::Run {
                file: Some("a.zap".into()),
                out: Some("bin/a".into())
            }
        );
    }

    #[test]
    fn run_without_file_is_project_mode() {
        assert_eq!(
            p(&["run"]).unwrap(),
            Command::Run {
                file: None,
                out: None
            }
        );
    }

    #[test]
    fn build_long_out_flag() {
        assert_eq!(
            p(&["build", "--out", "x", "a.zap"]).unwrap(),
            Command::Build {
                file: Some("a.zap".into()),
                out: Some("x".into())
            }
        );
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
                file: Some("a.zap".into())
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
                watch: false
            }
        );
        assert_eq!(
            p(&["test", "--watch"]).unwrap(),
            Command::Test {
                file: None,
                json: false,
                watch: true
            }
        );
        assert_eq!(
            p(&["test", "math_test.zap", "--json"]).unwrap(),
            Command::Test {
                file: Some("math_test.zap".into()),
                json: true,
                watch: false
            }
        );
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
    fn inspect_parses_pid_and_gc_flag() {
        assert_eq!(
            p(&["inspect", "1234"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: false
            }
        );
        assert_eq!(
            p(&["inspect", "1234", "--gc"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: true
            }
        );
        assert_eq!(
            p(&["inspect", "-g", "1234"]).unwrap(),
            Command::Inspect {
                pid: "1234".into(),
                gc: true
            }
        );
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
    fn manifest_template_names_the_project() {
        let m = manifest_template("widgets");
        assert!(m.contains("name = \"widgets\""));
        assert!(m.contains("[dependencies]"));
    }
}

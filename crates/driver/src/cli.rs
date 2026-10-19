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
    /// `zap add <dep>` — stub (package manager, issue #69).
    Add,
    /// `zap remove <dep>` — stub (package manager, issue #69).
    Remove,
    /// `zap zenv <...>` — stub (project environment, issue #69).
    Zenv,
    /// `zap test [--watch]` — stub (`@test` framework, issue #73).
    Test { watch: bool },
    /// `zap mcp` — stub (agent MCP server, issue #72).
    Mcp,
    /// `zap version`.
    Version,
    /// `zap help`.
    Help,
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
        "add" => Ok(Command::Add),
        "remove" => Ok(Command::Remove),
        "zenv" => Ok(Command::Zenv),
        "test" => Ok(Command::Test {
            watch: rest.iter().any(|a| a == "--watch"),
        }),
        "mcp" => Ok(Command::Mcp),
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
        Command::Add | Command::Remove => stub(
            "dependency management",
            "issue #69 (package manager, D-0052)",
        ),
        Command::Zenv => stub(
            "the project environment (zenv)",
            "issue #69 (package manager, D-0052)",
        ),
        Command::Test { .. } => stub("the test runner", "issue #73 (the @test framework, D-0054)"),
        Command::Mcp => stub("the agent MCP server", "issue #72 (zap mcp, D-0055)"),
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

/// Report that a verb's underlying system is not built yet, naming the ticket.
fn stub(what: &str, ticket: &str) -> ExitCode {
    eprintln!("zap: {what} is not available yet — tracked in {ticket}.");
    ExitCode::from(2)
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

diagnostics:
  version               print the toolchain version
  help                  show this help

not yet implemented / pending their own tickets:
  fmt                   canonical formatter (D-0037)
  test [--watch]        test runner (@test framework, issue #73)
  add / remove          dependency management (package manager, issue #69)
  zenv                  project environment (issue #69)
  mcp                   agent MCP server (issue #72)
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
    fn test_watch_flag() {
        assert_eq!(p(&["test"]).unwrap(), Command::Test { watch: false });
        assert_eq!(
            p(&["test", "--watch"]).unwrap(),
            Command::Test { watch: true }
        );
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
    fn stub_and_project_verbs_parse() {
        assert_eq!(p(&["add", "foo"]).unwrap(), Command::Add);
        assert_eq!(p(&["remove", "foo"]).unwrap(), Command::Remove);
        assert_eq!(p(&["zenv", "create"]).unwrap(), Command::Zenv);
        assert_eq!(p(&["mcp"]).unwrap(), Command::Mcp);
        assert_eq!(p(&["init"]).unwrap(), Command::Init);
    }

    #[test]
    fn manifest_template_names_the_project() {
        let m = manifest_template("widgets");
        assert!(m.contains("name = \"widgets\""));
        assert!(m.contains("[dependencies]"));
    }
}

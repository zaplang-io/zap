//! `fs` module — sema facet (ticket #97, D-0056).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "fs", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        "read_file" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Str
        }
        "write_file" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::Unit
        }
        "exists" | "is_dir" | "is_file" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Bool
        }
        "size" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Int64
        }
        "listdir" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }
        // fs.lines(path) -> list[str] (#969, D-0212): whole-file read split into
        // lines with the terminators stripped. Convenience for in-memory files;
        // stream larger inputs with io.reader + io.readline.
        "lines" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }
        // fs.open_reader(path) -> LineReader (#1031, D-0223): open `path` for
        // reading and wrap it in the prelude `LineReader`, so `for line in
        // fs.open_reader(path)` streams the file a line at a time (lines keep
        // their trailing newline) via the D-0048 iterator protocol.
        "open_reader" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Named("LineReader".to_string())
        }
        // Directory traversal (#1032, D-0224).
        // fs.scandir(path) -> list[DirEntry]: one directory level, each entry a
        // prelude `DirEntry` (name + is_dir/is_file) typed from readdir d_type.
        "scandir" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::List(Box::new(Ty::Named("DirEntry".to_string())))
        }
        // fs.walk(root) -> list[(dirpath, dirs, files)]: os.walk-shaped, top-down,
        // `for (dirpath, dirs, files) in fs.walk(root)`. dirs/files are the
        // entry names at `dirpath` (sorted); the whole tree is materialised.
        "walk" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::List(Box::new(Ty::Tuple(vec![
                Ty::Str,
                Ty::List(Box::new(Ty::Str)),
                Ty::List(Box::new(Ty::Str)),
            ])))
        }
        // fs.copytree(src, dst) -> unit: recursive directory copy (shutil.copytree,
        // merging into an existing dst). Reuses the #968 file-copy primitives.
        "copytree" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::Unit
        }
        // Path matching (#1033, D-0216).
        // fs.fnmatch(name, pattern) -> bool: shell-glob string match.
        "fnmatch" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::Bool
        }
        // fs.glob(root, pattern) -> list[str]: matching paths relative to root.
        "glob" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::List(Box::new(Ty::Str))
        }
        "mkdtemp" => {
            expect_argc(checker, "fs", method, args, 0, span);
            Ty::Str
        }
        // Filesystem gaps (#968, D-0211).
        // Metadata: `stat` returns the prelude `Stat` value struct; `mtime` an int.
        "stat" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Named("Stat".to_string())
        }
        "mtime" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Int64
        }
        // Mutation (unit-returning).
        "rename" | "move" | "copy" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::Unit
        }
        "makedirs" | "rmtree" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Unit
        }
        // Binary I/O.
        "read_bytes" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Bytes
        }
        "write_bytes" | "append" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::Unit
        }
        _ => return None,
    })
}

//! `fs` module — lowering facet (ticket #97, D-0056; filesystem gaps #968,
//! D-0211).
//!
//! Most entries lower to a single backing `zap_fs_*` runtime call. `fs.stat`
//! builds the prelude `Stat` value struct — `StructNew` then a `FieldSet` per
//! field, each fed by a `zap_fs_stat_<field>(path)` runtime getter (the `KeyInfo`
//! pattern, crypto #643). `size` is emitted first as a throwing call: it is the
//! validating gate, so a non-existent path raises a catchable error before any
//! other field is read.

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, empty_str, line_reader_wrap, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "fs", lower };

/// Build the prelude `Stat` value struct from the per-field getters. Field order
/// matches `PRELUDE_SRC`: size(0), mtime(1), ctime(2), mode(3) are `int64`;
/// is_dir(4), is_file(5), is_symlink(6) are `bool`.
fn lower_stat(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let sid = *lowerer
        .ctx
        .ids
        .get("Stat")
        .ok_or_else(|| err("Stat struct unavailable (built-in prelude missing)"))?;
    let (path, _) = lowerer.expr(&arg.value)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });

    // Field 0: size (int64) — the validating gate, throwing (raises on a bad path).
    let size = lowerer.new_value(Ty::I64);
    lowerer.throwing_call(Some(size), "zap_fs_stat_size", vec![path]);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: size,
    });

    // Fields 1..=3: mtime / ctime / mode (int64). Reached only once `size`
    // validated the path, so they never raise for a readable path.
    for (idx, func) in [
        (1u32, "zap_fs_stat_mtime"),
        (2, "zap_fs_stat_ctime"),
        (3, "zap_fs_stat_mode"),
    ] {
        let v = lowerer.new_value(Ty::I64);
        lowerer.emit(Inst::Call {
            dest: Some(v),
            func: func.to_string(),
            args: vec![path],
        });
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx,
            val: v,
        });
    }

    // Fields 4..=6: is_dir / is_file / is_symlink (bool) — C int64 0/1 -> i1.
    for (idx, func) in [
        (4u32, "zap_fs_stat_is_dir"),
        (5, "zap_fs_stat_is_file"),
        (6, "zap_fs_stat_is_symlink"),
    ] {
        let (v, _) = call_bool(lowerer, func, vec![path]);
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx,
            val: v,
        });
    }

    Ok((dest, Ty::Struct(sid)))
}

/// `fs.open_reader(path)`: open `path` read-only and wrap the buffered reader in
/// the prelude `LineReader` (#1031, D-0223). Composes the existing `io.open` +
/// `io.reader` runtime entries — `open` is the validating gate, so a missing or
/// unreadable path raises a catchable error before the wrap.
fn lower_open_reader(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let (path, _) = lowerer.expr(&arg.value)?;
    let mode = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::StrConst {
        dest: mode,
        bytes: b"r".to_vec(),
    });
    let fd = lowerer.new_value(Ty::I64);
    lowerer.throwing_call(Some(fd), "zap_io_open", vec![path, mode]);
    let handle = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(handle),
        func: "zap_io_reader_new".to_string(),
        args: vec![fd],
    });
    line_reader_wrap(lowerer, handle)
}

/// `fs.mkstemp([prefix, suffix, dir])` -> `(fd, path)` (#979, D-0239). The three
/// optional str arguments are padded to a fixed 3-word runtime call — an omitted
/// `prefix` defaults to `"tmp"` (Python `tempfile`'s prefix), `suffix`/`dir` to
/// `""` (empty `dir` => the platform temp dir, resolved in the runtime). The
/// create is the throwing/validating call (a bad `dir`, no permission, etc.
/// raises a catchable error); the two getters then read the handle's fd and path
/// back into an `(int64, str)` tuple (the scandir/walk handle pattern, #1032).
fn lower_mkstemp(lowerer: &mut FnLower, a: Vec<Value>) -> (Value, Ty) {
    let prefix = match a.first().copied() {
        Some(v) => v,
        None => {
            let d = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::StrConst {
                dest: d,
                bytes: b"tmp".to_vec(),
            });
            d
        }
    };
    let suffix = a.get(1).copied().unwrap_or_else(|| empty_str(lowerer));
    let dir = a.get(2).copied().unwrap_or_else(|| empty_str(lowerer));
    let handle = lowerer.new_value(Ty::Ptr);
    lowerer.throwing_call(Some(handle), "zap_fs_mkstemp", vec![prefix, suffix, dir]);
    let fd = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(fd),
        func: "zap_fs_mkstemp_fd".to_string(),
        args: vec![handle],
    });
    let path = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(path),
        func: "zap_fs_mkstemp_path".to_string(),
        args: vec![handle],
    });
    // Intern the (int64, str) tuple with the real field types — str stays a
    // pointer word (`Ty::Ptr`), matching how `fs.walk` interns its tuple (#1032).
    let tuple_sid = lowerer
        .ctx
        .tuples
        .borrow_mut()
        .intern(vec![Ty::I64, Ty::Ptr]);
    let tup = lowerer.new_value(Ty::Struct(tuple_sid));
    lowerer.emit(Inst::StructNew {
        dest: tup,
        sid: tuple_sid,
    });
    lowerer.emit(Inst::FieldSet {
        base: tup,
        sid: tuple_sid,
        idx: 0,
        val: fd,
    });
    lowerer.emit(Inst::FieldSet {
        base: tup,
        sid: tuple_sid,
        idx: 1,
        val: path,
    });
    (tup, Ty::Struct(tuple_sid))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // stat builds a value struct from the path arg; the rest are single runtime
    // calls over the lowered argument words.
    if method == "stat" {
        return Ok(Some(lower_stat(lowerer, &args[0])?));
    }
    // open_reader composes io.open + io.reader and wraps the handle in the
    // prelude `LineReader` iterable (#1031, D-0223) — not a single flat call.
    if method == "open_reader" {
        let arg = args
            .first()
            .ok_or_else(|| err("fs.open_reader expects 1 argument"))?;
        return Ok(Some(lower_open_reader(lowerer, arg)?));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "read_file" => call_ret(lowerer, "zap_fs_read_file", a, Ty::Ptr, true),
        "write_file" => call_void(lowerer, "zap_fs_write_file", a, true),
        "exists" => call_bool(lowerer, "zap_fs_exists", a),
        "is_dir" => call_bool(lowerer, "zap_fs_is_dir", a),
        "is_file" => call_bool(lowerer, "zap_fs_is_file", a),
        "size" => call_ret(lowerer, "zap_fs_size", a, Ty::I64, true),
        "listdir" => call_ret(lowerer, "zap_os_listdir", a, Ty::List(Elem::Ptr), true),
        "mkdtemp" => call_ret(lowerer, "zap_fs_mkdtemp", a, Ty::Ptr, true),
        // Tempfile (#979, D-0239). mkstemp pads/builds its (fd, path) tuple;
        // gettempdir never raises; write_atomic is a throwing void call (an I/O
        // error must reach try/except, so throwing_call/invoke, not plain Call).
        "mkstemp" => lower_mkstemp(lowerer, a),
        "gettempdir" => call_ret(lowerer, "zap_fs_gettempdir", a, Ty::Ptr, false),
        "write_atomic" => call_void(lowerer, "zap_fs_write_atomic", a, true),
        // Filesystem gaps (#968, D-0211).
        "mtime" => call_ret(lowerer, "zap_fs_mtime", a, Ty::I64, true),
        "rename" => call_void(lowerer, "zap_fs_rename", a, true),
        "move" => call_void(lowerer, "zap_fs_move", a, true),
        "copy" => call_void(lowerer, "zap_fs_copy", a, true),
        "makedirs" => call_void(lowerer, "zap_fs_makedirs", a, true),
        "rmtree" => call_void(lowerer, "zap_fs_rmtree", a, true),
        "read_bytes" => call_ret(lowerer, "zap_fs_read_bytes", a, Ty::Bytes, true),
        "write_bytes" => call_void(lowerer, "zap_fs_write_bytes", a, true),
        "append" => call_void(lowerer, "zap_fs_append", a, true),
        // Line-oriented input (#969, D-0212): whole-file read split into lines.
        "lines" => call_ret(lowerer, "zap_fs_lines", a, Ty::List(Elem::Ptr), true),
        // Path matching (#1033, D-0216). Neither raises: a bad pattern or an
        // unreadable directory simply yields no match.
        "fnmatch" => call_bool(lowerer, "zap_fs_fnmatch", a),
        "glob" => call_ret(lowerer, "zap_fs_glob", a, Ty::List(Elem::Ptr), false),
        // Directory traversal (#1032, D-0224). scandir / walk call the runtime for
        // an opaque handle, then enumerate it into a list[DirEntry] / list of
        // (dirpath, dirs, files) tuples (the req_headers pattern). copytree is a
        // plain throwing void call.
        "scandir" => lowerer.lower_fs_scandir(a[0]),
        "walk" => lowerer.lower_fs_walk(a[0]),
        "copytree" => call_void(lowerer, "zap_fs_copytree", a, true),
        _ => return Ok(None),
    }))
}

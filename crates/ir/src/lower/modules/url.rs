//! `url` module — lowering facet (ticket #335, D-0056; parse/build #970,
//! D-0209).
//!
//! The percent codec and the `dict[str,str]` query builders each lower to a
//! backing `zap_url_*` C entry point in `zaprt.c`: `str`/`dict` values are
//! pointers and the result is a `str` (pointer); none of them raise.
//!
//! The parse/build half (#970):
//!
//!   * `split(s)` builds the prelude `UrlParts` value struct — `StructNew` then
//!     a `FieldSet` per component, each fed by a `zap_url_<field>(s)` runtime
//!     getter (the `KeyInfo` pattern, crypto #643). `has_authority` is a C
//!     `int64` 0/1 normalized to an `i1`; `port` is an `int64` (`-1` absent) and
//!     is the one throwing getter — a present but invalid port raises a
//!     catchable `ValueError`.
//!   * `build(parts)` reads the eight fields back with `FieldGet` and hands them
//!     to a single `zap_url_build` call (the `i1` `has_authority` widened to the
//!     `int64` the runtime takes).
//!   * `parse_query` / `parse_query_first` lower a `str` to a freshly-built dict;
//!     `encode_query_multi` a `dict[str,list[str]]` to a `str`; `join` resolves
//!     `(base, ref)` to a `str`. All are plain (infallible) pointer calls.

use zapc_syntax::ast;

use super::support::{call_bool, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{elem_kind, err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "url", lower };

/// The prelude `UrlParts` field getters, in field order. `has_authority` and
/// `port` return an `int64`; the rest return a `str` (pointer).
const SCHEME: &str = "zap_url_scheme";
const USERINFO: &str = "zap_url_userinfo";
const HOST: &str = "zap_url_host";
const PATH: &str = "zap_url_path";
const QUERY: &str = "zap_url_query";
const FRAGMENT: &str = "zap_url_fragment";

/// Resolve the prelude `UrlParts` struct id (injected by the parser's prelude
/// whenever the `url` surface is used).
fn url_parts_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("UrlParts")
        .copied()
        .ok_or_else(|| err("UrlParts struct unavailable (built-in prelude missing)"))
}

/// Emit an infallible `zap_url_<field>(s) -> str` getter.
fn str_getter(lowerer: &mut FnLower, func: &str, s: Value) -> Value {
    let v = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(v),
        func: func.to_string(),
        args: vec![s],
    });
    v
}

/// `url.split(s)`: parse `s` and build a `UrlParts` value struct, one
/// `zap_url_<field>(s)` getter per field. Field order matches `PRELUDE_SRC`:
/// scheme(0), has_authority(1), userinfo(2), host(3), port(4), path(5),
/// query(6), fragment(7).
fn lower_split(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let sid = url_parts_sid(lowerer)?;
    let (s, _) = lowerer.expr(&arg.value)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });

    // 0: scheme (str).
    let scheme = str_getter(lowerer, SCHEME, s);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: scheme,
    });

    // 1: has_authority (bool) — C int64 0/1 normalized to i1.
    let (has_auth, _) = call_bool(lowerer, "zap_url_has_authority", vec![s]);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 1,
        val: has_auth,
    });

    // 2: userinfo (str).
    let userinfo = str_getter(lowerer, USERINFO, s);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 2,
        val: userinfo,
    });

    // 3: host (str).
    let host = str_getter(lowerer, HOST, s);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 3,
        val: host,
    });

    // 4: port (int64, -1 absent) — the validating gate: a present but invalid
    // port raises a catchable ValueError, so emit it as a throwing call.
    let port = lowerer.new_value(Ty::I64);
    lowerer.throwing_call(Some(port), "zap_url_port", vec![s]);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 4,
        val: port,
    });

    // 5: path (str).
    let path = str_getter(lowerer, PATH, s);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 5,
        val: path,
    });

    // 6: query (str).
    let query = str_getter(lowerer, QUERY, s);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 6,
        val: query,
    });

    // 7: fragment (str).
    let fragment = str_getter(lowerer, FRAGMENT, s);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 7,
        val: fragment,
    });

    Ok((dest, Ty::Struct(sid)))
}

/// `url.build(parts)`: read the eight `UrlParts` fields and reassemble. The
/// `i1` `has_authority` is widened to the `int64` the runtime takes.
fn lower_build(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let sid = url_parts_sid(lowerer)?;
    let (base, _) = lowerer.expr(&arg.value)?;
    let scheme = lowerer.field_get(base, sid, 0, Ty::Ptr);
    let has_auth_i1 = lowerer.field_get(base, sid, 1, Ty::I1);
    let has_auth = lowerer.coerce(has_auth_i1, Ty::I1, Ty::I64);
    let userinfo = lowerer.field_get(base, sid, 2, Ty::Ptr);
    let host = lowerer.field_get(base, sid, 3, Ty::Ptr);
    let port = lowerer.field_get(base, sid, 4, Ty::I64);
    let path = lowerer.field_get(base, sid, 5, Ty::Ptr);
    let query = lowerer.field_get(base, sid, 6, Ty::Ptr);
    let fragment = lowerer.field_get(base, sid, 7, Ty::Ptr);
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: "zap_url_build".to_string(),
        args: vec![
            scheme, has_auth, userinfo, host, port, path, query, fragment,
        ],
    });
    Ok((d, Ty::Ptr))
}

/// Emit an infallible runtime call returning a value of IR type `ret`, from the
/// (already lowered) argument values. The `ret` must match the collection shape
/// so a later `auto` binding indexes/iterates it correctly: the query readers
/// return a dict (keys + values are reference words — `Elem::Ptr`), everything
/// else a `str` (`Ty::Ptr`).
fn plain_call(lowerer: &mut FnLower, func: &str, args: Vec<Value>, ret: Ty) -> (Value, Ty) {
    let d = lowerer.new_value(ret);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args,
    });
    (d, ret)
}

/// `url.unquote(s[, errors])` / `url.unquote_plus(s[, errors])` (#1242). The
/// `errors` keyword selects the UTF-8 decode mode for the percent-decoded octets
/// ("strict" — the default — raises a catchable error on invalid UTF-8 like
/// `str(bytes)`; "replace" substitutes U+FFFD). Both forms are emitted as
/// throwing calls: strict mode can raise a `ValueError`, and an unrecognized
/// `errors` name raises a `LookupError`. When `errors` is omitted the seam binder
/// leaves the trailing slot absent, so we fall back to the one-arg entry point
/// (which is itself the strict decode — identical to `errors="strict"`).
fn lower_unquote(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    plus: bool,
) -> Result<(Value, Ty), LowerError> {
    let (s, _) = lowerer.expr(&args[0].value)?;
    let d = lowerer.new_value(Ty::Ptr);
    if let Some(errors_arg) = args.get(1) {
        let (errors, _) = lowerer.expr(&errors_arg.value)?;
        let func = if plus {
            "zap_url_unquote_plus_e"
        } else {
            "zap_url_unquote_e"
        };
        lowerer.throwing_call(Some(d), func, vec![s, errors]);
    } else {
        let func = if plus {
            "zap_url_unquote_plus"
        } else {
            "zap_url_unquote"
        };
        lowerer.throwing_call(Some(d), func, vec![s]);
    }
    Ok((d, Ty::Ptr))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // split / build need the struct-building / field-reading paths; the rest are
    // plain pointer calls over the lowered argument words.
    Ok(Some(match method {
        "split" => lower_split(lowerer, &args[0])?,
        "build" => lower_build(lowerer, &args[0])?,
        "unquote" => lower_unquote(lowerer, args, false)?,
        "unquote_plus" => lower_unquote(lowerer, args, true)?,
        "parse_query" => {
            // `dict[str, list[str]]`: the value is a NESTED collection, so its
            // element must be `Elem::Nested(list[str])`, NOT a bare `Elem::Ptr` —
            // otherwise read-back (`q[k][i]`) collapses the inner `list[str]` to a
            // pointer and dispatches as a string op, yielding empty values (the
            // exact #625 nested-collection failure). The `str` key stays
            // `Elem::Ptr`. An `auto`-bound result keys off this IR type, so it
            // must carry the real nesting.
            let a = lower_args(lowerer, args)?;
            let val = elem_kind(Ty::List(Elem::Ptr), &lowerer.ctx.nested)?;
            plain_call(lowerer, "zap_url_parse_query", a, Ty::Dict(Elem::Ptr, val))
        }
        "parse_query_first" => {
            // `dict[str, str]`: both key and value are plain `str` reference words.
            let a = lower_args(lowerer, args)?;
            plain_call(
                lowerer,
                "zap_url_parse_query_first",
                a,
                Ty::Dict(Elem::Ptr, Elem::Ptr),
            )
        }
        _ => {
            let a = lower_args(lowerer, args)?;
            match method {
                "quote" => plain_call(lowerer, "zap_url_quote", a, Ty::Ptr),
                "quote_path" => plain_call(lowerer, "zap_url_quote_path", a, Ty::Ptr),
                "encode_query" => plain_call(lowerer, "zap_url_encode_query", a, Ty::Ptr),
                "canonical_query" => plain_call(lowerer, "zap_url_canonical_query", a, Ty::Ptr),
                "encode_query_multi" => {
                    plain_call(lowerer, "zap_url_encode_query_multi", a, Ty::Ptr)
                }
                "join" => plain_call(lowerer, "zap_url_join", a, Ty::Ptr),
                _ => return Ok(None),
            }
        }
    }))
}

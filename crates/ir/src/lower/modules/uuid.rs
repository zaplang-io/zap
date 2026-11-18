//! `uuid` module — lowering facet (ticket #975, D-0232/D-0233).
//!
//! The generators and `parse` each produce the 16 raw UUID bytes from a backing
//! `zap_uuid_*` runtime call and wrap them in the prelude `Uuid` value struct
//! (`StructNew` + one `FieldSet`, the `KeyInfo` pattern). `uuid3`/`uuid5` read
//! the namespace argument's raw bytes back out with `FieldGet`. `nanoid` lowers
//! to one `zap_uuid_nanoid(alphabet, size)` call; its 0-arg / size-only forms
//! supply the default URL-safe alphabet as a `StrConst`. The internal
//! `_format`/`_hex`/`_urn`/`_version`/`_variant` helpers back the `Uuid`
//! accessor methods.

use zapc_syntax::ast;

use super::support::call_ret;
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "uuid",
    lower,
};

/// The ai/nanoid default URL-safe alphabet (`urlAlphabet`, 64 chars). The single
/// authoritative copy; the runtime only ever receives an explicit alphabet.
const NANOID_DEFAULT_ALPHABET: &[u8] =
    b"useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict";

/// Wrap 16 raw bytes in the prelude `Uuid` value struct.
fn build_uuid(lowerer: &mut FnLower, raw: Value) -> Result<(Value, Ty), LowerError> {
    let sid = *lowerer
        .ctx
        .ids
        .get("Uuid")
        .ok_or_else(|| err("Uuid struct unavailable (built-in prelude missing)"))?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: raw,
    });
    Ok((dest, Ty::Struct(sid)))
}

/// Read a `Uuid` argument's raw-bytes field (field 0).
fn uuid_raw(lowerer: &mut FnLower, uuid_val: Value) -> Result<Value, LowerError> {
    let sid = *lowerer
        .ctx
        .ids
        .get("Uuid")
        .ok_or_else(|| err("Uuid struct unavailable (built-in prelude missing)"))?;
    let raw = lowerer.new_value(Ty::Bytes);
    lowerer.emit(Inst::FieldGet {
        dest: raw,
        base: uuid_val,
        sid,
        idx: 0,
    });
    Ok(raw)
}

/// Emit a zero-argument generator call returning 16 raw bytes, then wrap it.
fn gen0(lowerer: &mut FnLower, func: &str) -> Result<(Value, Ty), LowerError> {
    let (raw, _) = call_ret(lowerer, func, Vec::new(), Ty::Bytes, false);
    build_uuid(lowerer, raw)
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    Ok(Some(match method {
        // Zero-argument generators + the Appendix C namespace constants.
        "uuid4" => gen0(lowerer, "zap_uuid_v4")?,
        "uuid7" => gen0(lowerer, "zap_uuid_v7")?,
        "uuid1" => gen0(lowerer, "zap_uuid_v1")?,
        "nil" => gen0(lowerer, "zap_uuid_nil")?,
        "max" => gen0(lowerer, "zap_uuid_max")?,
        "namespace_dns" => gen0(lowerer, "zap_uuid_ns_dns")?,
        "namespace_url" => gen0(lowerer, "zap_uuid_ns_url")?,
        "namespace_oid" => gen0(lowerer, "zap_uuid_ns_oid")?,
        "namespace_x500" => gen0(lowerer, "zap_uuid_ns_x500")?,
        // uuid3(ns, name) / uuid5(ns, name): hash(ns.raw || name).
        "uuid3" | "uuid5" => {
            if args.len() != 2 {
                return Err(err("uuid.uuid3/uuid5 takes exactly two arguments"));
            }
            let (ns_val, _) = lowerer.expr(&args[0].value)?;
            let ns_raw = uuid_raw(lowerer, ns_val)?;
            let (name, _) = lowerer.expr(&args[1].value)?;
            let func = if method == "uuid3" {
                "zap_uuid_v3"
            } else {
                "zap_uuid_v5"
            };
            let (raw, _) = call_ret(lowerer, func, vec![ns_raw, name], Ty::Bytes, false);
            build_uuid(lowerer, raw)?
        }
        // parse(s): throwing (raises ValueError on a malformed string).
        "parse" => {
            if args.len() != 1 {
                return Err(err("uuid.parse takes exactly one argument"));
            }
            let (s, _) = lowerer.expr(&args[0].value)?;
            let (raw, _) = call_ret(lowerer, "zap_uuid_parse", vec![s], Ty::Bytes, true);
            build_uuid(lowerer, raw)?
        }
        // nanoid() / nanoid(size) / nanoid(alphabet, size) -> str (throwing: a
        // negative size or an out-of-range alphabet length raises ValueError).
        "nanoid" => {
            let (alphabet, size) = match args.len() {
                0 => {
                    let a = lowerer.new_value(Ty::Ptr);
                    lowerer.emit(Inst::StrConst {
                        dest: a,
                        bytes: NANOID_DEFAULT_ALPHABET.to_vec(),
                    });
                    (a, lowerer.iconst(Ty::I64, 21))
                }
                1 => {
                    let a = lowerer.new_value(Ty::Ptr);
                    lowerer.emit(Inst::StrConst {
                        dest: a,
                        bytes: NANOID_DEFAULT_ALPHABET.to_vec(),
                    });
                    let (sz, szt) = lowerer.expr(&args[0].value)?;
                    (a, lowerer.coerce(sz, szt, Ty::I64))
                }
                2 => {
                    let (a, _) = lowerer.expr(&args[0].value)?;
                    let (sz, szt) = lowerer.expr(&args[1].value)?;
                    (a, lowerer.coerce(sz, szt, Ty::I64))
                }
                _ => return Err(err("uuid.nanoid takes 0, 1, or 2 arguments")),
            };
            call_ret(
                lowerer,
                "zap_uuid_nanoid",
                vec![alphabet, size],
                Ty::Ptr,
                true,
            )
        }
        // Internal accessors over the raw 16 bytes (back the `Uuid` methods).
        "_format" | "_hex" | "_urn" => {
            if args.len() != 1 {
                return Err(err("uuid internal accessor takes one argument"));
            }
            let (raw, _) = lowerer.expr(&args[0].value)?;
            let func = match method {
                "_format" => "zap_uuid_format",
                "_hex" => "zap_uuid_hex",
                _ => "zap_uuid_urn",
            };
            call_ret(lowerer, func, vec![raw], Ty::Ptr, false)
        }
        "_version" | "_variant" => {
            if args.len() != 1 {
                return Err(err("uuid internal accessor takes one argument"));
            }
            let (raw, _) = lowerer.expr(&args[0].value)?;
            let func = if method == "_version" {
                "zap_uuid_version"
            } else {
                "zap_uuid_variant"
            };
            call_ret(lowerer, func, vec![raw], Ty::I64, false)
        }
        _ => return Ok(None),
    }))
}

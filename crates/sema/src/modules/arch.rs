//! `arch.x86_64` / `arch.aarch64` modules — sema facet (I#139 Tier 2, #962,
//! D-0199).
//!
//! The curated, architecture-specific low-level-performance escape hatch: the
//! CPU intrinsics exposed by their familiar C names, each lowering to the
//! matching hardware instruction (see the lowering/codegen facets). Both arches
//! expose the same v1 surface, so one `check` serves both registry entries; the
//! per-arch difference is only in the instruction each op lowers to.
//!
//! v1 surface (D-0079 §1, the minimal memory-safe core):
//!   `crc32c_u8(crc: uint, v: uint)  -> uint32`   CRC32C / Castagnoli step
//!   `crc32c_u16(crc: uint, v: uint) -> uint32`
//!   `crc32c_u32(crc: uint, v: uint) -> uint32`
//!   `crc32c_u64(crc: uint, v: uint) -> uint64`
//!   `popcount_u32(v: uint)          -> uint32`   population count
//!   `popcount_u64(v: uint)          -> uint32`
//!   `prefetch(buf: bytes|str)                 `  locality hint (never faults)
//!   `prefetch(buf: bytes|str, locality: int)  `  locality 0 (NTA) .. 3 (T0)
//!
//! All three op families are **memory-safe** — none performs a load or store the
//! compiler cannot see — so none is behind an `unsafe` gate (Zap has none yet;
//! the over-reading SSE4.2 string compares that would need one are Phase 4,
//! deferred). `crc32c_*` is hardware CRC32C (Castagnoli, poly 0x1EDC6F41), **not**
//! the ISO/zlib CRC32 that `zlib.crc32`/gzip use — hence the `crc32c` name,
//! never a bare `crc32` (D-0079 §6.6).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{expr_span, Checker, Ty};

/// `arch.x86_64` — x86-64 intrinsics (SSE4.2 `crc32`, `POPCNT`, `PREFETCHh`).
pub(super) const MODULE_X86_64: ModuleCheck = ModuleCheck {
    name: "arch.x86_64",
    check,
};

/// `arch.aarch64` — aarch64 intrinsics (CRC32C ext, `CNT`, `PRFM`).
pub(super) const MODULE_AARCH64: ModuleCheck = ModuleCheck {
    name: "arch.aarch64",
    check,
};

/// A 2-arg intrinsic whose arguments must both be integers (`crc`, `value`).
fn int2(checker: &mut Checker, module: &str, method: &str, args: &[Arg], span: Span, result: Ty) {
    if args.len() != 2 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "{module}.{method}() takes 2 arguments (crc, value), found {}",
                args.len()
            ),
            span,
        );
    }
    for a in args {
        let t = checker.infer(&a.value);
        if !t.is_int() && t != Ty::Unknown {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "{module}.{method}() expects integer arguments, found {}",
                    t.display()
                ),
                expr_span(&a.value),
            );
        }
    }
    let _ = result;
}

/// A 1-arg intrinsic whose argument must be an integer (`popcount_*`).
fn int1(checker: &mut Checker, module: &str, method: &str, args: &[Arg], span: Span) {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!("{module}.{method}() takes 1 argument, found {}", args.len()),
            span,
        );
    }
    for a in args {
        let t = checker.infer(&a.value);
        if !t.is_int() && t != Ty::Unknown {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "{module}.{method}() expects an integer argument, found {}",
                    t.display()
                ),
                expr_span(&a.value),
            );
        }
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    // The module name for diagnostics is the method's family; both arch modules
    // share this `check`, so report the generic `arch` surface name.
    let module = "arch";
    Some(match method {
        "crc32c_u8" | "crc32c_u16" | "crc32c_u32" => {
            int2(checker, module, method, args, span, Ty::Uint32);
            Ty::Uint32
        }
        "crc32c_u64" => {
            int2(checker, module, method, args, span, Ty::Uint64);
            Ty::Uint64
        }
        "popcount_u32" | "popcount_u64" => {
            int1(checker, module, method, args, span);
            Ty::Uint32
        }
        "prefetch" => {
            // `prefetch(buf[, locality])`: buf is a bytes/str buffer (a managed
            // reference, so the compiler owns a valid pointer — the hint never
            // faults); the optional locality is an int 0 (NTA) .. 3 (T0).
            if args.is_empty() || args.len() > 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "{module}.prefetch() takes 1 or 2 arguments (buf[, locality]), found {}",
                        args.len()
                    ),
                    span,
                );
            }
            if let Some(a) = args.first() {
                let t = checker.infer(&a.value);
                if t != Ty::Bytes && t != Ty::Str && t != Ty::Unknown {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "{module}.prefetch() expects a bytes or str buffer, found {}",
                            t.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
            if let Some(a) = args.get(1) {
                let t = checker.infer(&a.value);
                if !t.is_int() && t != Ty::Unknown {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "{module}.prefetch() locality must be an integer 0..3, found {}",
                            t.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
            Ty::Unit
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use crate::check_str;
    use zapc_diag::ErrorCode;

    /// Type-check `src`, asserting it lexes+parses cleanly, and return the type
    /// errors.
    fn terrs(src: &str) -> Vec<crate::TypeError> {
        let (terrs, perrs, lerrs) = check_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        terrs
    }

    fn has(src: &str, code: ErrorCode) -> bool {
        terrs(src).iter().any(|e| e.code == code)
    }

    #[test]
    fn well_formed_arch_calls_typecheck() {
        // The aliased import resolves `x86` -> the `arch.x86_64` registry entry,
        // so every v1 intrinsic type-checks. (sema does not arch-gate the import;
        // that is the driver's job — see the modlink off-target tests.)
        let src = "\
import arch.x86_64 as x86

def main():
    auto a = x86.crc32c_u8(0, 97)
    auto b = x86.crc32c_u16(a, 258)
    auto c = x86.crc32c_u32(b, 42)
    auto d = x86.crc32c_u64(0, 42)
    auto n = x86.popcount_u32(255)
    auto m = x86.popcount_u64(1023)
    x86.prefetch(\"buffer\")
    x86.prefetch(\"buffer\", 3)
    print(a)
    print(d)
    print(n)
    print(m)
";
        assert!(terrs(src).is_empty(), "unexpected errors: {:?}", terrs(src));
    }

    #[test]
    fn aarch64_alias_resolves_the_same_surface() {
        let src = "\
import arch.aarch64 as a

def main():
    auto c = a.crc32c_u64(0, 7)
    print(c)
";
        assert!(terrs(src).is_empty(), "unexpected errors: {:?}", terrs(src));
    }

    #[test]
    fn bare_last_segment_name_without_alias() {
        // `import arch.x86_64` (no `as`) binds the last path segment as the name.
        let src = "\
import arch.x86_64

def main():
    auto n = x86_64.popcount_u64(255)
    print(n)
";
        assert!(terrs(src).is_empty(), "unexpected errors: {:?}", terrs(src));
    }

    #[test]
    fn crc32c_rejects_non_integer_arg() {
        let src = "\
import arch.x86_64 as x86

def main():
    auto c = x86.crc32c_u32(\"nope\", 1)
    print(c)
";
        assert!(has(src, ErrorCode::ZAP4024), "errors: {:?}", terrs(src));
    }

    #[test]
    fn crc32c_rejects_wrong_arity() {
        let src = "\
import arch.x86_64 as x86

def main():
    auto c = x86.crc32c_u32(1)
    print(c)
";
        assert!(has(src, ErrorCode::ZAP4025), "errors: {:?}", terrs(src));
    }

    #[test]
    fn prefetch_rejects_non_buffer() {
        let src = "\
import arch.x86_64 as x86

def main():
    x86.prefetch(5)
";
        assert!(has(src, ErrorCode::ZAP4024), "errors: {:?}", terrs(src));
    }

    #[test]
    fn prefetch_rejects_non_integer_locality() {
        let src = "\
import arch.x86_64 as x86

def main():
    x86.prefetch(\"buf\", \"hot\")
";
        assert!(has(src, ErrorCode::ZAP4024), "errors: {:?}", terrs(src));
    }

    #[test]
    fn unknown_arch_method_is_rejected() {
        // An unknown method on a resolved arch module is a no-such-member error.
        let src = "\
import arch.x86_64 as x86

def main():
    auto c = x86.pdep_u64(1, 2)
    print(c)
";
        assert!(
            !terrs(src).is_empty(),
            "expected an error for unknown arch method"
        );
    }

    #[test]
    fn hash_crc32c_typechecks_and_rejects_bad_arg() {
        let ok = "\
def main():
    auto c = hash.crc32c(\"123456789\")
    print(c)
";
        assert!(terrs(ok).is_empty(), "unexpected errors: {:?}", terrs(ok));
        let bad = "\
def main():
    auto c = hash.crc32c(42)
    print(c)
";
        assert!(has(bad, ErrorCode::ZAP4024), "errors: {:?}", terrs(bad));
    }
}

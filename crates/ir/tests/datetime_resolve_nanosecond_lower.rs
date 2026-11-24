//! `datetime.resolve(tz, …, fold=, nanosecond=)` — lowering (ZIR) checks for
//! #1361.
//!
//! Host-only (no LLVM): `lower_module` runs monomorphization + lowering. These
//! pin that `resolve` threads a `nanosecond=` keyword through to the DateTime's
//! fractional field (index `F_NANOSECOND == 9`) instead of the historical
//! hard-coded zero, while still emitting the `zap_tz_resolve_local` offset
//! resolution for the chosen `fold`. The LLVM CI golden
//! (`datetime_resolve_fraction`) then exercises the offsets + digits end-to-end.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

/// Mirror of `crates/ir/src/lower/modules/datetime.rs`'s `F_NANOSECOND`: the
/// DateTime struct's fractional-second slot (#1148).
const F_NANOSECOND: u32 = 9;

fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

fn called_symbols(m: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call { func, .. } | Inst::Invoke { func, .. } = inst {
                    out.push(func.clone());
                }
            }
        }
    }
    out
}

fn has_call(m: &Module, sym: &str) -> bool {
    called_symbols(m).iter().any(|s| s == sym)
}

/// True iff some `FieldSet` into slot `F_NANOSECOND` is fed by an `IConst`
/// carrying exactly `want` — i.e. the `nanosecond=` literal reached the
/// fractional field rather than being discarded for a constant zero.
fn fraction_field_is(m: &Module, want: i64) -> bool {
    for f in &m.funcs {
        // Map every IConst-defined value to its literal in this function.
        let mut consts: std::collections::HashMap<u32, i64> = std::collections::HashMap::new();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::IConst { dest, val } = inst {
                    consts.insert(*dest, *val);
                }
            }
        }
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::FieldSet { idx, val, .. } = inst {
                    if *idx == F_NANOSECOND && consts.get(val) == Some(&want) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

const HDR: &str = "import datetime\n\ndef main():\n";

#[test]
fn nanosecond_keyword_reaches_the_fraction_field() {
    // The headline shape: an explicit fold AND a nine-digit fraction together.
    // The offset is still zone-resolved (zap_tz_resolve_local) and the fraction
    // literal lands in F_NANOSECOND.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.zone(\"America/New_York\")\n    \
         auto late = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, fold=1, nanosecond=123456789)\n"
    ));
    assert!(
        has_call(&m, "zap_tz_resolve_local"),
        "resolve must still zone-resolve the offset; called: {:?}",
        called_symbols(&m)
    );
    assert!(
        fraction_field_is(&m, 123456789),
        "nanosecond=123456789 must reach F_NANOSECOND (idx {F_NANOSECOND})"
    );
}

#[test]
fn nanosecond_keyword_standalone_without_fold() {
    // nanosecond= on its own (fold defaults to 0) still populates the fraction.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, nanosecond=500)\n"
    ));
    assert!(fraction_field_is(&m, 500));
}

#[test]
fn positional_fold_coexists_with_keyword_nanosecond() {
    // A positional 8th fold and a keyword nanosecond do not collide: fold is the
    // offset selector, the fraction still reaches F_NANOSECOND.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, 1, nanosecond=999999999)\n"
    ));
    assert!(has_call(&m, "zap_tz_resolve_local"));
    assert!(fraction_field_is(&m, 999999999));
}

#[test]
fn fixed_offset_zone_takes_a_fraction() {
    // A fixed-offset zone (never a gap/overlap) still carries a fraction.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.fixed_offset(-300)\n    \
         auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0, nanosecond=1)\n"
    ));
    assert!(fraction_field_is(&m, 1));
}

#[test]
fn omitted_nanosecond_keeps_whole_seconds() {
    // Without nanosecond= the fraction is a constant 0 — existing behaviour and
    // existing call sites are untouched.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, fold=1)\n"
    ));
    assert!(
        fraction_field_is(&m, 0),
        "an omitted nanosecond must still zero F_NANOSECOND"
    );
}

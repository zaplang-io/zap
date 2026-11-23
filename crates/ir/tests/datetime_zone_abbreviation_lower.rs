//! `datetime.zone_abbreviation(dt) -> Optional[str]` — lowering (ZIR) checks
//! for #1345.
//!
//! Host-only (no LLVM): `lower_module` runs monomorphization + lowering, so
//! these pin the mono → lower wiring that the LLVM CI golden
//! (`datetime_zone_abbreviation`) then exercises end-to-end. In particular they
//! prove the mono hook requests the concrete `Optional$str` instance and encodes
//! it into the call symbol (`zone_abbreviation$Optional$str`), and that lowering
//! resolves that instance and emits the `zap_tz_abbr_at_id` runtime call —
//! without which lowering would raise the "not instantiated" internal error.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

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
                match inst {
                    Inst::Call { func, .. } | Inst::Invoke { func, .. } => out.push(func.clone()),
                    _ => {}
                }
            }
        }
    }
    out
}

fn has(m: &Module, sym: &str) -> bool {
    called_symbols(m).iter().any(|s| s == sym)
}

const HDR: &str = "import datetime\n\ndef main():\n";

#[test]
fn named_zone_value_lowers_to_the_abbr_runtime_call() {
    // A named-zone value: the call must lower (mono instantiates Optional$str,
    // lowering resolves it) and emit the abbreviation runtime call.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0)\n    \
         Optional[str] a = datetime.zone_abbreviation(d)\n"
    ));
    assert!(
        has(&m, "zap_tz_abbr_at_id"),
        "expected the zone_abbreviation lowering to call zap_tz_abbr_at_id; \
         called: {:?}",
        called_symbols(&m)
    );
}

#[test]
fn auto_binding_result_still_lowers() {
    // Without an annotated sink the result type is still `Optional[str]` (the
    // payload is fixed), so the mono hook fires and lowering resolves the
    // instance from the mono-encoded symbol.
    lower_ok(&format!(
        "{HDR}    \
         auto z = datetime.zone(\"Europe/Paris\")\n    \
         auto d = datetime.resolve(z, 2024, 7, 15, 12, 0, 0)\n    \
         auto a = datetime.zone_abbreviation(d)\n"
    ));
}

#[test]
fn naive_and_fixed_offset_values_lower() {
    // A naive value and a fixed-offset value both lower (they yield None at
    // runtime); the point here is that the lowering path is exercised and clean.
    let m = lower_ok(&format!(
        "{HDR}    \
         auto t = datetime.from_timestamp(0)\n    \
         Optional[str] a = datetime.zone_abbreviation(t)\n    \
         auto f = datetime.astimezone(t, datetime.fixed_offset(330))\n    \
         Optional[str] b = datetime.zone_abbreviation(f)\n"
    ));
    assert!(has(&m, "zap_tz_abbr_at_id"));
}

#[test]
fn abbreviation_is_matchable_as_optional_str() {
    // The result threads through the Optional surface — a `match` over
    // Some/None lowers cleanly, confirming the value carries the Optional type.
    lower_ok(
        "import datetime\n\n\
         def show(Optional[str] a) -> str:\n    \
         match a:\n        \
         case Optional.Some(v):\n            \
         return v\n        \
         case Optional.None:\n            \
         return \"<none>\"\n\n\
         def main():\n    \
         auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 7, 15, 12, 0, 0)\n    \
         auto s = show(datetime.zone_abbreviation(d))\n",
    );
}

//! #1329: matching an `Optional[T]` (or any generic enum) returned by a method
//! call on a `for`-loop receiver must keep the generic enum's instance so the
//! match pattern pins its variant — including when the receiver is iterated from
//! a `list`-typed struct FIELD.
//!
//! Before the fix, the monomorphizer declared a `for`-loop variable as
//! `Unknown` (unlike a comprehension clause, which binds the iterable's element
//! type since #1131). So `counter.collect()` on the loop variable could not be
//! typed, its declared `Optional[Point]` return was lost, and lowering the
//! `match` failed with ZAP6001 "cannot determine the instance of generic enum
//! 'Optional' for this match pattern". The loop variable now binds the
//! iterable's element type (`Mono::iter_elem`), the same way a comprehension
//! clause does, so the monomorphizer's own inferer types the whole chain.
//!
//! These tests run the real lowering pass (mono + AST→ZIR) **without** sema's
//! `TypeMap` — proving mono's own inferer closes the gap, not the #1167 backfill
//! — so they guard the fix wherever `cargo test` runs (host-only, no LLVM). The
//! Some/None execution and the otel #3 meter shape are covered end-to-end by the
//! `for_optional_match_field_1329` golden fixture.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Lower `src` (no sema types), asserting it parses and lowers with no errors.
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
}

/// Lower `src` (no sema types), asserting lowering is rejected with a ZAP6001
/// whose message is the generic-enum match-pattern instance error.
fn lower_err_generic_enum_match(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(
        errs.iter().any(|e| e.message.contains(
            "cannot determine the instance of generic enum 'Optional' for this match pattern"
        )),
        "expected the generic-enum match-pattern ZAP6001, got: {errs:?}"
    );
}

// --- The failing form: a method returning `Optional[T]` on a loop receiver
//     iterated from a `list`-typed struct FIELD. ---

const PRELUDE: &str = "struct Point:\n\
    \x20   int64 value\n\
struct Counter:\n\
    \x20   def collect(self) -> Optional[Point]:\n\
    \x20       return Optional.Some(Point(1))\n";

#[test]
fn field_list_loop_match_lowers() {
    // The issue's exact reduction.
    lower_ok(&format!(
        "{PRELUDE}\
        struct Meter:\n\
        \x20   list[Counter] counters\n\
        \x20   def check(self):\n\
        \x20       for counter in self.counters:\n\
        \x20           match counter.collect():\n\
        \x20               case Optional.Some(point):\n\
        \x20                   assert point.value == 1\n\
        \x20               case Optional.None:\n\
        \x20                   pass\n"
    ));
}

#[test]
fn field_list_loop_only_some_arm_lowers() {
    // Both variant arms must lower; here the None arm is elided to prove the
    // Some arm alone also pins the instance (not an exhaustiveness artifact).
    lower_ok(&format!(
        "{PRELUDE}\
        struct Meter:\n\
        \x20   list[Counter] counters\n\
        \x20   def check(self):\n\
        \x20       for counter in self.counters:\n\
        \x20           match counter.collect():\n\
        \x20               case Optional.Some(point):\n\
        \x20                   assert point.value == 1\n\
        \x20               case _:\n\
        \x20                   pass\n"
    ));
}

// --- Forms that already worked: lock them in so they never regress. ---

#[test]
fn local_list_loop_match_lowers() {
    lower_ok(&format!(
        "{PRELUDE}\
        def check():\n\
        \x20   list[Counter] counters = [Counter()]\n\
        \x20   for counter in counters:\n\
        \x20       match counter.collect():\n\
        \x20           case Optional.Some(point):\n\
        \x20               assert point.value == 1\n\
        \x20           case Optional.None:\n\
        \x20               pass\n"
    ));
}

#[test]
fn local_receiver_match_lowers() {
    lower_ok(&format!(
        "{PRELUDE}\
        def check():\n\
        \x20   Counter single = Counter()\n\
        \x20   match single.collect():\n\
        \x20       case Optional.Some(point):\n\
        \x20           assert point.value == 1\n\
        \x20       case Optional.None:\n\
        \x20           pass\n"
    ));
}

#[test]
fn field_index_receiver_match_lowers() {
    // Indexing the field list (no loop) already worked — the regression was
    // specific to loop iteration. Keep it covered as the contrast case.
    lower_ok(&format!(
        "{PRELUDE}\
        struct Meter:\n\
        \x20   list[Counter] counters\n\
        \x20   def check(self):\n\
        \x20       match self.counters[0].collect():\n\
        \x20           case Optional.Some(point):\n\
        \x20               assert point.value == 1\n\
        \x20           case Optional.None:\n\
        \x20               pass\n"
    ));
}

// --- Wall off the general element-type binding path, not the single
//     `Optional[Point]` instance. ---

#[test]
fn field_list_loop_different_optional_instance_lowers() {
    // A different `Optional[T]` (`str`) from a different instrument, same
    // field-loop shape: the general inference path, not a Point special case.
    lower_ok(
        "struct Label:\n\
        \x20   str name\n\
        \x20   def read(self) -> Optional[str]:\n\
        \x20       return Optional.Some(self.name)\n\
        struct Meter:\n\
        \x20   list[Label] labels\n\
        \x20   def check(self):\n\
        \x20       for label in self.labels:\n\
        \x20           match label.read():\n\
        \x20               case Optional.Some(name):\n\
        \x20                   print(name)\n\
        \x20               case Optional.None:\n\
        \x20                   pass\n",
    );
}

#[test]
fn otel_meter_shape_lowers() {
    // zaplang-io/otel#3: a meter iterates its registered instruments (a `list`
    // FIELD) and matches each instrument's optional observation.
    lower_ok(
        "struct Observation:\n\
        \x20   int64 value\n\
        struct Instrument:\n\
        \x20   int64 last\n\
        \x20   def observe(self) -> Optional[Observation]:\n\
        \x20       if self.last < 0:\n\
        \x20           return Optional.None\n\
        \x20       return Optional.Some(Observation(self.last))\n\
        struct Meter:\n\
        \x20   list[Instrument] instruments\n\
        \x20   def collect(self) -> int64:\n\
        \x20       int64 total = 0\n\
        \x20       for inst in self.instruments:\n\
        \x20           match inst.observe():\n\
        \x20               case Optional.Some(obs):\n\
        \x20                   total = total + obs.value\n\
        \x20               case Optional.None:\n\
        \x20                   total = total + 0\n\
        \x20       return total\n",
    );
}

#[test]
fn nested_field_list_loops_lower() {
    // A `list` field of a struct that itself has a `list` field whose elements
    // return `Optional[T]`: nested field-loops must each bind their element
    // type so the inner `match` pins the instance.
    lower_ok(&format!(
        "{PRELUDE}\
        struct Group:\n\
        \x20   list[Counter] counters\n\
        struct Meter:\n\
        \x20   list[Group] groups\n\
        \x20   def check(self):\n\
        \x20       for group in self.groups:\n\
        \x20           for counter in group.counters:\n\
        \x20               match counter.collect():\n\
        \x20                   case Optional.Some(point):\n\
        \x20                       assert point.value == 1\n\
        \x20                   case Optional.None:\n\
        \x20                       pass\n"
    ));
}

// --- Fail-closed: the diagnostic stays reachable for a genuinely
//     un-annotatable loop element (#1190 boundary). ---

#[test]
fn opaque_loop_element_still_reports_zap6001() {
    // Iterating a `py` handle yields an opaque element the monomorphizer cannot
    // type; matching a generic-enum pattern on it must still be rejected with
    // the match-pattern ZAP6001 (not silently mis-instantiated).
    lower_err_generic_enum_match(
        "def f(py obj):\n\
        \x20   for v in obj:\n\
        \x20       match v:\n\
        \x20           case Optional.Some(x):\n\
        \x20               print(x)\n\
        \x20           case Optional.None:\n\
        \x20               pass\n",
    );
}

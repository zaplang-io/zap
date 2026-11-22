//! User-defined associated functions on struct/enum types — lowering (ZIR)
//! checks for #1260.
//!
//! A receiverless `def` (its first parameter is not `self`) is an associated
//! function, callable only as `Type.func(...)`. In ZIR it lowers like a
//! namespaced free function `Type__func` with NO receiver: the call site emits a
//! plain `Call` to `Type__func` whose argument values are the call's arguments
//! (no `self` prepended), and the function itself is emitted with
//! `self_param == false` so its first parameter uses the ordinary by-value ABI
//! (a by-reference receiver ABI would mismatch the no-receiver call site).
//!
//! These are host-only lowering checks (no LLVM); the end-to-end runtime values
//! are covered by the `assoc_functions` golden on the LLVM CI matrix.

use zapc_ir::{lower_module, Func, Inst, Module};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, returning the ZIR.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

fn func<'m>(m: &'m Module, name: &str) -> &'m Func {
    m.funcs
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function `{name}` in module; have: {:?}", names(m)))
}

fn names(m: &Module) -> Vec<&str> {
    m.funcs.iter().map(|f| f.name.as_str()).collect()
}

/// Does any function body emit a `Call`/`Invoke` to `sym`?
fn calls(m: &Module, sym: &str) -> bool {
    m.funcs.iter().any(|f| {
        f.blocks.iter().any(|b| {
            b.insts.iter().any(|inst| {
                matches!(inst, Inst::Call { func, .. } | Inst::Invoke { func, .. } if func == sym)
            })
        })
    })
}

const MESSAGE: &str = "struct Message:\n    \
    int64 id\n    \
    bytes payload\n    \
    def query(int64 id) -> Message:\n        \
        return Message(id, b\"\")\n    \
    def from_wire(bytes data) -> Message:\n        \
        return Message(int64(data[0]), data)\n    \
    def to_wire(self) -> bytes:\n        \
        return self.payload\n";

#[test]
fn associated_function_is_emitted_as_a_free_function_with_no_receiver() {
    let m = lower_ok(&format!(
        "{MESSAGE}def main():\n    auto a = Message.query(42)\n    print(a.id)\n"
    ));
    // The associated function is a function named `Message__query`.
    let q = func(&m, "Message__query");
    // It has NO by-reference receiver: its first (and only) parameter is the
    // ordinary by-value `int64 id`.
    assert!(
        !q.self_param,
        "an associated function must lower with self_param == false"
    );
    assert_eq!(q.params.len(), 1, "query takes exactly one real parameter");
    // The call site dispatches to it directly.
    assert!(
        calls(&m, "Message__query"),
        "Message.query(42) must emit a Call to Message__query"
    );
}

#[test]
fn instance_method_keeps_its_receiver() {
    // The coexisting instance method `to_wire` still lowers WITH a receiver, so
    // the associated-function change does not disturb ordinary methods.
    let m = lower_ok(&format!(
        "{MESSAGE}def main():\n    \
         auto a = Message.query(1)\n    \
         auto w = a.to_wire()\n    \
         print(len(w))\n"
    ));
    let tw = func(&m, "Message__to_wire");
    assert!(tw.self_param, "an instance method keeps self_param == true");
    // `self` plus zero declared parameters.
    assert_eq!(tw.params.len(), 1, "to_wire has just the self receiver");
}

#[test]
fn associated_call_with_keyword_and_default_assembly() {
    // A keyword argument at the call site assembles through the free-function
    // path (no receiver to drop), so the argument lands in its parameter slot.
    let m = lower_ok(&format!(
        "{MESSAGE}def main():\n    auto a = Message.query(id=7)\n    print(a.id)\n"
    ));
    assert!(calls(&m, "Message__query"));
}

#[test]
fn multi_arg_associated_mixed_positional_keyword() {
    let m = lower_ok(
        "struct Point:\n    int64 x\n    int64 y\n    \
         def at(int64 x, int64 y) -> Point:\n        return Point(x, y)\n\
         def main():\n    auto p = Point.at(1, y=2)\n    print(p.x + p.y)\n",
    );
    let at = func(&m, "Point__at");
    assert!(!at.self_param);
    assert_eq!(at.params.len(), 2);
    assert!(calls(&m, "Point__at"));
}

#[test]
fn enum_associated_function_lowers_without_a_receiver() {
    // A receiverless `def` on an enum lowers like a namespaced free function too.
    let m = lower_ok(
        "enum Dir:\n    NORTH\n    SOUTH\n    \
         def south() -> Dir:\n        return Dir.SOUTH\n\
         def main():\n    auto d = Dir.south()\n    print(d == Dir.SOUTH)\n",
    );
    let s = func(&m, "Dir__south");
    assert!(!s.self_param, "enum associated function has no receiver");
    assert!(calls(&m, "Dir__south"));
}

#[test]
fn enum_variant_construction_still_lowers() {
    // Regression: a tier-3 variant construction `Shape.Circle(2.0)` (D-0046) must
    // still lower to variant construction, alongside an enum associated function.
    let m = lower_ok(
        "enum Shape:\n    Circle(f64 r)\n    Square(f64 s)\n    \
         def unit() -> Shape:\n        return Shape.Circle(1.0)\n\
         def main():\n    \
         auto c = Shape.Circle(2.0)\n    \
         auto u = Shape.unit()\n    \
         match c:\n        \
             case Shape.Circle(r):\n            print(r)\n        \
             case Shape.Square(s):\n            print(s)\n    \
         match u:\n        \
             case Shape.Circle(r):\n            print(r)\n        \
             case Shape.Square(s):\n            print(s)\n",
    );
    // The associated function is emitted and called.
    assert!(calls(&m, "Shape__unit"));
    // `Shape.Circle(2.0)` did NOT turn into a `Shape__Circle` function call (it is
    // variant construction, not an associated function).
    assert!(
        !calls(&m, "Shape__Circle"),
        "variant construction must not dispatch as an associated function"
    );
}

#[test]
fn field_constructor_and_named_constructor_coexist() {
    let m = lower_ok(&format!(
        "{MESSAGE}def main():\n    \
         auto a = Message(1, b\"\")\n    \
         auto b = Message.query(2)\n    \
         print(a.id + b.id)\n"
    ));
    assert!(calls(&m, "Message__query"));
}

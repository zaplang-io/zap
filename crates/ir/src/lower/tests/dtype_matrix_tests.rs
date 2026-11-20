//! Construction/indexing/ops lower for every dtype in the matrix (#503 f32/i32,
//! #890 i8/i16 + u16/u32/u64). Host-only (AST->ZIR), so the dtype plumbing is a
//! walled garden independent of codegen.
use crate::lower::lower_module;

fn lowers_ok(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    let (_m, errs) = lower_module(&module);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
}

#[test]
fn every_dtype_constructs_indexes_and_reduces() {
    for (dt, lit) in [
        ("f32", "[1.0, 2.0, 3.0]"),
        ("f64", "[1.0, 2.0, 3.0]"),
        ("i8", "[1, 2, 3]"),
        ("i16", "[1, 2, 3]"),
        ("i32", "[1, 2, 3]"),
        ("i64", "[1, 2, 3]"),
        ("u8", "[1, 2, 3]"),
        ("u16", "[1, 2, 3]"),
        ("u32", "[1, 2, 3]"),
        ("u64", "[1, 2, 3]"),
        ("bool", "[True, False, True]"),
    ] {
        lowers_ok(&format!(
            "def main():\n    auto a = array[{dt}]({lit})\n    a[0] = a[1]\n    \
                 print(a[2], len(a), sum(a))\n"
        ));
        // N-D construction + multi-axis index for the same dtype.
        if dt != "bool" {
            lowers_ok(&format!(
                "def main():\n    auto m = array[{dt}]([[1, 2], [3, 4]])\n    \
                     print(m[1, 0], m.ndim, m.size)\n"
            ));
        }
    }
}

#[test]
fn wide_dtype_elementwise_and_broadcast_lower() {
    // Element-wise + scalar broadcast + array-array broadcast across new dtypes.
    lowers_ok(
        "def main():\n    auto a = array[f32]([[1.0, 2.0], [3.0, 4.0]])\n    \
             auto b = array[f32]([10.0, 20.0])\n    print(sum(a + b), sum(a * 2.0))\n",
    );
    lowers_ok(
        "def main():\n    auto a = array[i32]([1, 2, 3])\n    \
             auto b = array[u16]([4, 5, 6])\n    print(sum(a + b))\n",
    );
    lowers_ok(
        "def main():\n    auto a = array[u64]([1, 2])\n    \
             auto b = array[i64]([3, 4])\n    print(sum(a + b))\n",
    );
}

#[test]
fn bogus_dtype_is_rejected() {
    let (module, _p, _l) =
        zapc_syntax::parser::parse_str("def main():\n    auto a = array[f128]([1.0])\n");
    let (_m, errs) = lower_module(&module);
    assert!(!errs.is_empty(), "array[f128] should be rejected");
}

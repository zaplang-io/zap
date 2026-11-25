//! Host-only lowering tests (AST→ZIR), extracted from `lower.rs` (#1122).
//!
//! Each submodule is a self-contained `#[cfg(test)]` walled garden exercising
//! one feature area's lowering directly, independent of the sema gate and of
//! LLVM — so these run on the host with `cargo test -p zapc_ir`. They were
//! moved out of the bottom of `lower.rs` verbatim (no behavior change); the
//! only edit was repointing `use super::lower_module` to `use crate::lower::lower_module`.

mod constructor_tests;
mod division_tests;
mod dtype_matrix_tests;
mod float_collection_tests;
mod mask_tests;
mod math_array_tests;
mod matmul_tests;
mod nd_array_tests;
mod null_reference_tests;
mod reduction_tests;
mod shape_ops_tests;
mod strided_view_tests;

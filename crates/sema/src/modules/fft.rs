//! `fft` module — sema facet (#929, D-0187).
//!
//! Fast Fourier transforms over numeric arrays, backed by vendored PocketFFT
//! (the transform NumPy/SciPy use). Surface mirrors `numpy.fft`:
//!   fft/ifft, rfft/irfft, fft2/ifft2, fftn/ifftn  -> complex (rfft a half
//!   spectrum, irfft a real array); fftfreq/rfftfreq -> the sample frequencies.
//! An optional `norm=` ("backward" default / "ortho" / "forward") sets the
//! normalization; `irfft` takes an optional output length `n=`; the frequency
//! helpers take an optional sample spacing `d=`.
//!
//! Arrays type loosely (`Ty::Unknown`) in sema, like the rest of the array
//! surface, so the arguments are inferred (type-checked) but not shape- or
//! dtype-constrained here; the lowering emits the concrete complex/real result
//! and the runtime validates shapes.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "fft", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    let min = match method {
        "fft" | "ifft" | "fft2" | "ifft2" | "fftn" | "ifftn" | "rfft" | "irfft" | "fftfreq"
        | "rfftfreq" => 1,
        _ => return None,
    };
    // At least one positional argument (the array, or `n` for the freq helpers);
    // `norm=` / `n=` / `d=` are optional keywords the lowering interprets.
    let positional = args.iter().filter(|a| a.name.is_none()).count();
    if positional < min {
        checker.error(
            crate::ErrorCode::ZAP4025,
            format!("fft.{method}() requires at least {min} argument(s)"),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
    Some(Ty::Unknown)
}

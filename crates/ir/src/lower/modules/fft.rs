//! `fft` module — lowering facet (#929, D-0187).
//!
//! Emits the PocketFFT-backed `zap_fft_*` runtime calls (implemented in
//! `zaprt.c`, with the transform itself in the C++ `zapfft.cpp`). fft/ifft
//! transform the last axis, fft2/ifft2 the last two, fftn/ifftn all axes; each
//! returns a `complex` array. rfft -> complex half spectrum (1-D), irfft ->
//! real array (1-D). fftfreq/rfftfreq return `f64` sample-frequency arrays. The
//! optional `norm=` keyword ("backward"/"ortho"/"forward") lowers to an int code,
//! `irfft` takes an optional `n=` output length, and the freq helpers an optional
//! `d=` spacing. All transforms can raise a catchable ValueError (bad shape / no
//! backend), so they route through `throwing_call`.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "fft", lower };

/// The positional (unnamed) argument expressions, in order.
fn positional(args: &[ast::Arg]) -> Vec<&ast::Expr> {
    args.iter()
        .filter(|a| a.name.is_none())
        .map(|a| &a.value)
        .collect()
}

/// The `norm=` keyword -> a runtime code (0 backward / 1 ortho / 2 forward),
/// default backward. Must be a plain string literal.
fn norm_code(args: &[ast::Arg]) -> Result<i64, LowerError> {
    for a in args {
        if a.name.as_deref() == Some("norm") {
            if let ast::Expr::Str {
                value,
                fstring: false,
                ..
            } = &a.value
            {
                return match value.as_str() {
                    "backward" => Ok(0),
                    "ortho" => Ok(1),
                    "forward" => Ok(2),
                    _ => Err(err(
                        "fft norm= must be \"backward\", \"ortho\", or \"forward\"",
                    )),
                };
            }
            return Err(err("fft norm= must be a string literal"));
        }
    }
    Ok(0)
}

/// A named keyword argument's lowered (value, type), or `None` if absent.
fn kw_value(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    name: &str,
) -> Result<Option<(Value, Ty)>, LowerError> {
    for a in args {
        if a.name.as_deref() == Some(name) {
            return Ok(Some(lowerer.expr(&a.value)?));
        }
    }
    Ok(None)
}

/// The rank of a lowered array operand (default 1 if it typed loosely).
fn rank_of(t: Ty) -> u8 {
    match t {
        Ty::Array(_, nd) => nd,
        _ => 1,
    }
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // c2c transforms (fft/ifft/fft2/ifft2/fftn/ifftn): one array -> a complex
    // array of the same shape. The runtime picks the axes (last / last-two / all).
    let c2c = |lowerer: &mut FnLower, func: &str| -> Result<Option<(Value, Ty)>, LowerError> {
        let pos = positional(args);
        if pos.len() != 1 {
            return Err(err("fft transform expects one array argument (plus norm=)"));
        }
        let (av, at) = lowerer.expr(pos[0])?;
        let norm = norm_code(args)?;
        let normv = lowerer.iconst(Ty::I64, norm);
        let out = Ty::Array(Dtype::Complex, rank_of(at));
        let d = lowerer.new_value(out);
        lowerer.throwing_call(Some(d), func, vec![av, normv]);
        Ok(Some((d, out)))
    };
    match method {
        "fft" => c2c(lowerer, "zap_fft_fft"),
        "ifft" => c2c(lowerer, "zap_fft_ifft"),
        "fft2" => c2c(lowerer, "zap_fft_fft2"),
        "ifft2" => c2c(lowerer, "zap_fft_ifft2"),
        "fftn" => c2c(lowerer, "zap_fft_fftn"),
        "ifftn" => c2c(lowerer, "zap_fft_ifftn"),
        // rfft: real 1-D -> complex half spectrum.
        "rfft" => {
            let pos = positional(args);
            if pos.len() != 1 {
                return Err(err("fft.rfft expects (a, norm=...)"));
            }
            let (av, _) = lowerer.expr(pos[0])?;
            let normv = lowerer.iconst(Ty::I64, norm_code(args)?);
            let out = Ty::Array(Dtype::Complex, 1);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_fft_rfft", vec![av, normv]);
            Ok(Some((d, out)))
        }
        // irfft: complex 1-D -> real array of length n (default 2*(len-1)).
        "irfft" => {
            let pos = positional(args);
            if pos.len() != 1 {
                return Err(err("fft.irfft expects (a, n=..., norm=...)"));
            }
            let (av, _) = lowerer.expr(pos[0])?;
            let n = match kw_value(lowerer, args, "n")? {
                Some((v, t)) => lowerer.coerce(v, t, Ty::I64),
                None => lowerer.iconst(Ty::I64, 0), // 0 -> runtime default
            };
            let normv = lowerer.iconst(Ty::I64, norm_code(args)?);
            let out = Ty::Array(Dtype::F64, 1);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_fft_irfft", vec![av, n, normv]);
            Ok(Some((d, out)))
        }
        // fftfreq / rfftfreq: n bins (int), optional spacing d= (float, 1.0).
        "fftfreq" | "rfftfreq" => {
            let pos = positional(args);
            if pos.len() != 1 {
                return Err(err("fft.fftfreq expects (n, d=...)"));
            }
            let (nv, nt) = lowerer.expr(pos[0])?;
            let n = lowerer.coerce(nv, nt, Ty::I64);
            let d = match kw_value(lowerer, args, "d")? {
                Some((v, t)) => lowerer.coerce(v, t, Ty::F64),
                None => lowerer.fconst(Ty::F64, 1.0),
            };
            let func = if method == "fftfreq" {
                "zap_fft_fftfreq"
            } else {
                "zap_fft_rfftfreq"
            };
            let out = Ty::Array(Dtype::F64, 1);
            let dest = lowerer.new_value(out);
            lowerer.throwing_call(Some(dest), func, vec![n, d]);
            Ok(Some((dest, out)))
        }
        _ => Ok(None),
    }
}

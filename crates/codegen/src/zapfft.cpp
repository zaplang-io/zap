// FFT backend (#929): a thin extern-"C" wrapper over vendored PocketFFT
// (third_party/pocketfft/pocketfft_hdronly.h, the transform NumPy/SciPy use).
//
// This is the only C++ translation unit in the runtime. It is deliberately
// self-contained — it touches no Zap runtime type, only raw contiguous `double`
// buffers (a complex value is two lanes, re then im) plus shape/axis arrays, so
// the C runtime (`zaprt.c`) owns all `ZapArray` marshalling and this unit owns
// only the math. It is compiled with `-fexceptions` and linked as a native
// object (not merged into the program's LTO bitcode), gated by `BackendUse::fft`,
// so only a program that imports `fft` pulls in this unit and the C++ stdlib.
//
// PocketFFT raises C++ exceptions (POCKETFFT_THROW) on a malformed transform; a
// C++ exception must not cross the extern-"C" boundary into C, so every entry
// point is wrapped in try/catch and returns 0 on success, 1 on failure (the C
// caller then raises a catchable Zap ValueError).

#include "pocketfft_hdronly.h"

#include <complex>
#include <cstddef>
#include <vector>

using pocketfft::shape_t;
using pocketfft::stride_t;
using cpx = std::complex<double>;

namespace {
// Row-major contiguous byte strides for `shape`, element size `elem`.
stride_t row_major_strides(const shape_t &shape, std::ptrdiff_t elem) {
  std::size_t n = shape.size();
  stride_t st(n);
  std::ptrdiff_t s = elem;
  for (std::size_t k = n; k-- > 0;) {
    st[k] = s;
    s *= static_cast<std::ptrdiff_t>(shape[k]);
  }
  return st;
}
} // namespace

extern "C" {

// Complex-to-complex transform over `axes` of an `ndim`-D contiguous complex
// array (in/out interleaved re,im). `forward` picks the sign; `fct` is the
// normalization scale the caller computed from `norm` and the transform size.
// in == out (in place) is allowed. fft / ifft / fft2 / ifft2 / fftn / ifftn all
// route here with a different `axes` set.
int zapfft_c2c(const double *in, double *out, const std::size_t *shape,
               std::size_t ndim, const std::size_t *axes, std::size_t naxes,
               int forward, double fct) {
  try {
    shape_t shp(shape, shape + ndim);
    shape_t ax(axes, axes + naxes);
    stride_t st = row_major_strides(shp, sizeof(cpx));
    pocketfft::c2c(shp, st, st, ax, forward != 0,
                   reinterpret_cast<const cpx *>(in),
                   reinterpret_cast<cpx *>(out), fct);
    return 0;
  } catch (...) {
    return 1;
  }
}

// Real-to-complex 1-D transform (rfft): `n` real inputs -> `n/2 + 1` complex
// outputs (the non-redundant half spectrum).
int zapfft_r2c(const double *in, double *out, std::size_t n, int forward,
               double fct) {
  try {
    shape_t shp{n};
    stride_t sin{static_cast<std::ptrdiff_t>(sizeof(double))};
    stride_t sout{static_cast<std::ptrdiff_t>(sizeof(cpx))};
    pocketfft::r2c(shp, sin, sout, std::size_t(0), forward != 0, in,
                   reinterpret_cast<cpx *>(out), fct);
    return 0;
  } catch (...) {
    return 1;
  }
}

// Complex-to-real 1-D transform (irfft): `n/2 + 1` complex inputs -> `n` real
// outputs. `n` (the real length) is supplied by the caller (it is ambiguous
// from the input alone).
int zapfft_c2r(const double *in, double *out, std::size_t n, int forward,
               double fct) {
  try {
    shape_t shp{n}; // the real (output) shape
    stride_t sin{static_cast<std::ptrdiff_t>(sizeof(cpx))};
    stride_t sout{static_cast<std::ptrdiff_t>(sizeof(double))};
    pocketfft::c2r(shp, sin, sout, std::size_t(0), forward != 0,
                   reinterpret_cast<const cpx *>(in), out, fct);
    return 0;
  } catch (...) {
    return 1;
  }
}

} // extern "C"

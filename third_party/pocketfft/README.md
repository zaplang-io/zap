# PocketFFT (vendored)

`pocketfft_hdronly.h` is the header-only C++ FFT library used by NumPy and SciPy.

- **Upstream:** https://github.com/mreineck/pocketfft (branch `cpp`)
- **License:** BSD-3-Clause (full text is retained at the top of the header)
- **Copyright:** Max-Planck-Society; Peter Bell; and the contributors named in
  the header.

Vendored for Zap's `import fft` module (#929, D-0187). The Zap runtime calls it
only through the thin extern-"C" wrapper `crates/codegen/src/zapfft.cpp`, which
is compiled and linked into a program only when it imports `fft`
(`BackendUse::fft`). The header is unmodified from upstream; update it by
replacing this file with a newer upstream revision and re-running the `fft`
golden / conformance tests.

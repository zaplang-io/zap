# Vendored NumPy ziggurat constants

`ziggurat_constants.h` is vendored verbatim from NumPy, used by the `random`
module's bit-exact `Generator` (ticket #930/#931, D-0189) to reproduce
`numpy.random.default_rng(...)`'s normal / exponential streams exactly.

- **Upstream**: https://github.com/numpy/numpy
- **File**: `numpy/random/src/distributions/ziggurat_constants.h`
- **Version**: `v2.5.4` (tag)
- **Retrieved**: 2026-11-15

The tables (`ki_double` / `wi_double` / `fi_double` for the normal ziggurat,
`ke_double` / `we_double` / `fe_double` for the exponential ziggurat, plus the
`ziggurat_nor_r` / `ziggurat_nor_inv_r` / `ziggurat_exp_r` scalars) are a fixed
part of NumPy's documented, stream-stable RNG; the `_float` tables are kept as
upstream ships them (unused for now — `f32` sampling is a follow-up).

## License

NumPy is distributed under the BSD 3-Clause License:

```
Copyright (c) 2005-2024, NumPy Developers.
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the NumPy Developers nor the names of any contributors
  may be used to endorse or promote products derived from this software without
  specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

The bit-exact distribution/sampling algorithms in
`crates/codegen/src/zaprandom.inc.c` follow NumPy's `distributions.c` /
`_generator.pyx` (same BSD-3 license) so that a seeded Zap `Generator`
reproduces NumPy's stream value-for-value.

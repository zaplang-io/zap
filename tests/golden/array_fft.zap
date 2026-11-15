# Golden fixture: the `fft` module (#929), vendored PocketFFT backend. 4-point
# transforms of an impulse / constant signal have exact ±1/±i twiddles, so every
# value here is exact; fftfreq/rfftfreq are exact rationals. fft/ifft/fft2 return
# complex arrays (a[i] is a complex scalar, rendered (re±imj)); irfft returns a
# real f64 array; the norm= keyword sets the scale. (Needs the FFT backend.)
import fft

def main():
    # fft of a unit impulse -> all ones.
    auto f = fft.fft(array[f64]([1.0, 0.0, 0.0, 0.0]))
    print("fft", str(f[0]), str(f[1]), str(f[2]), str(f[3]))  # (1+0j) x4

    # fft of a constant -> [4, 0, 0, 0].
    auto g = fft.fft(array[f64]([1.0, 1.0, 1.0, 1.0]))
    print("fftc", str(g[0]), str(g[1]))  # (4+0j) (0+0j)

    # round-trip ifft(fft(x)) == x.
    auto x = fft.ifft(f)
    print("ifft", str(x[0]), str(x[1]), str(x[2]), str(x[3]))  # (1+0j) (0+0j) (0+0j) (0+0j)

    # ortho normalization: fft / sqrt(4) = 0.5 per bin.
    auto o = fft.fft(array[f64]([1.0, 0.0, 0.0, 0.0]), norm="ortho")
    print("ortho", str(o[0]), str(o[1]))  # (0.5+0j) (0.5+0j)

    # rfft -> non-redundant half spectrum (3 complex); irfft back to the real signal.
    auto r = fft.rfft(array[f64]([1.0, 0.0, 0.0, 0.0]))
    print("rfft", len(r), str(r[0]), str(r[2]))  # 3 (1+0j) (1+0j)
    auto ir = fft.irfft(r, n=4)
    print("irfft", str(ir[0]), str(ir[1]), ir.ndim)  # 1.0 0.0 1

    # sample frequencies (exact rationals).
    auto fr = fft.fftfreq(4)
    print("freq", str(fr[0]), str(fr[1]), str(fr[2]), str(fr[3]))  # 0.0 0.25 -0.5 -0.25
    auto rf = fft.rfftfreq(4)
    print("rfreq", str(rf[0]), str(rf[1]), str(rf[2]), len(rf))  # 0.0 0.25 0.5 3

    # 2-D fft of a 2-D impulse -> all ones.
    auto m = fft.fft2(array[f64]([[1.0, 0.0], [0.0, 0.0]]))
    print("fft2", str(m[0, 0]), str(m[1, 1]))  # (1+0j) (1+0j)

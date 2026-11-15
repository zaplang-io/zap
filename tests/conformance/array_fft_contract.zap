# claim: ARR-21
# SPEC §15 (#929): `import fft` — FFTs over numeric arrays, vendored PocketFFT.
# fft/ifft (last axis), fft2/ifft2 (last two), fftn/ifftn (all axes) return
# complex arrays; rfft -> complex half spectrum (1-D), irfft -> real array;
# fftfreq/rfftfreq -> f64 sample frequencies. Optional norm= (backward default /
# ortho / forward); irfft takes n=, the freq helpers d=. Transforms of a 4-point
# impulse/constant are exact (±1/±i twiddles).
import fft

def main():
    auto f = fft.fft(array[f64]([1.0, 0.0, 0.0, 0.0]))
    print("fft", str(f[0]), str(f[1]), len(f))  # (1+0j) (1+0j) 4
    auto x = fft.ifft(f)
    print("ifft", str(x[0]), str(x[1]))  # (1+0j) (0+0j)
    auto r = fft.rfft(array[f64]([1.0, 0.0, 0.0, 0.0]))
    print("rfft", len(r), str(r[0]))  # 3 (1+0j)
    auto ir = fft.irfft(r, n=4)
    print("irfft", str(ir[0]), str(ir[1]))  # 1.0 0.0
    auto fr = fft.fftfreq(4)
    print("freq", str(fr[0]), str(fr[1]), str(fr[2]))  # 0.0 0.25 -0.5
    auto m = fft.fft2(array[f64]([[1.0, 0.0], [0.0, 0.0]]))
    print("fft2", str(m[0, 0]))  # (1+0j)

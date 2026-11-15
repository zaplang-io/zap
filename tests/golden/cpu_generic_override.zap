# Golden fixture: the ZAP_CPU=generic downgrade override (#943, epic #139). The
# sidecar `.env` forces the baseline clone on whatever hardware runs the test —
# the one knob that lets the CI / qemu matrix exercise the fallback path on
# capable silicon. After it, only the mandatory baseline survives, so every
# above-baseline feature reads false; and because the override is DOWNGRADE-only,
# the baseline itself is never stranded. Arch-independent: `avx2` (x86) and `sve`
# (aarch64) are both cleared where present and are non-names (false) on the other
# arch, so the output is identical on both.
def main():
    if sys.cpu_has("sse2") or sys.cpu_has("neon"):
        print("baseline_survives")
    if not sys.cpu_has("avx2"):
        print("no_avx2")
    if not sys.cpu_has("sve"):
        print("no_sve")

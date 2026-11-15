# Golden fixture: sys CPU-feature introspection (#943, ISA-acceleration epic
# #139). Env- and arch-independent: it asserts SHAPES, never a specific feature,
# so the output is identical on x86-64 and aarch64 CI boxes.
#   - every supported arch has its mandatory baseline (SSE2 on x86, NEON on arm),
#     so `cpu_has("sse2") or cpu_has("neon")` is always true;
#   - an unknown / arch-foreign name is false, never an error;
#   - cpu_source() is a non-empty detection-path label;
#   - cpu_sve_vl() is a non-negative byte count (0 when there is no SVE).
def main():
    if sys.cpu_has("sse2") or sys.cpu_has("neon"):
        print("has_baseline")
    if not sys.cpu_has("totally-made-up-feature"):
        print("unknown_is_false")
    if len(sys.cpu_source()) > 0:
        print("has_source")
    if sys.cpu_sve_vl() >= 0:
        print("sve_vl_nonneg")

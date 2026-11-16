# Golden fixture: the portable `arch.cpu_has` CPU-feature query (I#139, #963,
# D-0200). The sidecar `.env` sets ZAP_CPU=generic, so the cpu_features detector
# (#943) reports baseline-only and every x86-specific feature query is false on
# every CI box (on aarch64 these names are arch-foreign, also false) — giving
# deterministic, host-independent output. The query reuses the #943 detector
# (same semantics as `sys.cpu_has`) but validates the feature name at compile
# time, so a typo is a compile error rather than a silently-false query.
def main():
    print(arch.cpu_has("avx2"))
    print(arch.cpu_has("sse4.2"))
    print(arch.cpu_has("avx512f"))

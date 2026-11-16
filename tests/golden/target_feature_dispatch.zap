# Golden fixture: a user `@target_feature` function called through the safe
# idiom (I#139, #963, D-0200). `@target_feature("avx2")` (x86) / "neon" (aarch64)
# compiles the kernel with that feature enabled; it is unsafe to call directly,
# so the call sits inside an `unsafe:` block guarded by `arch.cpu_has(...)` — the
# programmer's assertion that the feature is present. The kernel is called only
# when the guard holds; when it does not, the portable `else` computes the same
# value. Either way the result is identical (49), so the output is the same on
# every CI box whether or not the host actually has the feature (no SIGILL). This
# exercises the @target_feature LLVM feature-stamp surviving the -O2/LTO pipeline.
when target.arch == .x86_64:
    @target_feature("avx2")
    def kernel(int64 n) -> int64:
        return n * n
elif target.arch == .aarch64:
    @target_feature("neon")
    def kernel(int64 n) -> int64:
        return n * n

def feat() -> str:
    when target.arch == .x86_64:
        return "avx2"
    elif target.arch == .aarch64:
        return "neon"
    else:
        return "default"

def main():
    if arch.cpu_has(feat()):
        unsafe:
            print(kernel(7))
    else:
        print(49)

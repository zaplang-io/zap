# Golden: whole-run CPU profiling via ZAP_PROFILE=cpu (ticket #78, D-0058).
#
# profile_cpu.env sets ZAP_PROFILE=cpu, so the profiler auto-starts once the
# scheduler exists (first spawn) and stops + flushes the folded profile at normal
# exit, and ZAP_PROFILE_OUT=/dev/null so no artifact file is left in the test
# dir. The workers run a tight CPU loop so the sampler has on-CPU stacks to
# capture. The printed total is DETERMINISTIC (the sum is independent of how many
# samples the profiler happened to take); the harness also checks that stderr
# carries the profiler's end-of-run banner. There is NO sample-count or timing
# assertion — sampling is non-deterministic, exactly like the GC goldens.
def busy(Atomic[int64] c, bytes wg):
    auto i = 0
    while i < 3000000:
        i = i + 1
    c.add(1)
    sync.done(wg)

def main() -> int64:
    Atomic[int64] c = Atomic(0)
    auto wg = sync.waitgroup()
    auto n = 8
    auto k = 0
    while k < n:
        sync.add(wg, 1)
        spawn busy(c, wg)
        k = k + 1
    sync.wait(wg)
    print(c.load())
    return 0

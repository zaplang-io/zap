# Golden: MAXPROCS is container-aware (#905, D-0176). Under a CFS CPU quota the
# node's online-core count is NOT the budget, so the default clamps DOWN to
# ceil(quota/period). maxprocs_cgroup_quota.env injects a 0.5-CPU quota via the
# ZAP_CPU_MAX seam (v2 "cpu.max" format: "50000 100000"), so the effective
# MAXPROCS is 1 — even on a multi-core CI runner, which is the bug this fixes
# (without the clamp it would report the runner's core count). No MAXPROCS env
# here, so the default path runs.
def main():
    print(runtime.maxprocs())

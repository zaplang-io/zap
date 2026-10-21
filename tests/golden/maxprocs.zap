# Golden: the MAXPROCS parallelism knob is honored (ticket #172). The env var
# `MAXPROCS` (see maxprocs.env) sets the worker-M count; `runtime.maxprocs()`
# reports the effective value. maxprocs.env pins it to 3, so the program prints
# 3 regardless of the host's hardware-thread count — deterministic under both
# collectors. (The knob dropped its old `ZAP_` prefix and GOMAXPROCS-analogue
# naming; it is simply `MAXPROCS`.)

def main():
    print(runtime.maxprocs())

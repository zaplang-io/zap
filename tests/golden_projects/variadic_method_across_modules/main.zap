# Cross-module variadic method call (#1218): the struct and its `bytes... keys`
# method are declared in `stats`; `main` constructs a value and calls the method
# with zero, one, and many arguments. Each call must build exactly one collected
# `list` argument (empty for the bare call) even though the callee lives in
# another module.
import stats

def main():
    stats.Client c = stats.make(100)
    print(c.record())
    print(c.record(b"a"))
    print(c.record(b"a", b"b", b"c"))

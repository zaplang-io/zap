# claim: CONV-2
# SPEC §9 (#456): an *uncaught* int(s) parse error leaves `main` through the
# exception machinery like any other uncaught exception — a clean
# `Uncaught exception: ...` line on stderr and exit status 1 (EXC-6 shape), not
# a process abort (which would print `zap: ...` and die on SIGABRT). This is the
# observable proof the failure is a catchable `ValueError`, not a fault.
def main():
    print("before")
    auto n = int("abc")
    print(str(n))

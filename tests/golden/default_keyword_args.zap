# Default arguments (D-0038) and keyword-argument reordering honored by codegen
# (I#298). Before the fix, any call that omitted a defaulted parameter or passed
# keyword arguments out of declaration order emitted an invalid LLVM module;
# only a fully-positional, in-declaration-order call worked. Each `print` below
# exercises one assembly path.

def f(str a, str b = "B", int64 c = 3) -> str:
    return a + b + str(c)

# No defaults: proves keyword matching/reordering independent of defaults.
def g(str a, str b, int64 c) -> str:
    return a + b + str(c)

def main():
    # --- still-working baselines: every parameter, in declaration order ------
    print(f("x", "Y", 9))  # xY9  (all positional)
    print(f(a="x", b="Y", c=9))  # xY9  (all by-name, in order)

    # --- omitted defaults ----------------------------------------------------
    print(f("x"))  # xB3  (both defaults)
    print(f("x", "Y"))  # xY3  (trailing default)

    # --- keyword fill skipping a middle default ------------------------------
    print(f("x", c=9))  # xB9  (b defaulted, c by name)
    print(f(a="z", c=5))  # zB5  (a by name, b defaulted, c by name)

    # --- out-of-order keywords, no defaults ----------------------------------
    print(g("x", c=9, b="Y"))  # xY9  (positional a, then c, b by name)
    print(g(c=9, a="x", b="Y"))  # xY9  (all by-name, fully scrambled)

    # --- compose: positional + out-of-order keyword + omitted default --------
    print(f("x", c=7))  # xB7

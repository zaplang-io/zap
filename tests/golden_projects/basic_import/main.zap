# The #283 repro: user code spanning two files. `a` resolves to `a.zap` in the
# project source root; `a.helper()` calls the imported function.
import a

def main():
    print(a.helper())

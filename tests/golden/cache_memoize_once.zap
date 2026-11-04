# @cache memoization (#666, D-0115, epic #661). A side effect in the body — the
# `computing` print — runs exactly ONCE across repeated identical calls: the
# generated module-level dict (real Zap, visible via `zap expand`) memoizes by
# the argument tuple. The assertion is the observable effect count (how many
# `computing` lines appear), which is deterministic on the single `main`
# coroutine and depends on neither timing nor heap bytes. A second distinct
# argument gets its own entry and so is computed once more.
@cache
def square(int64 n) -> int64:
    print("computing", n)
    return n * n

def main():
    print(square(3))
    print(square(3))
    print(square(3))
    print(square(4))
    print(square(4))

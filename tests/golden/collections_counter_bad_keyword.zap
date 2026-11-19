# I#972 (negative): `collections.Counter[K](...)` is sugar for the bare prelude
# `new_counter[K](...)`, whose only parameter is `items` — so an unknown keyword
# is a compile error, diagnosed on the desugared constructor exactly as the bare
# form would be. Locks in that the seam sugar does not invent parameters.
import collections

def main():
    auto c = collections.Counter[str](bogus=[])
    print(len(c))

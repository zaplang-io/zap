# E4 (#1429/#1444): shadow-prove that the built-in `@derive(Hash)` can be
# expressed as a library `@derivable` generator, WITHOUT replacing the built-in.
# A struct derives BOTH the built-in `Hash` (`__hash__`) and a library
# `StructHash` whose generator folds the field hashes with `hash_combine`. The
# shadow is byte-identical: same seed (0), same `zap_hash_combine` fold in
# declaration order, and `.__hash__()` lowers to the same per-field hash as the
# built-in — so `__hash__()` and `.structhash()` return the SAME value. Raw hash
# values are wyhash-seed-dependent per run, so this asserts built-in == shadow
# equality (and equal structs hash equal), never a fixed hash number. Built-in,
# auto-derive and prelude are untouched.
@derivable(StructHash)
def _derive_structhash[T](T value) -> int64:
    int64 h = 0
    comptime for f in reflect.type_info(T).fields:
        h = hash_combine(h, reflect.field(value, comptime f.index).__hash__())
    return h

interface StructHash:
    def structhash(self) -> int64

@derive(Hash, Eq, StructHash)
struct P:
    int64 x
    str s

def main():
    auto p = P(1, "a")
    auto q = P(2, "b")
    # built-in `__hash__` and the library shadow return the same value...
    print(p.__hash__() == p.structhash())
    print(q.__hash__() == q.structhash())
    # ...and the shadow is consistent with equality (equal structs hash equal).
    print(P(1, "a").structhash() == P(1, "a").structhash())
    print(P(1, "a").__hash__() == P(1, "a").structhash())

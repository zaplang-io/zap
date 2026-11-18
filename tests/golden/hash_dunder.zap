# Golden (#1064): the user-callable `__hash__()` on CONCRETE intrinsic receivers
# (str/bytes/int/bool/char/float) and the `hash_combine(acc, h)` builtin. The
# str/bytes hash is now seeded per process (wyhash), so this fixture prints only
# SEED-INDEPENDENT facts: booleans comparing hashes WITHIN a run, the seed-free
# scalar hashes (splitmix64), and that a hand-written struct `__hash__` built from
# field `__hash__()`s + `hash_combine` reproduces the `@derive(Hash)` fold
# bit-for-bit and behaves identically as a dict key. Raw str-hash VALUES are
# pinned under a fixed seed in hash_wyhash_seed{1,2}.zap.


# `@derive(Hash, Eq)` folds field hashes with a 0-seeded accumulator in
# declaration order (D-0047). A str field stays usable as a key under the new
# wyhash str hash.
@derive(Hash, Eq)
struct Derived:
    str name
    int64 age


# The same two fields, hand-hashed exactly the way the derive fold does: seed the
# accumulator at 0, then mix each field's own `__hash__()` with `hash_combine`, in
# declaration order. This MUST reproduce `Derived`'s derived hash bit-for-bit, so
# the two compare equal for equal field values (regardless of the process seed).
struct Hand:
    str name
    int64 age

    def __hash__(self) -> int64:
        auto h = hash_combine(0, self.name.__hash__())
        h = hash_combine(h, self.age.__hash__())
        return h

    def __eq__(self, Hand other) -> bool:
        return self.name == other.name and self.age == other.age


def main():
    # --- str.__hash__(): equal strings hash equal within a run ---
    print("abc".__hash__() == "abc".__hash__())  # True
    auto built = "a" + "bc"
    print(built.__hash__() == "abc".__hash__())  # True  (equal bytes -> equal hash)
    print("abc".__hash__() == "abd".__hash__())  # False (distinct bytes)
    print("".__hash__() == "".__hash__())  # True  (empty string)
    # A str and the bytes with the same bytes hash equal (shared ZapStr repr).
    auto raw = b"abc"
    print(raw.__hash__() == "abc".__hash__())  # True

    # --- scalar __hash__() is unseeded (splitmix64): deterministic raw values ---
    auto five = 5
    print(five.__hash__())  # 7134611160154358618
    auto flag = True
    # `bool` is an i1; coercing to i64 for hashing sign-extends `True` to -1
    # (the same path dict bool-keys take), so this is zap_hash_i64(-1).
    print(flag.__hash__())  # -1956407806741107680
    auto ch = chr(97)  # the char 'a' (codepoint 97)
    print(ch.__hash__())  # 5718913436695043505
    # float __hash__: value-equal floats hash equal; distinct differ (#803 word).
    auto f1 = 3.5
    auto f2 = 3.6
    print(f1.__hash__() == f1.__hash__())  # True
    print(f1.__hash__() == f2.__hash__())  # False

    # --- hash_combine: deterministic and order-sensitive ---
    print(hash_combine(1, 2) == hash_combine(1, 2))  # True
    print(hash_combine(1, 2) == hash_combine(2, 1))  # False

    # --- hand-written struct __hash__ == @derive(Hash), bit-for-bit ---
    auto d1 = Derived("alice", 30)
    auto h1 = Hand("alice", 30)
    print(d1.__hash__() == h1.__hash__())  # True
    auto d2 = Derived("bob", 7)
    auto h2 = Hand("bob", 7)
    print(d2.__hash__() == h2.__hash__())  # True

    # Both usable as dict keys, behaving identically (str field in the key).
    dict[Derived, int64] md = {}
    md[Derived("alice", 30)] = 1
    md[Derived("bob", 7)] = 2
    print(md[Derived("alice", 30)])  # 1
    print(len(md))  # 2
    dict[Hand, int64] mh = {}
    mh[Hand("alice", 30)] = 1
    mh[Hand("bob", 7)] = 2
    print(mh[Hand("alice", 30)])  # 1
    print(len(mh))  # 2

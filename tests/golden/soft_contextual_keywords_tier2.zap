# Golden fixture: Tier-2 soft (contextual) keywords (#1412, epic #1413):
# `spawn`, `when`, `do`, `unsafe`, `python`. Each is a keyword only in its own
# statement/clause slot and an ordinary identifier everywhere else. This program
# uses all five as a struct field, a parameter, a named-argument label, an
# attribute read and a plain variable — in the SAME program as the `do:`…`while`,
# `unsafe:`, `spawn <call>` and `when target.*:` statement forms — and runs to a
# deterministic result (the `when` arms print the same, so it is OS-independent;
# the single-producer scope join drains after the barrier).


struct Bag:
    str spawn
    str do
    bool unsafe
    str when
    str python


def tag(str spawn, str python) -> str:
    # `spawn` and `python` as ordinary parameters and expressions.
    return spawn + "/" + python


def deposit(channel[str] ch, str v):
    ch <- v


def main():
    # Named-argument labels + attribute reads that are soft keywords.
    auto b = Bag(spawn="w", do="build", unsafe=False, when="now", python="3.12")
    print(b.spawn)  # w
    print(b.do)  # build
    print(b.unsafe)  # False
    print(b.when)  # now
    print(b.python)  # 3.12
    print(tag(spawn=b.spawn, python=b.python))  # w/3.12

    # `do` KEYWORD form (do-while) with a deterministic counter.
    auto n = 0
    do:
        n = n + 1
    while n < 3
    print(n)  # 3

    # `unsafe` KEYWORD form — an ordinary block body runs normally.
    unsafe:
        print("unsafe-ok")  # unsafe-ok

    # `when` KEYWORD form — both arms print the same, so the output does not
    # depend on the target OS.
    when target.os == .linux:
        print("when-ok")  # when-ok
    else:
        print("when-ok")

    # `spawn` KEYWORD form alongside the `spawn` field/variable; the single
    # worker's result is drained after the scope join, so it is deterministic.
    auto ch = channel[str](1)
    auto spawn = b.spawn
    scope:
        spawn deposit(ch, spawn)
    print(<-ch)  # w

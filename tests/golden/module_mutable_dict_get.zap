# `dict.get` on a module-level MUTABLE dict global (#662 / D-0112) — the case
# that surfaced issue #671. This is the issue's smallest repro verbatim
# (`t["a"] = t.get("a", 0) + 1`) plus a running word tally that both reads
# (two-arg get with a default) and writes the same module global across calls,
# and a one-arg Optional read of it. Before the fix, `dict.get` on a module
# global fell through to lowering as a bare `get` and was rejected (ZAP6001);
# the identical body on a local worked. (A mutable non-@shareable global is
# flagged by the #664 sharing lint, ZAP5125, on stderr — expected and harmless;
# stdout is what this golden asserts.)
dict[str, int64] t = {}
dict[str, int64] tally = {}
dict[str, str] last = {}


def seen(str k):
    # The issue's exact idiom: read-with-default, increment, store back — all on
    # the module global `t`.
    t[k] = t.get(k, 0) + 1


def bump(str word):
    # Same two-arg-get idiom on a different mutable global, across calls.
    tally[word] = tally.get(word, 0) + 1


def remember(str k, str v):
    last[k] = v


def probe(str k) -> str:
    # One-arg Optional read of a mutable global.
    match last.get(k):
        case Optional.Some(v):
            return v
        case Optional.None:
            return "<none>"


def main():
    seen("a")
    seen("a")
    seen("b")
    print(t["a"])  # 2 — get-with-default accumulated across calls
    print(t["b"])  # 1

    bump("x")
    bump("x")
    bump("x")
    bump("y")
    print(tally.get("x", 0))  # 3
    print(tally.get("y", 0))  # 1
    print(tally.get("z", 0))  # 0 — default, absent key

    remember("host", "example.com")
    print(probe("host"))  # example.com
    print(probe("missing"))  # <none>

    # Reassigning the whole global to a fresh dict still leaves `get` specialized.
    tally = {"reset": 9}
    print(tally.get("reset", -1))  # 9
    print(tally.get("x", -1))  # -1 — old contents gone after reassignment

# `dict.get` on a module-level CONST dict global (#658 / D-0112), the receiver
# whose type monomorphization previously could not resolve (issue #671). Both
# the two-arg `get(key, default) -> V` (#570) and the one-arg
# `get(key) -> Optional[V]` (#281) forms are receiver-type-driven rewrites: mono
# must know the receiver is a `dict` to specialize them, and a module global was
# opaque to it. This fixture drives both forms, both value kinds (scalar int64,
# pointer str) and both key kinds (str, int64), reading a CONST global dict from
# several functions — proving the receiver type now resolves for globals exactly
# as it does for locals.
const dict[str, int64] counts = {"a": 1, "b": 2, "z": 0}
const dict[str, str] headers = {"accept": "json", "host": "x", "empty": ""}
const dict[int64, str] byid = {10: "ten", 20: "twenty"}


def count_or(str k, int64 d) -> int64:
    # Two-arg get on a const global: stored value when present, else default.
    return counts.get(k, d)


def header_or(str k, str d) -> str:
    return headers.get(k, d)


def describe(str k) -> str:
    # One-arg get on a const global -> Optional[str]; match on the result.
    match headers.get(k):
        case Optional.Some(v):
            return v
        case Optional.None:
            return "<absent>"


def main():
    # Two-arg form, scalar values: present -> value, absent -> default.
    print(count_or("a", -1))  # 1
    print(count_or("missing", -1))  # -1
    # A legitimately stored 0 is the value, never confused with a miss.
    print(count_or("z", 7))  # 0
    # The default composes directly in arithmetic (result is int64).
    print(counts.get("b", 0) + counts.get("nope", 100))  # 102

    # Two-arg form, pointer values.
    print(header_or("accept", "none"))  # json
    print(header_or("missing", "none"))  # none
    # A stored empty string is present, so the default is unused.
    print(len(header_or("empty", "fallback")))  # 0

    # One-arg Optional form on a const global, via match and the prelude helper.
    print(describe("host"))  # x
    print(describe("gone"))  # <absent>
    Optional[int64] ca = counts.get("a")
    Optional[int64] cz = counts.get("zzz")
    print(ca.unwrap_or(-1))  # 1
    print(cz.unwrap_or(-1))  # -1

    # Int-keyed const global, str values, both forms.
    print(byid.get(10, "?"))  # ten
    print(byid.get(99, "?"))  # ?
    match byid.get(20):
        case Optional.Some(v):
            print(v)  # twenty
        case Optional.None:
            print("?")

# Golden fixture: `dict.get(key, default) -> V` (ticket #570). Unlike the one-arg
# `get(key) -> Optional[V]` safe read (#281), the two-arg form returns a plain
# `V` — the stored value when the key is present, else the supplied `default`.
# This is the common Python idiom `d.get(k, default)`. Presence is decided by
# the #299 membership probe, not by the fetched word, so a legitimately-stored
# `0`/`""` selects the stored value, never the default. Exercises both value
# kinds (scalar int64 and pointer str) and both key kinds (str and int).
def main():
    # Scalar-payload dict (int64 values): present -> value, absent -> default.
    dict[str, int64] counts = {"a": 1, "b": 2}
    print(counts.get("a", -1))  # 1
    print(counts.get("z", -1))  # -1
    # The default participates directly in arithmetic (result is int64, not
    # Optional), and composes in an expression.
    print(counts.get("b", 0) + counts.get("missing", 100))  # 102

    # A legitimately stored 0 is returned, never confused with a miss.
    dict[str, int64] zeros = {"k": 0}
    print(zeros.get("k", 7))  # 0
    print(zeros.get("gone", 7))  # 7

    # Pointer-payload dict (str values): present -> value, absent -> default.
    dict[str, str] headers = {"accept": "json", "host": "x"}
    print(headers.get("accept", "none"))  # json
    print(headers.get("missing", "none"))  # none

    # A stored empty string is Some, so the default is not used.
    dict[str, str] empties = {"k": ""}
    print(len(empties.get("k", "fallback")))  # 0
    print(empties.get("gone", "fallback"))  # fallback

    # Int-keyed dict (key kind i64), str values.
    dict[int64, str] byid = {10: "ten", 20: "twenty"}
    print(byid.get(10, "?"))  # ten
    print(byid.get(99, "?"))  # ?

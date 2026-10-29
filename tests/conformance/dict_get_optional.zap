# claim: DICT-1
# SPEC §18 (#281): `d.get(key)` is the safe, non-aborting read — it returns
# `Optional[V]`, `Some(v)` for a present key and `None` for an absent one (unlike
# `d[key]`, which faults on a miss). Exercised through `unwrap_or` and `is_none`
# over both a pointer-payload (str) and a scalar-payload (int64) value type.
def main():
    auto headers = {"accept": "json", "host": "x"}
    Optional[str] hit = headers.get("accept")
    Optional[str] miss = headers.get("nope")
    print(hit.unwrap_or("fallback"))
    print(miss.unwrap_or("fallback"))
    if miss.is_none():
        print("miss-none")
    auto counts = {"a": 1, "b": 2}
    Optional[int64] ca = counts.get("a")
    Optional[int64] cz = counts.get("z")
    print(ca.unwrap_or(-1))
    print(cz.unwrap_or(-1))

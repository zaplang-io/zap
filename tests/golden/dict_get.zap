# Golden fixture: `dict.get(key) -> Optional[V]`, the safe, non-aborting read
# (ticket #281). `d[missing]` aborts the coroutine; `d.get(missing)` returns
# `Optional.None`, so a dynamic map (e.g. a `json.loads(...)` object) can be
# probed without decoding into a struct. This completes #281 — its `k in d`
# membership half shipped with #299. Exercises BOTH Optional payload layouts:
# a pointer payload (str values -> niche nullable-pointer Optional) and an
# unboxed scalar payload (int64 values -> tagged Optional), and both key kinds.
def describe_str(Optional[str] o) -> str:
    match o:
        case Optional.Some(v):
            return v
        case Optional.None:
            return "<absent>"

def main():
    # Pointer-payload dict (str values): present -> Some(value), absent -> None.
    auto headers = {"accept": "json", "host": "x"}
    print(describe_str(headers.get("accept")))  # json
    print(describe_str(headers.get("missing")))  # <absent>

    # Compose with the prelude Optional helpers (is_none / unwrap_or).
    Optional[str] hit = headers.get("host")
    Optional[str] miss = headers.get("nope")
    if hit.is_none():
        print("hit-none")
    else:
        print("hit-some")  # hit-some
    if miss.is_none():
        print("miss-none")  # miss-none
    print(hit.unwrap_or("fallback"))  # x
    print(miss.unwrap_or("fallback"))  # fallback

    # A legitimately stored empty string is Some(""), never confused with a
    # miss — presence is decided by membership, not by the fetched word.
    auto e = {"k": ""}
    match e.get("k"):
        case Optional.Some(v):
            print(len(v))  # 0
        case Optional.None:
            print(-1)

    # Scalar-payload dict (int64 values): the tagged Optional layout.
    auto counts = {"a": 1, "b": 2}
    Optional[int64] ca = counts.get("a")
    Optional[int64] cz = counts.get("z")
    print(ca.unwrap_or(-1))  # 1
    print(cz.unwrap_or(-1))  # -1

    # Int-keyed dict (key kind i64), str values.
    auto byid = {10: "ten", 20: "twenty"}
    print(describe_str(byid.get(10)))  # ten
    print(describe_str(byid.get(99)))  # <absent>

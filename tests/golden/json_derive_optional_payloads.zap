# @derive(Json) DECODE of an `Optional[T]` field whose payload is any type the
# shared element codec handles — not just the `int64` / `str` fast paths the
# decoder used to be stuck on (#1696: 274/435 generated AWS clients had an
# Optional field of some other payload and failed to derive the decoder with
# `@derive(Json): unsupported Optional payload`). Decode is now symmetric with
# encode (`__json_dec_elem[f.ty.inner]` wrapped in `Some`): a present value is
# `Some(decode payload)`, an absent key OR an explicit JSON null is `None`.
#
# Covers an AWS-flavored shape with several Optional fields — a scalar
# (`int64`), a pointer scalar (`str`), a `float64`, a `bool`, a `list`, a nested
# derived struct, and an open string enum — across present / absent / null.
# Dict order is unspecified (#1066), so the round-trips inspect by field, and the
# encode round-trip proves None omits its key.

@derive(Json)
@json(open)
enum Status:
    Active
    Inactive
    Unknown(str v)

@derive(Json)
struct Tag:
    str key
    str value

@derive(Json)
struct Resource:
    str name
    Optional[int64] size
    Optional[str] region
    Optional[float64] ratio
    Optional[bool] enabled
    Optional[list[str]] zones
    Optional[Tag] primary_tag
    Optional[Status] status

def main():
    # Every Optional present: each payload decodes and wraps in Some.
    auto r = Resource.from_json(
        "{\"name\":\"r1\",\"size\":10,\"region\":\"us\",\"ratio\":1.5," + "\"enabled\":true,\"zones\":[\"a\",\"b\"]," + "\"primary_tag\":{\"key\":\"env\",\"value\":\"prod\"}," + "\"status\":\"Active\"}",
    )
    print(r.name)  # r1
    print(r.size.unwrap())  # 10
    print(r.region.unwrap())  # us
    if r.ratio.unwrap() == 1.5:
        print("ratio-ok")  # ratio-ok
    if r.enabled.unwrap():
        print("enabled")  # enabled
    auto z = r.zones.unwrap()
    print(z[0])  # a
    print(z[1])  # b
    print(len(z))  # 2
    print(r.primary_tag.unwrap().key)  # env
    print(r.primary_tag.unwrap().value)  # prod
    print(r.status.unwrap().to_json())  # "Active"
    # Round-trip: every present Optional re-encodes.
    print(r.to_json())

    # Every Optional absent: each decodes to None; dumps omits them all.
    auto e = Resource.from_json("{\"name\":\"r2\"}")
    if e.size.is_none():
        print("size-none")  # size-none
    if e.region.is_none():
        print("region-none")  # region-none
    if e.ratio.is_none():
        print("ratio-none")  # ratio-none
    if e.enabled.is_none():
        print("enabled-none")  # enabled-none
    if e.zones.is_none():
        print("zones-none")  # zones-none
    if e.primary_tag.is_none():
        print("tag-none")  # tag-none
    if e.status.is_none():
        print("status-none")  # status-none
    print(e.to_json())  # {"name":"r2"}

    # Explicit JSON null also decodes to None, for scalar and nested payloads.
    auto n = Resource.from_json(
        "{\"name\":\"r3\",\"size\":null,\"primary_tag\":null,\"zones\":null}",
    )
    if n.size.is_none():
        print("n-size-none")  # n-size-none
    if n.primary_tag.is_none():
        print("n-tag-none")  # n-tag-none
    if n.zones.is_none():
        print("n-zones-none")  # n-zones-none
    print(n.to_json())  # {"name":"r3"}

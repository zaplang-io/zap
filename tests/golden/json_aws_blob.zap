# Golden fixture: @derive(Json) AWS blob hook (ticket #346, D-0086). A `bytes`
# field (de)serializes as a base64 (standard alphabet, padded) JSON string via
# encoding/base64, round-tripping the exact bytes — including non-ASCII bytes
# that are not valid UTF-8.
@derive(Json)
struct Blob:
    bytes data

def main():
    print(Blob(data=b"foobar").to_json())  # {"data":"Zm9vYmFy"}

    auto a = Blob.from_json("{\"data\":\"Zm9vYmFy\"}")
    print(str(a.data))  # foobar

    # Binary (non-UTF-8) bytes round-trip exactly.
    auto b1 = Blob(data=bytes([251, 255]))
    print(b1.to_json())  # {"data":"+/8="}
    auto b2 = Blob.from_json(b1.to_json())
    print(base64.encode(b2.data))  # +/8=

# Golden fixture: @derive(Json) on a struct whose field is literally `name`
# (I#141) — the common real-world payload shape. Round-trips encode/decode.
@derive(Json)
struct User:
    str name
    int64 id

def main():
    auto u = User.from_json("{\"name\": \"ada\", \"id\": 7}")
    print(u.name)           # ada
    print(u.id)             # 7
    print(u.to_json())      # {"name":"ada","id":7}

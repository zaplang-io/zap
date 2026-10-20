# Golden fixture: @derive(Yaml) encode + decode round-trip (I#149, D-0063) on a
# nested struct, including a field literally named `name` (an I#141 regression
# signal). `to_yaml` emits a block mapping; `from_yaml` parses it back through
# the shared JsonValue decode core (the same one @derive(Json) uses).
@derive(Yaml)
struct Addr:
    str city
    int64 zip

@derive(Yaml)
struct User:
    str name
    int64 id
    bool admin
    Addr addr

def main():
    auto u = User(name="ada", id=7, admin=True, addr=Addr(city="paris", zip=75001))
    print(u.to_yaml())
    auto v = User.from_yaml(u.to_yaml())
    print(v.name)        # ada
    print(v.id)          # 7
    print(v.addr.city)   # paris
    print(v.addr.zip)    # 75001

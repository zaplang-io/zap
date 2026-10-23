# Golden fixture: @derive(Json) decode direction (D-0059, I#100). The derive
# generates a compile-time associated function `Struct.from_json(str) -> Struct`
# that parses the JSON document and populates each field by name. Covers flat
# scalar/str fields; bool and float decoding are verified via the clean
# from_json -> to_json round-trip (print() rendering of bool/float is a separate
# concern).

@derive(Json)
struct Config:
    str host
    int64 port
    bool debug
    float64 ratio

@derive(Json)
struct Inner:
    int64 a

@derive(Json)
struct Outer:
    str tag
    Inner inner

def main():
    auto c = Config.from_json(
        "{\"host\": \"srv\", \"port\": 8080, \"debug\": true, \"ratio\": 0.5}",
    )
    print(c.host)  # srv
    print(c.port)  # 8080
    # Round-trip proves debug (true) and ratio (0.5) decoded correctly.
    print(c.to_json())  # {"host":"srv","port":8080,"debug":true,"ratio":0.5}

    # Nested decode: a nested @derive(Json) struct, via from_json_value recursion.
    auto o = Outer.from_json("{\"tag\": \"x\", \"inner\": {\"a\": 9}}")
    print(o.to_json())  # {"tag":"x","inner":{"a":9}}

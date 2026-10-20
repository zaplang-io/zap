# Golden fixture: json.dumps — compact serialization of a JsonValue (D-0059,
# I#131). A Zap prelude writer walks the DOM, emitting fragments into a list[str]
# joined once. Covers scalars, string escaping, arrays, a (single-key) object,
# and a loads->dumps round-trip. Object key order is unspecified (hash order),
# so multi-key objects are exercised only via round-trip structure, not here.

def main():
    print(json.dumps(JsonValue.Null))             # null
    print(json.dumps(JsonValue.Bool(True)))       # true
    print(json.dumps(JsonValue.Int(42)))          # 42
    print(json.dumps(JsonValue.Str("hi")))        # "hi"
    print(json.dumps(JsonValue.Str("a\"b\nc")))   # "a\"b\nc" (escaped)

    auto xs = [JsonValue.Int(1), JsonValue.Int(2), JsonValue.Null]
    print(json.dumps(JsonValue.Array(xs)))        # [1,2,null]

    auto m = {"k": JsonValue.Bool(False)}
    print(json.dumps(JsonValue.Object(m)))        # {"k":false}

    print(json.dumps(json.loads("[true, 3, \"x\"]")))  # [true,3,"x"]

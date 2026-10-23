# Golden fixture: the dynamic JSON DOM sum type `JsonValue` (D-0059, I#131).
# A compiler-known prelude enum (data-carrying, D-0046): `Int`/`Float` kept
# distinct, with recursive `Array(list[JsonValue])` / `Object(dict[str,
# JsonValue])` variants. This fixture only exercises the type itself —
# construction, nesting, and exhaustive `match` — ahead of `json.loads`/`dumps`.

def tag(JsonValue v) -> str:
    match v:
        case JsonValue.Null:
            return "null"
        case JsonValue.Bool(b):
            return "bool"
        case JsonValue.Int(i):
            return "int"
        case JsonValue.Float(f):
            return "float"
        case JsonValue.Str(s):
            return "str"
        case JsonValue.Array(items):
            return "array"
        case JsonValue.Object(fields):
            return "object"

def main():
    print(tag(JsonValue.Null))  # null
    print(tag(JsonValue.Bool(True)))  # bool
    print(tag(JsonValue.Int(42)))  # int
    print(tag(JsonValue.Float(3.5)))  # float
    print(tag(JsonValue.Str("hi")))  # str

    # Recursive nesting: an array holding scalar elements.
    auto xs = [JsonValue.Int(1), JsonValue.Int(2)]
    print(tag(JsonValue.Array(xs)))  # array

    # And an object mapping string keys to JsonValue.
    auto m = {"a": JsonValue.Bool(False)}
    print(tag(JsonValue.Object(m)))  # object

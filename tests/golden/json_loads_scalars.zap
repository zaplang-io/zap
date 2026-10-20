# Golden fixture: json.loads over scalar documents (D-0059, I#131).
# loads parses a JSON source (str|bytes) into the dynamic JsonValue DOM; the
# tree build is a Zap prelude function driving the C tokenizer cursor. This
# fixture covers each scalar kind, integer value fidelity, and escape decoding.

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
    print(tag(json.loads("null")))        # null
    print(tag(json.loads("true")))        # bool
    print(tag(json.loads("false")))       # bool
    print(tag(json.loads("  42 ")))       # int (leading/trailing ws ignored)
    print(tag(json.loads("3.5")))         # float
    print(tag(json.loads("\"hi\"")))      # str

    # Integer value fidelity (negative).
    match json.loads("-7"):
        case JsonValue.Int(i):
            print(i)                      # -7
        case _:
            print("notint")

    # String escape decoding: JSON "a\nb" -> a<newline>b.
    match json.loads("\"a\\nb\""):
        case JsonValue.Str(s):
            print(s)                      # a / b on two lines
        case _:
            print("notstr")

# Golden fixture: json.loads over arrays and objects (D-0059, I#131) — the
# recursive prelude builder over the C cursor. Covers nested arrays/objects,
# empty containers, element indexing, and key lookup.

def describe(JsonValue v) -> str:
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
    # Array: length + element values by index.
    match json.loads("[10, 20, 30]"):
        case JsonValue.Array(items):
            print(len(items))  # 3
            match items[0]:
                case JsonValue.Int(i):
                    print(i)  # 10
                case _:
                    print("?")
            match items[2]:
                case JsonValue.Int(i):
                    print(i)  # 30
                case _:
                    print("?")
        case _:
            print("notarray")

    print(describe(json.loads("[]")))  # array (empty)

    # Object: length + values by key.
    match json.loads("{\"a\": 1, \"b\": \"hi\"}"):
        case JsonValue.Object(fields):
            print(len(fields))  # 2
            match fields["a"]:
                case JsonValue.Int(i):
                    print(i)  # 1
                case _:
                    print("?")
            match fields["b"]:
                case JsonValue.Str(s):
                    print(s)  # hi
                case _:
                    print("?")
        case _:
            print("notobject")

    print(describe(json.loads("{}")))  # object (empty)

    # Nesting: an object whose value is an array.
    match json.loads("{\"xs\": [true, null]}"):
        case JsonValue.Object(fields):
            print(describe(fields["xs"]))  # array
        case _:
            print("?")

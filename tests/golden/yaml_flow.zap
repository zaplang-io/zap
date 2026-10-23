# Golden fixture: yaml.load flow style (I#149, D-0063). The supported flow
# subset is JSON-compatible, so flow collections pass through the YAML->JSON
# transpiler verbatim and parse via the json tokenizer.

def main():
    auto d = yaml.load("{\"a\": 1, \"b\": [10, 20, 30], \"c\": \"hi\"}")
    match d:
        case JsonValue.Object(m):
            match m["a"]:
                case JsonValue.Int(i):
                    print(i)  # 1
                case _:
                    print("?")
            match m["b"]:
                case JsonValue.Array(xs):
                    print(len(xs))  # 3
                    match xs[2]:
                        case JsonValue.Int(i):
                            print(i)  # 30
                        case _:
                            print("?")
                case _:
                    print("?")
            match m["c"]:
                case JsonValue.Str(s):
                    print(s)  # hi
                case _:
                    print("?")
        case _:
            print("notobject")

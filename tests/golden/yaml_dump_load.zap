# Golden fixture: yaml.dump -> yaml.load round-trip over the JsonValue DOM
# (I#149, D-0063). `dump` emits block YAML (nested mapping + a nested block
# sequence); re-`load`ing that text must reproduce the same values.

def main():
    auto v1 = yaml.load(
        "name: svc\nport: 90\nnested:\n  flag: false\n  items:\n    - 1\n    - 2\n",
    )
    auto text = yaml.dump(v1)
    auto v2 = yaml.load(text)
    match v2:
        case JsonValue.Object(m):
            match m["name"]:
                case JsonValue.Str(s):
                    print(s)  # svc
                case _:
                    print("?")
            match m["port"]:
                case JsonValue.Int(i):
                    print(i)  # 90
                case _:
                    print("?")
            match m["nested"]:
                case JsonValue.Object(inner):
                    match inner["flag"]:
                        case JsonValue.Bool(b):
                            if b:
                                print(1)
                            else:
                                print(0)  # 0
                        case _:
                            print("?")
                    match inner["items"]:
                        case JsonValue.Array(xs):
                            print(len(xs))  # 2
                            match xs[1]:
                                case JsonValue.Int(i):
                                    print(i)  # 2
                                case _:
                                    print("?")
                        case _:
                            print("?")
                case _:
                    print("?")
        case _:
            print("notobject")

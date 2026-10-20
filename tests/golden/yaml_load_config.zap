# Golden fixture: yaml.load over a representative config (I#149, D-0063) — a
# block mapping with a nested mapping, a block sequence, scalars of every kind,
# and a comment. Asserts the shared JsonValue DOM the YAML->JSON transpiler
# builds (YAML 1.2 is a JSON superset, so `load` reuses JsonValue).

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
    auto doc = yaml.load("# service config\nname: web\nport: 8080\ndebug: true\nratio: 1.5\ntags:\n  - alpha\n  - beta\ndb:\n  host: localhost\n  pool: 5\n")
    match doc:
        case JsonValue.Object(fields):
            match fields["name"]:
                case JsonValue.Str(s):
                    print(s)                  # web
                case _:
                    print("?")
            match fields["port"]:
                case JsonValue.Int(i):
                    print(i)                  # 8080
                case _:
                    print("?")
            match fields["debug"]:
                case JsonValue.Bool(b):
                    if b:
                        print(1)              # 1
                    else:
                        print(0)
                case _:
                    print("?")
            print(describe(fields["ratio"]))  # float
            print(describe(fields["tags"]))   # array
            match fields["tags"]:
                case JsonValue.Array(items):
                    print(len(items))         # 2
                    match items[0]:
                        case JsonValue.Str(s):
                            print(s)          # alpha
                        case _:
                            print("?")
                case _:
                    print("?")
            match fields["db"]:
                case JsonValue.Object(inner):
                    match inner["host"]:
                        case JsonValue.Str(s):
                            print(s)          # localhost
                        case _:
                            print("?")
                    match inner["pool"]:
                        case JsonValue.Int(i):
                            print(i)          # 5
                        case _:
                            print("?")
                case _:
                    print("?")
        case _:
            print("notobject")

# Golden fixture: json.scan — a public pull/token Scanner (D-0059, I#100). It
# returns a JsonScanner, an Iterator[JsonToken] (D-0048) over the tokenizer
# cursor, so `for tok in json.scan(src)` yields each structural/scalar token in
# constant memory. The loop stops at End (comma-ok __next__ returns ok=False).

def tokname(JsonToken t) -> str:
    match t:
        case JsonToken.End:
            return "end"
        case JsonToken.Null:
            return "null"
        case JsonToken.Bool(b):
            return "bool"
        case JsonToken.Int(i):
            return "int"
        case JsonToken.Float(f):
            return "float"
        case JsonToken.Str(s):
            return "str"
        case JsonToken.BeginObject:
            return "{"
        case JsonToken.EndObject:
            return "}"
        case JsonToken.BeginArray:
            return "["
        case JsonToken.EndArray:
            return "]"
        case JsonToken.Colon:
            return ":"
        case JsonToken.Comma:
            return ","

def main():
    for tok in json.scan("{\"a\": [1, true]}"):
        print(tokname(tok))

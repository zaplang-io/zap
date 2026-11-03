# Runtime JSON boundary regression (#630): validation precedes cursor creation,
# so malformed arrays cannot reach the old prelude's non-terminating EOF loop.
def reject(str source):
    auto caught = False
    try:
        json.loads(source)
    except Exception as e:
        caught = e.startswith("invalid JSON document")
    assert caught

def main():
    for source in ["", " ", "{", "[", "[1", "[1,", "{\"a\"", "{\"a\":", "{\"a\":1", "{}{}", "{} garbage", "null true", "[1,]", "{\"a\":1,}", "[1 2]", "{\"a\" 1}", "{\"a\":1 \"b\":2}", "[}", "{]", "nul", "tru", "falsex", "NaN", "Infinity", "-", "+1", "01", "-01", ".1", "1.", "1e", "1e+", "1e-", "--1", "\"open", "\"\\q\"", "\"\\u000g\"", "\"\\u000\"", "\"\\uD800\"", "\"\\uDC00\"", "\"\\uD800\\u0041\"", "\"a\nb\"", "\"a\tb\""]:
        reject(source)
    reject("\"" + str(chr(0)) + "\"")
    auto too_deep = "0"
    for i in range(129):
        too_deep = "[" + too_deep + "]"
    reject(too_deep)
    auto at_limit = "0"
    for i in range(128):
        at_limit = "[" + at_limit + "]"
    json.loads(at_limit)
    for source in ["null", "true", "false", "0", "-0", "123", "-42", "1.5", "0.0", "1E+2", "-2e-2", "  {\"x\": [true, null, -1.25e+2]} \r\n", "\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"", "\"\\u0000\"", "\"\\uD7FF\"", "\"\\uE000\"", "\"\\udbff\\udfff\"", "\"café 😀\""]:
        json.loads(source)
    assert json.dumps(json.loads("\"\\ud83d\\ude00\"")) == "\"😀\""
    assert json.dumps(json.loads("\"\\u0061\"")) == "\"a\""
    # Preserve documented DOM duplicate policy: the last decoded name wins.
    assert json.dumps(json.loads("{\"a\":1,\"\\u0061\":2}")) == "{\"a\":2}"
    auto bad_utf8 = False
    try:
        json.loads(bytes([34, 255, 34]))
    except Exception:
        bad_utf8 = True
    assert bad_utf8
    auto scanner_rejected = False
    try:
        for token in json.scan("[1,"):
            print("unexpected token")
    except Exception:
        scanner_rejected = True
    assert scanner_rejected
    print("strict JSON: PASS")

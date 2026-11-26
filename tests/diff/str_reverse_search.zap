# diff: match
# #1630: str.rfind / str.rindex report codepoint indices (like find/index, len,
# indexing and slicing), not UTF-8 byte offsets. CPython is the authoritative
# oracle; reverse search over multibyte strings must agree with it.
def main():
    str s = "éx"
    print(len(s))
    print(s.find("x"))
    print(s.rfind("x"))
    print(s.rindex("x"))
    print(s.rfind(""))
    str t = "éxéx"
    print(t.rfind("x"))
    print(t.rfind("é"))
    print(t.rfind("éx"))
    print("aaaa".rfind("aa"))
    print("ééé".rfind("éé"))
    print("éx".rfind("z"))
    print("".rfind(""))
    print("".rfind("a"))

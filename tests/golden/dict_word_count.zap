# Golden fixture (#572): the everyday dict word-count loop — `w in counts`
# membership guarding an indexed read-modify-write `counts[w] = counts[w] + 1`,
# with an `else` first-insert, all inside a `for` over a list. A language review
# reported this ICEd codegen; this guards the dict-membership + indexed-update
# path. Output is deterministic: dict iteration order is unspecified, so we read
# known keys and len() rather than printing the dict.
def main():
    dict[str, int64] counts = {}
    list[str] words = ["a", "b", "a", "c", "b", "a"]
    for w in words:
        if w in counts:
            counts[w] = counts[w] + 1
        else:
            counts[w] = 1
    print(counts["a"])  # 3
    print(counts["b"])  # 2
    print(counts["c"])  # 1
    print(len(counts))  # 3

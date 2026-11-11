def main():
    dict[str, int64] d = {}
    int64 i = 0
    while i < 200000:
        d[f"k{i}"] = i
        i = i + 1
    int64 s = 0
    i = 0
    while i < 200000:
        s = s + d[f"k{i}"]
        i = i + 1
    print(s)

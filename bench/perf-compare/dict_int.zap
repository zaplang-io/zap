def main():
    dict[int64, int64] d = {}
    int64 i = 0
    while i < 200000:
        d[i] = i
        i = i + 1
    int64 s = 0
    i = 0
    while i < 200000:
        s = s + d[i]
        i = i + 1
    print(s)

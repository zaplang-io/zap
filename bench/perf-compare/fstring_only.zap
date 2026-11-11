def main():
    int64 total = 0
    int64 pass_num = 0
    while pass_num < 2:
        int64 i = 0
        while i < 200000:
            auto s = f"k{i}"
            total = total + len(s)
            i = i + 1
        pass_num = pass_num + 1
    print(total)

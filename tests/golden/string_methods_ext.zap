# Golden fixture: the thin str method set (ticket #971, D-0210). Exhaustive
# companion to string_methods (#284). Covers whitespace/maxsplit split, rsplit,
# splitlines, count, rfind/rindex, removeprefix/removesuffix, char-set strip,
# the ljust/rjust/center/zfill pad family, title/capitalize/casefold/swapcase,
# and the is* predicates. List results are rendered via "|".join(...) for a
# deterministic, readable line.
def main():
    # -- split: whitespace (no arg) collapses runs, ignores leading/trailing ---
    print("|".join("  a  b  c  ".split()))  # a|b|c
    print(len("".split()))  # 0
    print(len("   ".split()))  # 0
    # split with an explicit separator + maxsplit.
    print("|".join("a b c".split(" ", 1)))  # a|b c
    print("|".join("a,b,c,d".split(",", 2)))  # a|b|c,d
    print("|".join("a,,b".split(",")))  # a||b
    print(len("a,,b".split(",")))  # 3

    # -- rsplit: from the right ------------------------------------------------
    print("|".join("a b c".rsplit(" ", 1)))  # a b|c
    print("|".join("  a b  c  ".rsplit()))  # a|b|c
    print("|".join("a.b.c".rsplit(".", 1)))  # a.b|c

    # -- splitlines: \n, \r, \r\n boundaries, terminators dropped --------------
    print("|".join("a\nb\r\nc\rd".splitlines()))  # a|b|c|d
    print(len("a\nb\r\nc\rd".splitlines()))  # 4
    print("|".join("a\nb\n".splitlines()))  # a|b
    print("|".join("a\n\nb".splitlines()))  # a||b
    print(len("".splitlines()))  # 0

    # -- count (non-overlapping) ----------------------------------------------
    print("abcabcabc".count("abc"))  # 3
    print("aaaa".count("aa"))  # 2
    print("abc".count("x"))  # 0
    print("abc".count(""))  # 4

    # -- rfind / rindex --------------------------------------------------------
    print("abcabc".rfind("bc"))  # 4
    print("abc".rfind("x"))  # -1
    print("abcabc".rindex("a"))  # 3

    # -- removeprefix / removesuffix ------------------------------------------
    print("foobar".removeprefix("foo"))  # bar
    print("foobar".removeprefix("xyz"))  # foobar
    print("foobar".removesuffix("bar"))  # foo
    print("foobar".removesuffix("xyz"))  # foobar

    # -- strip with an explicit char set (and the no-arg whitespace form) -----
    print("xxhixx".strip("x"))  # hi
    print("xxhixx".lstrip("x"))  # hixx
    print("xxhixx".rstrip("x"))  # xxhi
    print("abcba".strip("ab"))  # c
    print("  hi  ".strip())  # hi

    # -- pad / justify ---------------------------------------------------------
    print(f"[{'hi'.ljust(5)}]")  # [hi   ]
    print("hi".ljust(5, "*"))  # hi***
    print("hi".rjust(5, "*"))  # ***hi
    print("hi".center(6, "-"))  # --hi--
    print("hi".center(5, "-"))  # --hi- (CPython split: extra fill on the left, #1656)
    print("42".zfill(5))  # 00042
    print("-42".zfill(5))  # -0042
    print("hello".ljust(3))  # hello (already wider; unchanged)

    # -- case ------------------------------------------------------------------
    print("hello world".title())  # Hello World
    print("abc-def ghi".title())  # Abc-Def Ghi
    print("hELLO".capitalize())  # Hello
    print("MixedCase".casefold())  # mixedcase
    print("Hello World".swapcase())  # hELLO wORLD

    # -- is* predicates --------------------------------------------------------
    print("12345".isdigit())  # True
    print("12a45".isdigit())  # False
    print("".isdigit())  # False
    print("abcDEF".isalpha())  # True
    print("abc1".isalpha())  # False
    print("abc123".isalnum())  # True
    print("abc 123".isalnum())  # False
    print("  \t\n".isspace())  # True
    print("a b".isspace())  # False
    print("hello".islower())  # True
    print("Hello".islower())  # False
    print("123".islower())  # False
    print("HELLO".isupper())  # True
    print("Hello".isupper())  # False

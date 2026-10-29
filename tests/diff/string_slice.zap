# diff: match
# String indexing and slicing by codepoint (D-0025, D-0026): negatives, steps,
# reverse, and out-of-range clamping, Python semantics. ord() turns a char into
# its codepoint int so the comparison is on plain ints.
def main():
    auto s = "Hello, World"
    print(ord(s[0]))
    print(ord(s[-1]))
    print(s[0:5])
    print(s[7:])
    print(s[:5])
    print(s[::2])
    print(s[::-1])
    print(s[-5:])
    print(s[1:100])
    print(len(s[3:1]))
    print(len(s))

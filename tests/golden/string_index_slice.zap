# ticket #284: str indexing (s[i] -> char), negative indices, len by codepoint,
# and slicing (s[a:b:step], Python semantics). `ord`/`char`/`chr` give char<->int
# and `str(char)` renders a char as a one-codepoint string.
def main():
    auto s = "Hello, World"
    print(ord(s[0]))
    print(ord(s[7]))
    print(ord(s[-1]))
    print(len(s))
    print(s[0:5])
    print(s[7:])
    print(s[:5])
    print(s[7:12])
    print(s[::2])
    print(s[::-1])
    print(s[-5:])
    print(str(s[0]))

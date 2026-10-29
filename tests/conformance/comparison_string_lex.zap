# claim: CMP-3
# SPEC §9.1 (D-0025): string ordering (`< <= > >=`) is lexicographic by codepoint
# (which for UTF-8 equals byte order). A shorter string that is a prefix of a
# longer one sorts first (`"ab" < "abc"`), and uppercase sorts before lowercase
# (`"Z" < "a"`, since 90 < 97). Rendered through str().
def main():
    print(str("abc" < "abd"))
    print(str("ab" < "abc"))
    print(str("abc" < "ab"))
    print(str("Z" < "a"))

# Golden fixture: an explicit renderer overrides the default (#274, D-0124).
# `__str__` is preferred over `__repr__`; a type with only `__repr__` uses it;
# `@derive(Repr)` (covered by operator_derive.zap) likewise overrides. The
# override applies everywhere the protocol is used (print/str/f-string).
struct HasStr:
    int64 n

    def __str__(self) -> str:
        return "STR"

    def __repr__(self) -> str:
        return "REPR"

struct HasReprOnly:
    int64 n

    def __repr__(self) -> str:
        return "ONLY_REPR"

def main():
    print(HasStr(n=1))  # STR  (__str__ preferred)
    print(str(HasStr(n=1)))  # STR
    print(f"{HasStr(n=1)}")  # STR
    print(HasReprOnly(n=2))  # ONLY_REPR  (__repr__ used when no __str__)

# Golden fixture: the Python-exact repr/str dunder contract (#1384).
#
# For a type defining BOTH __str__ and __repr__:
#   - str() / print() / f-string (str contexts) prefer __str__;
#   - repr() prefers __repr__;
#   - a CONTAINER renders its elements with repr (so str([x]) uses __repr__ too).
# A type defining only one dunder uses it in both contexts (fallback).
struct Both:
    int64 x

    def __str__(self) -> str:
        return "STR"

    def __repr__(self) -> str:
        return "REPR"


struct ReprOnly:
    int64 n

    def __repr__(self) -> str:
        return "R_ONLY"


struct StrOnly:
    int64 n

    def __str__(self) -> str:
        return "S_ONLY"


def main():
    # Dual-dunder type: str context prefers __str__, repr context __repr__.
    print(str(Both(x=1)))  # STR
    print(repr(Both(x=1)))  # REPR
    print(Both(x=1))  # STR   (print is a str context)
    print(f"{Both(x=1)}")  # STR   (f-string field is a str context)

    # Container elements render with repr in BOTH str() and repr() of the list.
    print(str([Both(x=1)]))  # [REPR]
    print(repr([Both(x=1)]))  # [REPR]

    # Single-dunder fallback: each context uses the one that exists.
    print(str(ReprOnly(n=1)))  # R_ONLY   (str falls back to __repr__)
    print(repr(ReprOnly(n=1)))  # R_ONLY
    print(str(StrOnly(n=1)))  # S_ONLY
    print(repr(StrOnly(n=1)))  # S_ONLY   (repr falls back to __str__)

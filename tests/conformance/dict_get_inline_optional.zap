# claim: DICT-5
# SPEC §18 (#497): the `Optional[V]` from `d.get(k)` is a method receiver in its
# own right — an inline `d.get(k).unwrap_or(default)` resolves and dispatches
# without first binding to an annotated `Optional[V]` local. This program never
# spells `Optional`, so it also pins that `dict.get` alone pulls the prelude
# `Optional[T]` into scope (the annotated-local form, which does name `Optional`,
# stays covered by DICT-1 / `dict_get_optional`). A present key yields the stored
# value; an absent key yields the default. Exercised over both payload layouts
# (str pointer, int64 scalar).
def main():
    dict[str, str] d = {"k": "v"}
    print(d.get("k").unwrap_or("x"))  # present -> v
    print(d.get("absent").unwrap_or("x"))  # absent -> x

    dict[str, int64] counts = {"a": 1}
    print(str(d.get("absent").is_none()))  # True
    print(str(counts.get("a").unwrap_or(-1)))  # 1
    print(str(counts.get("z").unwrap_or(-1)))  # -1

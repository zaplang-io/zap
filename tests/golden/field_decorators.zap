# Golden fixture: built-in field decorators (#472, D-0101).
#
# `@default(expr)` supplies a per-instance construction-time default for an
# omitted field (reusing the literal-default machinery, #461). `@validate(...)`
# and `@range(...)` generate a `{Struct}____validate__` guard that runs after
# every field is set and raises a catchable `ValueError` (D-0007) on a
# violation. `zap expand` shows the generated validator.

struct Account:
    @default(100)
    int64 balance
    @validate(max_len=8)
    str name
    @range(min=0, max=120)
    int64 age

def main():
    # The omitted `@default` field is initialized to its default.
    auto a = Account(name="ada", age=30)
    print(a.balance)
    print(a.name)
    print(a.age)

    # A name longer than max_len raises at construction; `except` catches it.
    try:
        auto bad = Account(name="verylongname", age=10)
        print(bad.name)
    except ValueError:
        print("name too long")

    # An out-of-range age likewise raises a catchable ValueError.
    try:
        auto old = Account(name="bob", age=200)
        print(old.age)
    except ValueError:
        print("age out of range")

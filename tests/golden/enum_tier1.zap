# Golden fixture: tier-1 enums (D-0018) — named constants backed by an int,
# member access, the explicit int<->enum conversions, and `match` dispatch with
# a `_` catch-all. A tier-1 enum is an unboxed scalar (its backing i64).
enum Day:
    MON
    TUE
    WED
    THU
    FRI
    SAT
    SUN

def is_weekend(Day d) -> int64:
    match d:
        case Day.SAT:
            return 1
        case Day.SUN:
            return 1
        case _:
            return 0

def main():
    print(is_weekend(Day.WED))  # 0
    print(is_weekend(Day.SAT))  # 1
    print(int64(Day.FRI))  # 4  (backing integer)
    auto d = Day(6)  # int -> enum  (SUN)
    print(is_weekend(d))  # 1

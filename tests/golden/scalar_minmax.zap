# Golden fixture: exact scalar min/max (#1279).
#
# Proves integer `min`/`max` select the lesser / greater WITHOUT routing through
# float: adjacent int64 values above 2^53 — where the f64 mantissa can no longer
# distinguish neighbours — stay distinct. Also covers equal inputs, negatives,
# mixed int widths, the full int64 range, unsigned values above INT64_MAX, and
# the float path. Mirrors the DNS deadline use case (zaplang-io/dns#6).

def earlier(int64 attempt_ns, int64 lifetime_ns) -> int64:
    return min(attempt_ns, lifetime_ns)

def main():
    # Adjacent int64 neighbours above 2^53: as f64 both round to the same
    # double, so a float path would collapse them. The integer path is exact.
    print(earlier(9007199254740993, 9007199254740992))  # 9007199254740992
    print(earlier(9007199254740993, 9007199254740994))  # 9007199254740993
    print(max(9007199254740993, 9007199254740992))  # 9007199254740993
    print(max(9007199254740993, 9007199254740994))  # 9007199254740994

    # Equal inputs return that value.
    print(min(42, 42))  # 42
    print(max(42, 42))  # 42

    # Negative operands (signed comparison).
    print(min(-5, -9))  # -9
    print(max(-5, -9))  # -5
    print(min(-1, 1))  # -1
    print(max(-1, 1))  # 1

    # Mixed int widths join to int64.
    int32 a = 7
    int64 b = 3
    print(min(a, b))  # 3
    print(max(a, b))  # 7

    # Near-full int64 range: a large negative vs INT64_MAX.
    int64 lo = -9223372036854775807
    int64 hi = 9223372036854775807
    print(min(lo, hi))  # -9223372036854775807
    print(max(lo, hi))  # 9223372036854775807

    # Unsigned values above INT64_MAX compare as unsigned, not as negative:
    # `uint64(0) - uint64(1)` wraps to the maximum uint64 (#552), which a signed
    # compare would read as -1.
    uint64 umax = uint64(0) - uint64(1)
    uint64 u2 = 1
    print(min(umax, u2))  # 1
    print(max(umax, u2))  # 18446744073709551615

    # Float path: ordered compare at f64.
    print(min(1.5, 2.5))  # 1.5
    print(max(1.5, 2.5))  # 2.5
    print(min(-2.5, -1.5))  # -2.5

# claim: BIG-1
# SPEC §4.1 (D-0019): bigint values retain precision beyond int64, use floor
# division and divisor-signed remainder, and support explicit conversions.
def main():
    auto n = 123456789012345678901234567890
    print(str(n + 11))
    print(str(n * 11))
    print(str(n // 11))
    print(str(n % 11))
    print(str(-n // 11))
    print(str(-n % 11))
    print(str(bigint(10.75)))
    print(int64(bigint(42)))

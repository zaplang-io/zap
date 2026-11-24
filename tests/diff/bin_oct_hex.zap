# diff: match
# bin()/oct()/hex() match CPython's sign-magnitude base strings (#1384): the
# 0b/0o/0x prefixes, lowercase digits, a leading '-' for negatives, and '0b0' /
# '0o0' / '0x0' at zero. Includes a >32-bit int64 value and a value large enough
# to promote to a bigint in Zap (an arbitrary-precision int in CPython), so the
# multi-limb formatting kernel is oracle-checked against CPython.
def main():
    print(bin(10))
    print(oct(10))
    print(hex(255))
    print(bin(0))
    print(oct(0))
    print(hex(0))
    print(bin(-5))
    print(oct(-9))
    print(hex(-255))
    print(hex(65535))
    print(bin(4294967296))
    print(hex(123456789012345678901234567890))
    print(hex(-123456789012345678901234567890))

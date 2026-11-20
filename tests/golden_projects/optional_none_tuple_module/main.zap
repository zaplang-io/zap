# Cross-module (#1195): call an imported function returning a tuple of
# `Optional[bytes]`, including the `(None, None)` cache-miss case, and
# destructure the pair in the importing module.
import cache


def main():
    auto value, cas = cache.gets_miss()
    print(value.is_none())  # True
    print(cas.is_none())  # True

    auto hv, hc = cache.gets_hit()
    print(hv.is_some())  # True
    print(hv.unwrap())  # value
    print(hc.unwrap())  # 42

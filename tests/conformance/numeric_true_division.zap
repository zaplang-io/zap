# claim: NUM-1
# SPEC §5 (D-0024): `/` is TRUE division and always yields a float, even when the
# quotient is integral (`6 / 2` is `3.0`, not `3`, after #384). Rendered through
# str() so the float form is the observable.
def main():
    print(str(7 / 2))
    print(str(6 / 2))
    print(str(1 / 4))
    print(str(5 / 2))
    print(str(1 / 8))

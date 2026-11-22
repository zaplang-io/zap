# Cross-module (#1265): the bare-payload → Optional[T] widening holds through
# imports — an imported function returns a bare payload, and an imported
# Optional[T] parameter is fed a bare payload at the call site. Read natively so
# the tagged-Some storage is observed.
import lib


def main():
    # Imported fn returning a bare payload → tagged Some, unwrapped in the importer.
    print(lib.boxed(7).unwrap())  # 7

    # Bare payload passed to an imported Optional[T] parameter (+ explicit None).
    print(lib.unwrap_or_zero(3))  # 3
    print(lib.unwrap_or_zero(None))  # 0

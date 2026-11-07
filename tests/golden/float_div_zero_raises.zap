# Golden (epic #374, #743 decision): float division by zero raises a catchable
# ZeroDivisionError (CPython parity), not IEEE inf. A runtime zero divisor forces
# the guarded helper (zap_fdiv); a literal nonzero divisor stays an inline fdiv.
def main():
    f64 z = 0.0
    try:
        print(1.0 / z)
    except Exception as e:
        print("caught")

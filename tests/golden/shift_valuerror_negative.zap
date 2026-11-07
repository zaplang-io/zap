# Golden (epic #374, #740): a negative shift count raises a catchable ValueError
# (CPython parity), not a poison value from an unmasked shift. A runtime count
# forces the guarded helper path (a literal in 0..63 stays an inline shift).
def main():
    int64 n = -1
    try:
        print(1 << n)
    except ValueError as e:
        print("caught")

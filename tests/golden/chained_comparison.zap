# Golden fixture: Python-style chained comparison (I#386). A run of two
# or more comparisons at the same level — `a op1 b op2 c ...` — means
# `(a op1 b) and (b op2 c) and ...`: each operand is evaluated exactly once, left
# to right, and evaluation stops at the first link that is False. This is NOT the
# left-associative `(a op1 b) op2 c` (which would compare a bool against `c`).

# A middle operand with an observable effect: it prints a marker when evaluated
# and returns its value. Used to prove every interior operand runs exactly once
# and that a later operand is skipped once a link fails.
def mark(int64 tag, int64 v) -> int64:
    print("eval " + str(tag))
    return v

def main():
    # --- three-way chains, true and false ---
    print(str(1 < 2 < 3))  # True
    print(str(1 < 3 < 2))  # False: 3 < 2 is False
    print(str(3 > 2 > 1))  # True: left-assoc would wrongly compare a bool to 1
    print(str(2 < 1 < 3))  # False: 2 < 1 is False

    # --- longer chains (four operands) ---
    print(str(1 < 2 < 3 < 4))  # True
    print(str(1 < 2 < 3 < 0))  # False: 3 < 0 is False

    # --- mixed operators ---
    print(str(1 <= 1 < 2))  # True
    print(str(5 >= 5 > 2 == 2))  # True
    print(str(1 == 1 != 2))  # True
    print(str(1 < 2 > 5))  # False: 2 > 5 is False

    # --- a single comparison is unchanged ---
    print(str(1 < 2))  # True

    # --- a side-effecting middle operand is evaluated exactly once ---
    print(str(1 < mark(0, 5) < 10))  # eval 0, then True
    print(str(1 < mark(3, 5) < 2))  # eval 3, then False (5 < 2 is False)

    # --- short-circuit: the first link is False, so the last operand never runs ---
    print(str(5 < 1 < mark(2, 9)))  # False, no "eval 2"

    # --- every operand runs once, left to right, when all links are reached ---
    print(str(mark(4, 1) < mark(5, 2) < mark(6, 3)))  # eval 4, 5, 6, then True

    # --- short-circuit on the last link: the final operand is skipped ---
    print(str(mark(7, 1) < mark(8, 0) < mark(9, 5)))  # eval 7, 8, then False

    print("done")

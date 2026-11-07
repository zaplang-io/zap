# diff: match
# Regression (epic #374, FIXED in #741): `float(s)` on a malformed string now
# raises a catchable `ValueError`, matching `int(s)` (#456) and CPython, instead
# of aborting the process uncatchably. Both print `caught`, exit 0.
def main():
    try:
        auto x = float("not-a-number")
        print(x)
    except ValueError as e:
        print("caught")

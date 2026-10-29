# claim: TRUTH-1
# SPEC §5 (D-0022): truthiness is Python-style — zero is falsy, every non-zero
# number (int or float, positive or negative) is truthy. Exercised through
# `if`/`not` so the branch taken is the observable.
def main():
    if not 0:
        print("zero-falsy")
    if not 0.0:
        print("zerof-falsy")
    if 1:
        print("one-truthy")
    if -1:
        print("neg-truthy")
    if 3.5:
        print("float-truthy")

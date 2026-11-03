# claim: COMP-3
# SPEC §8 (#599): a comprehension takes any number of `for` clauses with
# interleaved `if` filters; nested `for`s iterate outer-to-inner, left to right,
# and each `if` filters the clauses to its left. Output is reduced to scalars.
def main():
    auto prod = [x * y for x in [1, 2] for y in [1, 2] if x != y]
    print(len(prod))
    print(prod[0])
    print(prod[0] + prod[1])

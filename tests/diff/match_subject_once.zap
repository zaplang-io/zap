# diff: match
# A `match` evaluates its subject exactly once, matching CPython (epic #374,
# walled garden): the side-effecting `subj()` prints "subj" a single time even
# though several `case` patterns are tested against its value. `lower_match`
# binds the scrutinee to one temporary before the arm chain, so later cases reuse
# it rather than re-evaluating.
def subj() -> int64:
    print("subj")
    return 2


def main():
    match subj():
        case 1:
            print("one")
        case 2:
            print("two")
        case _:
            print("other")

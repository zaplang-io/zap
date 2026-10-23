# Golden fixture: the grouped multi-import form (I#70, D-0052). The parentheses
# allow the list to span several lines (implicit continuation) and end with a
# trailing comma; each listed module desugars to its own import. `random` is
# seeded so the draw is deterministic.

import math
import random


def main():
    random.seed(7)
    print(int64(math.floor(9.9)))  # 9
    print(int64(math.ceil(0.1)))  # 1
    auto xs = [10, 20, 30]
    int64 c = random.choice(xs)  # seeded -> a member of xs
    if c == 10 or c == 20 or c == 30:
        print(1)
    else:
        print(0)

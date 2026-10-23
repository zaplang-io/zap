# Golden fixture: the comma-list multi-import form (I#70, D-0052):
# `import a, b` desugars to one import per module, so both stdlib modules are
# imported and usable from a single statement. `random` is seeded so the draw
# is deterministic across platforms and both collectors.
import math
import random

def main():
    random.seed(42)
    print(int64(math.sqrt(144.0)))  # 12
    print(int64(math.pow(2.0, 5.0)))  # 32
    int64 r = random.randint(1, 6)  # seeded -> in [1, 6]
    if r >= 1 and r <= 6:
        print(1)
    else:
        print(0)

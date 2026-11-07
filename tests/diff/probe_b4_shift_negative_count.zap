# diff: match
# PROBE (epic #374, suspected bug B4): a negative shift count. CPython raises a
# catchable `ValueError("negative shift count")`, so the handler runs and prints
# `caught`. If Zap instead returns poison/garbage (unmasked shift) the guarded
# `print` runs with a bogus value and `caught` never prints -> MATCH-FAILED
# reveals the behavior. CPython prints:
#   caught
def main():
    try:
        print(1 << -1)
    except ValueError as e:
        print("caught")

# diff: match
# Regression (epic #374, FIXED in #740): a negative shift count now raises a
# catchable ValueError (zap_ishl/zap_ishr -> zap_panic), matching CPython,
# instead of returning a poison value from an unmasked LLVM shift. Both print
# `caught`, exit 0.
def main():
    try:
        print(1 << -1)
    except ValueError as e:
        print("caught")

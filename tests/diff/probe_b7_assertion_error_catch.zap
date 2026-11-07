# diff: match
# Regression (epic #374, FIXED via #743/B7): a failed `assert` now raises a
# catchable AssertionError, so `except AssertionError` resolves and prints
# `caught`, matching CPython. Previously the name was unknown (ZAP3001) and the
# type did not exist.
def main():
    try:
        assert 1 == 2
        print("not-reached")
    except AssertionError as e:
        print("caught")

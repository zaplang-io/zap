# Golden (epic #374, B7): a failed `assert` raises a catchable AssertionError.
# It is caught by `except AssertionError`, and — being under `Exception` in the
# hierarchy — also by `except Exception`.
def main():
    try:
        assert 1 == 2
    except AssertionError as e:
        print("caught-assertion")
    try:
        assert 1 == 2
    except Exception as e:
        print("caught-exception")

# diff: match
# #1566: `str.index` of an absent substring raises a catchable `ValueError`,
# exactly like CPython, so the matching `except` runs and execution continues
# instead of the process aborting. CPython and Zap both print caught/continued
# and exit 0.
def main():
    try:
        print("x".index("y"))
    except ValueError:
        print("caught")
    print("continued")

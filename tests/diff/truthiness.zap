# diff: match
# Python-style truthiness of numbers and non-empty strings (D-0022): 0 and 0.0
# are falsy; non-zero numbers and non-empty strings are truthy. Exercised through
# `if`/`not` so the branch taken is the observable. (Two truthiness cases are NOT
# here because they diverge: EMPTY-string truthiness — `""` is truthy in Zap —
# is documented in string_truthiness.zap, and container truthiness crashes
# codegen, documented in collection_truthiness.zap.)
def main():
    if not 0:
        print("zero-falsy")
    if not 0.0:
        print("zerof-falsy")
    if 1:
        print("one-truthy")
    if "x":
        print("str-truthy")
    if 42:
        print("int-truthy")
    if -1:
        print("neg-truthy")
    if not 1 == 2:
        print("cmp-falsy")

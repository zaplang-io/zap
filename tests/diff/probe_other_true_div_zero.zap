# diff: match
# Regression (epic #374, FIXED via #743 decision): float `/` by zero now raises
# a catchable ZeroDivisionError (zap_fdiv), matching CPython, instead of
# yielding IEEE inf. Both print `caught`, exit 0.
def main():
    try:
        print(1.0 / 0.0)
    except Exception as e:
        print("caught")

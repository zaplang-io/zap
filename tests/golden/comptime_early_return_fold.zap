# #1686: a comptime-folded early `return` must not drag the statements that
# textually followed the folded `if` into the taken path. The supported
# specialization returns immediately; the fallback `raise` / side effect that
# came after the `if` is unreachable there and must not run. The unsupported
# specialization still reaches that fallback.

# Supported type returns its value; unsupported raises. In `resolve$str` the
# trailing `raise` must be dropped, so the supported call returns cleanly.
def resolve[T](T value) -> T:
    if reflect.type_info(T).name == "str":
        return value
    raise ValueError("unsupported")

# A side effect (not a raise) after the early return: it must not run on the
# supported path, and must run on the fallback path.
def describe[T](T value) -> str:
    if reflect.type_info(T).name == "str":
        return "supported"
    print("fallback-ran")
    return "unsupported"

def main():
    # Supported: returns "ok", prints no fallback side effect.
    print(resolve("ok"))
    # Supported describe: "supported", and "fallback-ran" must NOT appear.
    print(describe("hi"))
    # Unsupported describe: side effect runs, then "unsupported".
    print(describe(5))
    # Unsupported resolve raises; caught here.
    try:
        auto n = resolve(7)
        print(n)
    except ValueError:
        print("caught")

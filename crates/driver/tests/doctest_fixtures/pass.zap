"""Doctest fixtures for ``zap test --doc`` (I#234, D-0074).

Every fenced ``zap`` block in a docstring is a runnable example, checked by the
doctest runner against its ``# =>`` / ``# error:`` / ``# raises:`` expectations.

```zap
print(6 * 7)
# => 42
```
"""


def greet() -> str:
    """Return a greeting — the example asserts what it prints.

    ```zap
    print("hello")
    # => hello
    ```
    """
    return "hello"


def doubled(int64 n) -> int64:
    """Double a number. A trailing bare expression auto-prints (REPL semantics).

    ```zap
    auto n = 21
    n + n
    # => 42
    ```
    """
    return n + n


def listy() -> int64:
    """Multiple expected output lines, in order.

    ```zap
    auto xs = [1, 2, 3]
    print(len(xs))
    print(xs[0])
    # => 3
    # => 1
    ```
    """
    return 0


def checked() -> bool:
    """An example may assert a compile error or a runtime exception.

    A snippet that must not compile names its stable code:

    ```zap
    auto x = 1 + "a"
    # error: ZAP4012
    ```

    A snippet that must raise names a substring of the message:

    ```zap
    raise "boom"
    # raises: boom
    ```
    """
    return True


def undocumented_example() -> int64:
    "Prose only — no fenced block, so this item contributes no doctest."
    return 0

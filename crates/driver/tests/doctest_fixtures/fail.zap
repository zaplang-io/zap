"""A module with deliberately wrong examples, to exercise failure reporting.

```zap
print(1 + 1)
# => 3
```
"""


def wrong() -> str:
    """This example claims output the snippet does not produce.

    ```zap
    print("actual")
    # => expected
    ```
    """
    return "actual"

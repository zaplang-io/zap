# ticket #498 (pairs with #285/D-0026): an f-string replacement field may
# contain string literals that reuse the f-string's delimiter quote (PEP 701),
# e.g. an inline dict-key subscript or a call argument. An `Optional[T]` field
# renders its payload when present and `None` when absent, matching Python's
# `f"{d.get(k)}"`.
def main():
    dict[str, str] d = {"k": "v", "x": "y"}
    print(f'{d["k"]}')  # double-quote outer, inline subscript
    print(f"{d['x']}")  # single-quote outer
    print(f'{d.get("k")}')  # Optional[str] present -> payload
    print(f'{d.get("absent")}')  # Optional[str] absent -> None
    print(f'len={len("ab")}')  # call argument reusing the outer quote

# ticket #498 (pairs with #285/D-0026): an f-string replacement field may
# contain string literals that reuse the f-string's delimiter quote (PEP 701),
# e.g. an inline dict-key subscript or a call argument. The pre-#498
# escaped-delimiter workaround (`\"`/`\'`) keeps working too.
def main():
    dict[str, str] d = {"k": "v", "x": "y"}
    print(f'{d["k"]}')  # double-quote outer, inline subscript
    print(f"{d['x']}")  # single-quote outer
    print(f"{d[\"k\"]}")  # escaped double-quote (pre-#498 form)
    print(f'len={len("ab")}')  # call argument reusing the outer quote
    print(f"len={len(\"abc\")}")  # escaped call argument

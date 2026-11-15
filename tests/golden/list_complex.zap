# Golden fixture: collections of `complex` (#928). A `complex` value element is
# admitted in value positions (list element, dict value) as a dedicated element
# kind: physically a heap `ZapComplex` pointer, but it retains complex identity so
# read-back recovers `complex` (arithmetic, indexing, iteration) and the Display
# protocol renders it `(re±imj)` rather than as a bare pointer. (This is what lets
# `linalg.eigvals` return a `list[complex]`.) `complex` as a dict/set *key* stays a
# follow-up.

def main():
    # list[complex]: literal (constructor + imaginary-literal forms), Display,
    # indexing, and len.
    auto xs = [complex(1.0, 2.0), complex(3.0, -4.0), 5.0 + 0.0j]
    print("list", str(xs), len(xs))  # [(1+2j), (3-4j), (5+0j)] 3
    print("idx", str(xs[0]), str(xs[2]))  # (1+2j) (5+0j)

    # append + read back the pushed element as a complex.
    xs.append(complex(0.0, 1.0))
    print("push", str(xs[3]), len(xs))  # (0+1j) 4

    # iteration yields complex elements; arithmetic composes.
    auto total = complex(0.0, 0.0)
    for z in xs:
        total = total + z
    print("sum", str(total))  # (9-1j)

    # dict value position also admits complex.
    auto d = {"re": complex(2.0, 0.0), "im": complex(0.0, 3.0)}
    print("dict", str(d["re"]), str(d["im"]))  # (2+0j) (0+3j)

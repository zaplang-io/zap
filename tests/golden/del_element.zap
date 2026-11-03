# Golden fixture: `del` element removal (#595). `del xs[i]` removes a list
# element (negatives count from the end) and `del d[k]` removes a dict entry by
# key, reusing the collection's own storage ops. Behavior matches element
# access: an out-of-range list index raises the same catchable IndexError as a
# read `xs[i]`, and a missing dict key is a no-op (as the non-raising read is).
def main():
    list[int64] xs = [10, 20, 30, 40]
    del xs[1]
    print(len(xs))
    print(xs[0])
    print(xs[1])
    print(xs[2])
    del xs[-1]
    print(len(xs))
    print(xs[1])

    dict[str, int64] d = {"a": 1, "b": 2, "c": 3}
    del d["b"]
    print(len(d))
    print(d.get("a", -1))
    print(d.get("b", -1))
    print(d.get("c", -1))

    # Removing a missing key is a no-op (matches the non-raising dict read).
    del d["zzz"]
    print(len(d))

    # An out-of-range list `del` raises the same IndexError as a read and is
    # catchable (#383, D-0007).
    try:
        del xs[9]
    except Exception as e:
        print(e)
    print("done")

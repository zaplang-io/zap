# Golden fixture: negative list indexing (#398). `xs[-1]` counts from the end
# like a string index; positive indices are unchanged; negative indices address
# the store side too; and an out-of-range negative index is a catchable fault
# (#383) carrying the index as written, just like positive OOB.
def main():
    auto xs = [10, 20, 30, 40, 50]
    print(xs[-1])
    print(xs[-2])
    print(xs[-5])
    print(xs[0])
    print(xs[4])
    xs[-1] = 99
    print(xs[4])
    xs[-5] = 11
    print(xs[0])
    try:
        print(xs[-6])
    except Exception as e:
        print(e)
    print("done")

# Compile-fail fixture (#978, D-0236): heap.nlargest / nsmallest without a key=
# needs an orderable element type. A list of lists has no runtime ordering, so
# it is rejected cleanly at compile time — the fix is to pass a key= selecting
# an orderable key.
def main():
    auto xs = [[1], [2]]
    auto top = heap.nlargest(2, xs)
    print(len(top))

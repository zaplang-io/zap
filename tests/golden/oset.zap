# Golden fixture: insertion-ordered set as a generic collection type (#983).
#
# oset preserves first-insertion order (unlike the built-in hash `set`): get(i)
# visits elements in the order they were first added, duplicates are ignored,
# and contains is O(1). `oset[int64]()` is the generic constructor; the old
# int64-only `oset.new()` module API is gone (D-0208).
def main():
    auto s = oset[int64]()
    s.add(10)
    s.add(20)
    s.add(10)
    s.add(30)
    print(len(s))
    print(s.get(0))
    print(s.get(1))
    print(s.get(2))
    if s.contains(20):
        print(1)
    else:
        print(0)
    if s.contains(99):
        print(1)
    else:
        print(0)

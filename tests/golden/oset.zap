# Golden fixture: insertion-ordered set (ticket #102).
#
# oset preserves first-insertion order (unlike the built-in hash `set`): get(i)
# visits elements in the order they were first added, duplicates are ignored,
# and contains is O(1).
def main():
    auto s = oset.new()
    oset.add(s, 10)
    oset.add(s, 20)
    oset.add(s, 10)
    oset.add(s, 30)
    print(oset.len(s))
    print(oset.get(s, 0))
    print(oset.get(s, 1))
    print(oset.get(s, 2))
    if oset.contains(s, 20):
        print(1)
    else:
        print(0)
    if oset.contains(s, 99):
        print(1)
    else:
        print(0)

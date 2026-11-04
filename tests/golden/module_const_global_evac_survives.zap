# #663 (epic #661): an immutable reference `const` module global needs the
# static-root-table entry but NO write barrier — it is stored once at init and
# is never reassigned (ZAP3017). Under the precise Immix collector the collector
# still marks and relocates the structure the slot holds through that permanent
# root on every collection, so a `const` holding a heap structure survives
# repeated collections and evacuation pressure with its contents intact.
#
# A nested structure (a list of lists of strings) gives the moving collector
# several reference edges to relocate. Contents/identity assertions only, never
# heap-byte counts, so the Boehm and precise Immix legs agree.
const list[list[str]] GROUPS = [["alpha", "beta"], ["gamma"], ["delta", "epsilon", "zeta"]]

def churn():
    for i in range(0, 4000):
        auto junk = [str(i), str(i + 1)]
    gc.collect()

def main():
    auto rounds = 0
    while rounds < 8:
        churn()
        rounds += 1
    # After 8 collections under churn the const's backing lists and every string
    # element must still read back with correct contents.
    print(len(GROUPS))                 # 3
    print(GROUPS[0][0])                # alpha
    print(GROUPS[2][2])                # zeta
    auto names = 0
    auto chars = 0
    for group in GROUPS:
        for name in group:
            names += 1
            chars += len(name)
    print(names)                       # 2 + 1 + 3 = 6
    print(chars)                       # alpha5 beta4 gamma5 delta5 epsilon7 zeta4 = 30

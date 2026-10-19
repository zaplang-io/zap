# GC (ticket #48): a large live set survives repeated collections. `keep` holds
# 50k elements and stays reachable (a root) across a second loop that allocates
# a lot of garbage, forcing collections. Summing `keep` afterwards proves every
# live element was retained precisely (nothing dropped).
def main():
    auto keep = [i for i in range(0, 50000)]
    for j in range(0, 50000):
        auto garbage = [j, j, j]
    auto total = 0
    for x in keep:
        total = total + x
    print(total)

# GC (ticket #48): allocation stress. Each iteration allocates short-lived list
# objects, far exceeding the collector's threshold, so many collections run
# while the loop proceeds. The running total must stay correct across every
# safepoint/collection — a smoke test that collection never frees a live object
# or corrupts the heap. Runs identically under Boehm and the precise Immix GC.
def main():
    auto total = 0
    for i in range(0, 200000):
        auto xs = [i, i + 1, i + 2]
        total = total + xs[0] + xs[1] + xs[2]
    print(total)

# Compile-fail fixture: a channel stores its elements as uniform words, and a
# tuple is a value aggregate whose delivery across the channel ABI is not
# implemented in this slice, so `channel[(int64, int64)]()` is rejected cleanly
# (#703) — never an internal compiler error. (Unlike a set/dict key, which
# hashes a tuple through its key functions; tuple channels are a follow-up.)
def main():
    auto c = channel[int64, int64]()
    print(0)

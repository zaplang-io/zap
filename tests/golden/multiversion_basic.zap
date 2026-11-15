# Golden fixture: @multiversion function dispatch (#944, ISA-acceleration epic
# #139). A kernel is compiled into one clone per CPU-feature tier + a runtime
# resolver/dispatcher reading cpu_features (#943). Whichever clone the host
# actually selects, the RESULT must be identical — that correctness invariant is
# what makes multiversioning safe, and it is arch- and CPU-independent, so this
# golden's output is the same on every CI box. (The companion
# `multiversion_generic` fixture forces the baseline `default` clone via
# ZAP_CPU=generic and must print the same numbers — proving the fallback path and
# that a feature-absent host never hits a SIGILL.)
@multiversion("avx2", "sse4.2", "default")
def sum_sq(int64 n) -> int64:
    int64 acc = 0
    for i in range(0, n):
        acc = acc + i * i
    return acc

def main():
    print(sum_sq(0))
    print(sum_sq(1))
    print(sum_sq(5))
    print(sum_sq(10))
    print(sum_sq(100))

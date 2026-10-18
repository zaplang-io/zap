# Golden fixture: allocation-heavy string building, to exercise the Boehm
# collector (ticket #20). Each `tag` call allocates fresh string objects and
# byte buffers (via zap_str_concat / zap_str_new); the loop discards all but
# the last, so the program allocates ~hundreds of thousands of short-lived
# strings. Under GC it runs in bounded memory and prints the final result.
def tag(str name) -> str:
    return "[" + name + "]"

def main():
    auto last = ""
    for i in range(0, 200000):
        last = tag("zap")
    print(last)

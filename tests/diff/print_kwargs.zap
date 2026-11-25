# diff: match
# print()'s sep=/end=/flush= keyword options match CPython (#1458). file= (an
# int64 fd in Zap) is exercised in the golden suite, not here. flush= is a no-op
# on Zap's raw-fd path but valid and output-neutral in both languages.
def main():
    print("a", "b", sep=", ")
    print("x", "y", "z", sep="-")
    print(1, 2, 3, sep="")
    print("a", end="")
    print("b")
    print("end", end="!\n")
    print("p", "q", sep="|", end=";\n")
    print("flushed", flush=True)
    print()
    print("after-blank")

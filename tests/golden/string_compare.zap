# ticket #284 (subsumes #278): str comparison == != < <= > >=, substring `in`,
# and char equality under iteration.
def main():
    if "abc" == "abc":
        print("eq")
    if "abc" != "abd":
        print("ne")
    if "abc" < "abd":
        print("lt")
    if "abc" <= "abc":
        print("le")
    if "abd" > "abc":
        print("gt")
    if "abc" >= "abc":
        print("ge")
    if "lo" in "Hello":
        print("in")
    if "zz" in "Hello":
        print("bad")
    else:
        print("notin")
    auto s = "aAaA"
    int64 lower = 0
    for c in s:
        if c == chr(97):
            lower = lower + 1
    print(lower)

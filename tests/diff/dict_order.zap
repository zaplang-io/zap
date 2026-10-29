# diff: diverge D-0056: the built-in dict does NOT preserve insertion order. Zap
# iterates in open-addressing hash order (unspecified), while CPython 3.7+
# guarantees insertion order. An insertion-ordered map is the opt-in
# `collections` OrderedDict (D-0056); see design/0010-json.md and dict_iter.zap.
# Keys are chosen so the two orders differ; the joined key sequence is the
# observable. (Membership and lookup are order-independent and DO match — see
# membership_and_get.zap.)
def main():
    auto d = {"one": 1, "two": 2, "three": 3, "four": 4, "five": 5}
    list[str] order = []
    for k in d:
        order.append(k)
    print("-".join(order))

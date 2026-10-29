# diff: diverge BUG: an empty string is truthy in Zap. CPython truthiness
# (D-0022, which Zap claims to follow) makes "" falsy, but Zap treats every
# string as truthy regardless of length, so `if "":` takes the then-branch and
# `not ""` is False. Numeric falsiness (0, 0.0) and non-empty-string truthiness
# DO match (see truthiness.zap) — only the empty-string case diverges. A real
# bug to triage. CPython: empty / notfalse; Zap: nonempty / isfalse... (differs).
def tag(bool b) -> str:
    if b:
        return "T"
    return "F"


def main():
    if "":
        print("nonempty-branch")
    else:
        print("empty-branch")
    print(tag(not ""))

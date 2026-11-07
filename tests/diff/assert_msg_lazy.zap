# diff: match
# The `assert cond, msg` message is evaluated lazily — only when the assertion
# fails — matching CPython (epic #374, walled garden). The condition here is
# true, so the side-effecting `tag()` message is never called and "evaluated"
# never prints; only "ok" does.
def tag() -> str:
    print("evaluated")
    return "bad"


def main():
    assert True, tag()
    print("ok")

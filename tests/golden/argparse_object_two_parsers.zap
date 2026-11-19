# Golden fixture: two independent ArgumentParser values do not clobber each
# other (#997) — the reentrancy the process-global singleton made impossible.
def main():
    auto a = argparse.ArgumentParser(prog="a")
    a.add_argument("--x", default="ax")
    auto b = argparse.ArgumentParser(prog="b")
    b.add_argument("--x", default="bx")
    auto na = a.parse_args(["--x", "one"])
    auto nb = b.parse_args(["--x", "two"])
    print(na.get("x"))
    print(nb.get("x"))
    auto na2 = a.parse_args([])
    print(na2.get("x"))
    print(nb.get("x"))

# Golden fixture: an unsatisfied required option raises a catchable error that,
# left uncaught, unwinds to the top frame (exit 1, message on stderr) — the
# object API never calls exit() itself, so a library parser never kills its host
# (#997). "after" is never printed.
def main():
    auto p = argparse.ArgumentParser(prog="tool")
    p.add_argument("--port", type="int", required=True)
    print("before")
    auto ns = p.parse_args([])
    print("after")

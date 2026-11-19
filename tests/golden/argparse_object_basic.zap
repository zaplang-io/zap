# Golden fixture: argparse ArgumentParser object (#997, D-0238) — a boolean flag,
# a string option, a typed int/float option and a positional, parsed from a
# literal argv that (unlike the flat `argparse.parse`) does NOT include the
# program name, mirroring Python's parse_args(sys.argv[1:]).
def main():
    auto p = argparse.ArgumentParser(prog="tool", description="demo")
    p.add_argument("--verbose", action="store_true")
    p.add_argument("--name", default="anon")
    p.add_argument("--count", type="int", default="1")
    p.add_argument("--ratio", type="float", default="0.5")
    p.add_argument("src")
    auto ns = p.parse_args(
        ["--verbose", "--name", "zap", "--count", "3", "--ratio", "2.5", "input.txt"],
    )
    print(ns.get_flag("verbose"))
    print(ns.get_str("name"))
    print(ns.get_int("count"))
    print(ns.get_float("ratio"))
    print(ns.get("src"))
    print(ns.has("name"))
    print(ns.has("verbose"))

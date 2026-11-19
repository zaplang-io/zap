# Golden fixture: git-style subcommands (#997). Each child parser owns its own
# arguments; the chosen command is stored under the subparsers `dest`, and the
# remaining argv is dispatched to the matching child. The same parent parser is
# reused for a second, different subcommand (reentrancy the singleton blocked).
def main():
    auto p = argparse.ArgumentParser(prog="git")
    p.add_argument("--global-flag", action="store_true")
    auto subs = p.add_subparsers(dest="cmd")
    auto build = subs.add_parser("build")
    build.add_argument("--release", action="store_true")
    build.add_argument("target")
    auto test = subs.add_parser("test")
    test.add_argument("--filter", default="all")
    auto ns1 = p.parse_args(["--global-flag", "build", "--release", "app"])
    print(ns1.get("cmd"))
    print(ns1.get_flag("global_flag"))
    print(ns1.get_flag("release"))
    print(ns1.get("target"))
    auto ns2 = p.parse_args(["test", "--filter", "unit"])
    print(ns2.get("cmd"))
    print(ns2.get("filter"))

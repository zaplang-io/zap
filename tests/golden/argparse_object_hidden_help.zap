# Golden fixture: `format_help()` visibility with hidden arguments (#1702).
# Covers a mix of visible and hidden options and positionals, an argument group
# with both visible and hidden members (the heading stays, hidden members drop
# out and do not shift the visible rows), a group whose members are all hidden
# (no heading at all), and a subparser child parser that inherits the same
# visibility rules. Deterministic text for tooling/goldens.
def main():
    auto p = argparse.ArgumentParser(prog="tool", description="A demo tool.")
    p.add_argument("--visible", action="store_true", help="Public flag")
    p.add_argument(
        "--internal",
        action="store_true",
        hidden=True,
        help="Compatibility flag",
    )
    p.add_argument("--name", default="anon", help="your name")
    p.add_argument("--legacy", hidden=True, help="old option")
    p.add_argument("src", help="source file")
    p.add_argument("shadow", hidden=True, help="hidden positional")
    auto g = p.add_argument_group("global options")
    g.add_argument("--config", help="config path")
    g.add_argument("--debug", action="store_true", hidden=True, help="debug output")
    auto compat = p.add_argument_group("compat options")
    compat.add_argument("--compat-a", action="store_true", hidden=True)
    compat.add_argument("--compat-b", hidden=True)
    print(p.format_help())
    print("---")
    auto git = argparse.ArgumentParser(prog="git")
    auto subs = git.add_subparsers(dest="command")
    auto build = subs.add_parser("build")
    build.add_argument("--target", help="target triple")
    build.add_argument(
        "--experimental",
        action="store_true",
        hidden=True,
        help="experimental",
    )
    print(build.format_help())

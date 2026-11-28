# Golden fixture (#1713): CPython-parity HELP rendering of the argparse object
# parser — epilog, choices shown as `{a,b,c}` in both usage and help, the
# `%(default)s` / `%(prog)s` help-string placeholders, nargs usage display, an
# option alias shown with both spellings, an argument-group description (B3),
# and a titled subparsers section with a metavar, a description, and an alias.
def main():
    auto p = argparse.ArgumentParser(
        prog="prog",
        description="Do things with %(prog)s.",
    )
    p.set_epilog("See %(prog)s --help.")
    p.add_argument("--mode", choices=["debug", "release"], help="build mode")
    p.add_argument(
        "--jobs",
        type="int",
        default="4",
        help="jobs (default: %(default)s)",
    )
    p.add_argument("src", nargs="+", help="sources")
    print(p.format_help())
    print("---")
    auto g = argparse.ArgumentParser(prog="git")
    auto net = g.add_argument_group("network", description="connection options")
    net.add_argument("--retries", type="int", help="retry count")
    g.add_argument("--verbose", "-v", action="store_true", help="loud")
    auto subs = g.add_subparsers(
        dest="cmd",
        title="commands",
        description="pick one",
        metavar="CMD",
        required=True,
    )
    subs.add_parser("build", aliases=["b"])
    subs.add_parser("test")
    print(g.format_help())

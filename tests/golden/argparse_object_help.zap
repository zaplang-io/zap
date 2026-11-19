# Golden fixture: format_help() rendering (#997) — usage line, description,
# positional + options sections, and (second parser) an argument-group heading
# plus a subcommands choice list. Deterministic text for tooling/goldens.
def main():
    auto p = argparse.ArgumentParser(prog="tool", description="A demo tool.")
    p.add_argument("--verbose", action="store_true", help="be loud")
    p.add_argument("--name", default="anon", help="your name")
    p.add_argument("src", help="source file")
    print(p.format_help())
    print("---")
    auto g = argparse.ArgumentParser(prog="git")
    auto grp = g.add_argument_group("global options")
    grp.add_argument("--config", help="config path")
    auto subs = g.add_subparsers(dest="command")
    subs.add_parser("build")
    subs.add_parser("test")
    print(g.format_help())

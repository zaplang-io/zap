# Golden fixture: `ArgumentParser.format_usage()` (#1710) returns the usage
# synopsis on its own — the single `usage:` line, with no trailing newline and
# none of the description or option rows that `format_help()` adds around it.
# The walled garden here is that `format_usage()` is byte-for-byte the first
# line of `format_help()` (they share one builder), that the method is pure (it
# neither parses argv nor prints nor exits), and that every synopsis element —
# plain options, store_true flags, positionals, custom metavars, custom program
# names, hidden arguments (#1702), grouped options, and the subcommand selector
# — appears exactly as it does in the help synopsis.
def plain_options_and_positionals():
    # Plain value option, a store_true flag, and a positional, with a prog name.
    auto p = argparse.ArgumentParser(prog="demo", description="Full description")
    p.add_argument("--jobs", type="int")
    p.add_argument("--verbose", action="store_true")
    p.add_argument("src")
    print(p.format_usage())


def no_program_name():
    # With no prog set, the synopsis omits the program name entirely.
    auto p = argparse.ArgumentParser()
    p.add_argument("--name")
    print(p.format_usage())


def only_help_flag():
    # A bare parser still lists the implicit `-h`.
    auto p = argparse.ArgumentParser(prog="bare")
    print(p.format_usage())


def custom_metavar():
    # A configured metavar overrides the dest-derived default in the synopsis.
    auto p = argparse.ArgumentParser(prog="meta")
    p.add_argument("--out", metavar="FILE")
    p.add_argument("input", metavar="PATH")
    print(p.format_usage())


def hidden_absent_from_usage():
    # Hidden arguments (#1702) parse normally but never appear in the synopsis,
    # whether optional or positional.
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--visible", action="store_true")
    p.add_argument("--internal", action="store_true", hidden=True)
    p.add_argument("shown")
    p.add_argument("secret", hidden=True)
    print(p.format_usage())


def grouped_options():
    # Options registered through an argument group still appear in the synopsis
    # (groups only affect the help-body sectioning, not usage).
    auto g = argparse.ArgumentParser(prog="git")
    auto grp = g.add_argument_group("global options")
    grp.add_argument("--config")
    print(g.format_usage())


def subcommand_selector():
    # The subcommands selector renders as `<dest> ...` at the end of the line.
    auto g = argparse.ArgumentParser(prog="git")
    g.add_argument("--config")
    auto subs = g.add_subparsers(dest="command")
    subs.add_parser("build")
    subs.add_parser("test")
    print(g.format_usage())


def subcommand_custom_dest():
    # A custom subparsers dest appears verbatim in the selector slot.
    auto g = argparse.ArgumentParser(prog="tool")
    auto subs = g.add_subparsers(dest="verb")
    subs.add_parser("run")
    print(g.format_usage())


def matches_format_help_first_line():
    # format_usage() is exactly the first line of format_help(): shared builder,
    # no drift. Compare against the help output split on the first newline.
    auto p = argparse.ArgumentParser(prog="demo", description="Full description")
    p.add_argument("--jobs", type="int")
    p.add_argument("src")
    auto usage = p.format_usage()
    auto help = p.format_help()
    auto nl = help.find("\n")
    auto first = help[:nl]
    print(usage == first)


def no_trailing_newline():
    # The synopsis carries no trailing newline of its own.
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--jobs", type="int")
    auto usage = p.format_usage()
    print(usage.endswith("\n"))
    print(usage.endswith("]"))


def pure_no_side_effects():
    # Calling format_usage() neither parses argv nor mutates the parser: a
    # subsequent parse still succeeds and repeated calls are identical.
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--jobs", type="int", default="1")
    auto a = p.format_usage()
    auto b = p.format_usage()
    print(a == b)
    auto ns = p.parse_args(["--jobs", "4"])
    print(ns.get_int("jobs"))
    print(p.format_usage() == a)


def main():
    plain_options_and_positionals()
    print("---")
    no_program_name()
    print("---")
    only_help_flag()
    print("---")
    custom_metavar()
    print("---")
    hidden_absent_from_usage()
    print("---")
    grouped_options()
    print("---")
    subcommand_selector()
    print("---")
    subcommand_custom_dest()
    print("---")
    matches_format_help_first_line()
    no_trailing_newline()
    pure_no_side_effects()

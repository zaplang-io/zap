# Golden fixture (#1712): `ArgumentParser.completion_context(words, prefix)` — a
# read-only query that reports what the cursor word completes in a partial
# command line, reusing the core token matching (subcommand selection, value
# options vs flags, `--option=value`, `--`). The cursor word is treated as
# incomplete: a partial value is never validated as a full choice and the
# child's required arguments are never enforced. Each case dumps kind,
# active_option, value_prefix, active_dest, the selected command path, the
# candidate list, options_allowed, and whether a COMPLETED word was invalid.
def j(list[str] xs) -> str:
    auto s = ""
    auto f = True
    for x in xs:
        if f:
            s = x
            f = False
        else:
            s = s + "," + x
    return s


def build_parser() -> ArgumentParser:
    auto p = argparse.ArgumentParser(prog="tool")
    p.add_argument("--global", action="store_true")
    auto subs = p.add_subparsers(dest="cmd")
    auto build = subs.add_parser("build")
    build.add_argument("--mode", choices=["debug", "release"])
    build.add_argument("target")
    subs.add_parser("test")
    return p


def dump(CompletionContext c):
    print(c.kind)
    print(c.active_option)
    print(c.value_prefix)
    print(c.active_dest)
    print(j(c.command_path))
    print(j(c.candidates))
    print(c.options_allowed)
    print(c.invalid)


def main():
    auto p = build_parser()
    # Pending value of a value-taking option after a subcommand, filtered by the
    # partial choice prefix (never validated as a full choice).
    dump(p.completion_context(["build", "--mode"], "re"))
    print("---")
    # Attached `--option=valuepart`: option and value prefix split out.
    dump(p.completion_context(["build"], "--mode=re"))
    print("---")
    # A child option name after subcommand selection.
    dump(p.completion_context(["build"], "--mo"))
    print("---")
    # An empty cursor word completes the next positional of the child.
    dump(p.completion_context(["build"], ""))
    print("---")
    # A positional after the `--` separator (options no longer allowed).
    dump(p.completion_context(["build", "--"], ""))
    print("---")
    # A root-level option name.
    dump(p.completion_context([], "--g"))
    print("---")
    # An invalid COMPLETED word is reported without validating the cursor word.
    dump(p.completion_context(["bogus"], "x"))

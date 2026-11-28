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


def build_q() -> ArgumentParser:
    # A flat parser (no subcommands) for the completed-value-word cases (#1715).
    auto q = argparse.ArgumentParser(prog="q")
    q.add_argument("--mode", choices=["debug", "release"])
    q.add_argument("--release", action="store_true")
    q.add_argument("pos")
    return q


def build_h() -> ArgumentParser:
    # A parser with a hidden choice option and a visible one (#1716).
    auto h = argparse.ArgumentParser(prog="h")
    h.add_argument("--internal-mode", choices=["secret-mode"], hidden=True)
    h.add_argument("--visible-mode", choices=["vis-a", "vis-b"])
    return h


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
    print("===")
    auto q = build_q()
    # #1715: a completed option-like word cannot satisfy a required value, so it
    # is invalid (matching strict parse_args), the option is reprocessed, and the
    # cursor word becomes the next positional. Named option, unknown
    # option-shaped word, and the `--` separator in the value position.
    dump(q.completion_context(["--mode", "--release"], ""))
    print("---")
    dump(q.completion_context(["--mode", "--bogus"], ""))
    print("---")
    dump(q.completion_context(["--mode", "--"], ""))
    print("---")
    # The corresponding INCOMPLETE cursor value is still a valid value prefix.
    dump(q.completion_context(["--mode"], ""))
    print("===")
    auto h = build_h()
    # #1716: a hidden option offers no choice suggestions, by exact spelling or
    # attached value; a visible option is unaffected.
    dump(h.completion_context(["--internal-mode"], "sec"))
    print("---")
    dump(h.completion_context([], "--internal-mode=sec"))
    print("---")
    dump(h.completion_context(["--visible-mode"], "vis"))
    print("---")
    dump(h.completion_context([], "--visible-mode=vis"))

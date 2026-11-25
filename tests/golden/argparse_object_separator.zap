# Golden fixture: the `--` end-of-options separator (#1507). After `--`, every
# remaining token fills a declared positional LITERALLY (dashed filenames, a
# literal `--help`, even a literal `--`). The separator is consumed and does not
# itself fill a positional; ordinary missing/extra-positional checks still apply.
# Mirrors Python's argparse.ArgumentParser.parse_args.


# Without a separator, an option-like token is NOT a positional: it is looked up
# as an option and, being undeclared, rejected.
def rejects_option_like_positional_without_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("path")
    try:
        p.parse_args(["--literal"])
        print("no-error")
    except Exception:
        print("no-sep-caught")


# After `--`, a leading-dash token binds the positional literally.
def accepts_dash_positional_after_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("path")
    auto ns = p.parse_args(["--", "--literal"])
    print(ns.get_str("path"))


# A literal `--help` after `--` is a value, not a help request.
def literal_help_after_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("path")
    auto ns = p.parse_args(["--", "--help"])
    print(ns.get_str("path"))
    print(ns.help_requested())


# A dashed filename after `--`.
def dashed_filename_after_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("path")
    auto ns = p.parse_args(["--", "-rf"])
    print(ns.get_str("path"))


# The separator itself does not fill a positional: `["--"]` leaves `path` unmet.
def separator_does_not_fill_positional():
    auto p = argparse.ArgumentParser()
    p.add_argument("path")
    try:
        p.parse_args(["--"])
        print("no-error")
    except Exception:
        print("sep-missing-caught")


# Two positionals filled literally after the separator, in order.
def two_positionals_after_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("first")
    p.add_argument("second")
    auto ns = p.parse_args(["--", "-a", "-b"])
    print(ns.get_str("first"))
    print(ns.get_str("second"))


# Too many positionals after the separator still raises.
def extra_positional_after_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("only")
    try:
        p.parse_args(["--", "one", "two"])
        print("no-error")
    except Exception:
        print("sep-extra-caught")


# Only the FIRST `--` is the separator; a second `--` is a literal value.
def second_separator_is_literal():
    auto p = argparse.ArgumentParser()
    p.add_argument("path")
    auto ns = p.parse_args(["--", "--"])
    print(ns.get_str("path"))


# Options are honored before the separator; positionals after it are literal.
def options_before_separator():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name", default="anon")
    p.add_argument("path")
    auto ns = p.parse_args(["--name", "zap", "--", "-pos"])
    print(ns.get_str("name"))
    print(ns.get_str("path"))


def main():
    rejects_option_like_positional_without_separator()
    accepts_dash_positional_after_separator()
    literal_help_after_separator()
    dashed_filename_after_separator()
    separator_does_not_fill_positional()
    two_positionals_after_separator()
    extra_positional_after_separator()
    second_separator_is_literal()
    options_before_separator()

# Golden fixture (#1713): CPython-parity ERROR paths of the argparse object
# parser, each a catchable raise (never a process exit). Covers B2 (required on
# a positional is a declaration error), a required subparser, an ambiguous
# long-option abbreviation, a missing nargs-`+` value, an invalid choice, an
# invalid typed value, and a mutually-exclusive conflict. A walled garden: every
# one of these stays a catchable error, so a library parser never kills its host.
def required_positional_decl():
    try:
        auto p = argparse.ArgumentParser(prog="p")
        p.add_argument("pos", required=True)
        print("no-error")
    except Exception:
        print("b2-caught")


def required_subparser():
    auto p = argparse.ArgumentParser(prog="p")
    auto s = p.add_subparsers(dest="cmd", required=True)
    s.add_parser("go")
    try:
        p.parse_args([])
        print("no-error")
    except Exception:
        print("sub-required-caught")
    print(p.parse_args(["go"]).get("cmd"))


def ambiguous_abbrev():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--foobar", action="store_true")
    p.add_argument("--foobaz", action="store_true")
    try:
        p.parse_args(["--foob"])
        print("no-error")
    except Exception:
        print("ambiguous-caught")


def nargs_plus_missing():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--vals", nargs="+")
    try:
        p.parse_args(["--vals"])
        print("no-error")
    except Exception:
        print("plus-missing-caught")


def bad_choice_and_type():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--mode", choices=["a", "b"])
    p.add_argument("--n", type="int")
    try:
        p.parse_args(["--mode", "c"])
        print("no-error")
    except Exception:
        print("bad-choice-caught")
    try:
        p.parse_args(["--n", "x"])
        print("no-error")
    except Exception:
        print("bad-type-caught")


def mx_conflict():
    auto p = argparse.ArgumentParser(prog="p")
    auto mx = p.add_mutually_exclusive_group()
    mx.add_argument("--json", action="store_true")
    mx.add_argument("--yaml", action="store_true")
    try:
        p.parse_args(["--json", "--yaml"])
        print("no-error")
    except Exception:
        print("mx-caught")


def main():
    required_positional_decl()
    required_subparser()
    ambiguous_abbrev()
    nargs_plus_missing()
    bad_choice_and_type()
    mx_conflict()

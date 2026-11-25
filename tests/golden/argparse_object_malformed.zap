# Golden fixture: malformed option inputs (#1508). A value-taking option is not
# satisfied by a following option-like token (known or unknown), and a zero-arity
# flag rejects an attached value. Explicit attached values and negative-integer
# values keep parsing. Mirrors Python's argparse.ArgumentParser.parse_args.


# A following KNOWN option does not silently satisfy a string option.
def rejects_missing_value_before_known_option():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name")
    p.add_argument("--verbose", action="store_true")
    try:
        p.parse_args(["--name", "--verbose"])
        print("no-error")
    except Exception:
        print("known-opt-caught")


# A following UNKNOWN option does not satisfy it either.
def rejects_missing_value_before_unknown_option():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name")
    try:
        p.parse_args(["--name", "--nope"])
        print("no-error")
    except Exception:
        print("unknown-opt-caught")


# A value-taking option at end of argv with no value raises.
def rejects_missing_value_at_end():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name")
    try:
        p.parse_args(["--name"])
        print("no-error")
    except Exception:
        print("end-caught")


# store_true takes zero values: an attached value is rejected.
def rejects_attached_value_on_store_true():
    auto p = argparse.ArgumentParser()
    p.add_argument("--verbose", action="store_true")
    try:
        p.parse_args(["--verbose=false"])
        print("no-error")
    except Exception:
        print("store-true-caught")


# store_false takes zero values too.
def rejects_attached_value_on_store_false():
    auto p = argparse.ArgumentParser()
    p.add_argument("--quiet", action="store_false")
    try:
        p.parse_args(["--quiet=yes"])
        print("no-error")
    except Exception:
        print("store-false-caught")


# An empty attached value (`--verbose=`) on a flag is still an attached value.
def rejects_empty_attached_on_flag():
    auto p = argparse.ArgumentParser()
    p.add_argument("--verbose", action="store_true")
    try:
        p.parse_args(["--verbose="])
        print("no-error")
    except Exception:
        print("empty-attached-caught")


# An explicit attached value may itself look like an option.
def accepts_attached_option_like_value():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name")
    auto ns = p.parse_args(["--name=--verbose"])
    print(ns.get_str("name"))


# An explicit empty attached value binds the empty string.
def accepts_attached_empty_value():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name", default="anon")
    auto ns = p.parse_args(["--name="])
    print("[" + ns.get_str("name") + "]")


# A negative integer is a value, not an option.
def accepts_negative_int_value():
    auto p = argparse.ArgumentParser()
    p.add_argument("--count", type="int")
    auto ns = p.parse_args(["--count", "-3"])
    print(ns.get_int("count"))


# A bare dash is a conventional value (stdin), not an option.
def accepts_bare_dash_value():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name")
    auto ns = p.parse_args(["--name", "-"])
    print(ns.get_str("name"))


def main():
    rejects_missing_value_before_known_option()
    rejects_missing_value_before_unknown_option()
    rejects_missing_value_at_end()
    rejects_attached_value_on_store_true()
    rejects_attached_value_on_store_false()
    rejects_empty_attached_on_flag()
    accepts_attached_option_like_value()
    accepts_attached_empty_value()
    accepts_negative_int_value()
    accepts_bare_dash_value()

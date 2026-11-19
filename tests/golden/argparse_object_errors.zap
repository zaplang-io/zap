# Golden fixture: parse-error surface (#997), each caught to a stable marker.
# Covers unknown argument, missing required option, missing positional, bad
# choice, bad typed value, an option missing its value, and an unsatisfied
# required mutually-exclusive group.
def unknown_arg():
    auto p = argparse.ArgumentParser()
    p.add_argument("--x")
    try:
        p.parse_args(["--y", "1"])
        print("no-error")
    except Exception:
        print("unknown-caught")

def missing_required():
    auto p = argparse.ArgumentParser()
    p.add_argument("--port", type="int", required=True)
    try:
        p.parse_args([])
        print("no-error")
    except Exception:
        print("required-caught")

def missing_positional():
    auto p = argparse.ArgumentParser()
    p.add_argument("src")
    try:
        p.parse_args([])
        print("no-error")
    except Exception:
        print("positional-caught")

def bad_choice():
    auto p = argparse.ArgumentParser()
    p.add_argument("--mode", choices=["fast", "slow"])
    try:
        p.parse_args(["--mode", "warp"])
        print("no-error")
    except Exception:
        print("choice-caught")

def bad_int():
    auto p = argparse.ArgumentParser()
    p.add_argument("--n", type="int")
    try:
        p.parse_args(["--n", "notanint"])
        print("no-error")
    except Exception:
        print("int-caught")

def missing_value():
    auto p = argparse.ArgumentParser()
    p.add_argument("--opt")
    try:
        p.parse_args(["--opt"])
        print("no-error")
    except Exception:
        print("value-caught")

def mx_required_missing():
    auto p = argparse.ArgumentParser()
    auto mx = p.add_mutually_exclusive_group(required=True)
    mx.add_argument("--a", action="store_true")
    mx.add_argument("--b", action="store_true")
    try:
        p.parse_args([])
        print("no-error")
    except Exception:
        print("mx-required-caught")

def main():
    unknown_arg()
    missing_required()
    missing_positional()
    bad_choice()
    bad_int()
    missing_value()
    mx_required_missing()

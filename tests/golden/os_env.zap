# Golden fixture: os — environment get/set + hasenv (ticket #97). `os.getenv`
# returns "" for an unset name; `os.hasenv` distinguishes unset from empty.
# Predicates are exercised via `if` (their idiomatic use).
def main():
    os.setenv("ZAP_TEST_97", "hello")
    print(os.getenv("ZAP_TEST_97"))
    if os.hasenv("ZAP_TEST_97"):
        print("has_set")
    if os.hasenv("ZAP_UNSET_97"):
        print("has_unset")
    else:
        print("no_unset")
    print("[" + os.getenv("ZAP_UNSET_97") + "]")

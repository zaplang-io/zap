# diff: diverge #743: no AssertionError type (except AssertionError -> ZAP3001) — CONFIRMED by PR #738 CI, decision pending (B7)
# PROBE (epic #374, suspected bug B7): there is no `AssertionError` in the
# compiler, stdlib, or SPEC §10, so it is unclear what `assert` raises and
# whether `except AssertionError` resolves at all. CPython raises AssertionError,
# the handler runs, and it prints `caught` (exit 0). If Zap rejects the name at
# compile time, or `assert` raises something this handler does not match, the
# run diverges and the report shows how. CPython prints:
#   caught
def main():
    try:
        assert 1 == 2
        print("not-reached")
    except AssertionError as e:
        print("caught")

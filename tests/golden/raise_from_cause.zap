# Golden fixture: `raise ... from ...` explicit cause chaining (#604, D-0103,
# epic #614 D12). A caught `ValueError` is re-raised as a `RuntimeError` with an
# explicit cause; the uncaught chained exception renders a two-section
# traceback — the cause's own section, the "direct cause" separator, then the
# raised exception — and exits non-zero. The harness checks the exit code
# (raise_from_cause.exit) and that stderr contains the two-section chain
# (raise_from_cause.stderr); stdout is matched as usual.
def main() -> int64:
    print("start")
    try:
        raise ValueError("original failure")
    except Exception as e:
        raise RuntimeError("wrapped failure") from e
    return 0

# Golden fixture: an exception with no matching handler unwinds to the top frame
# and aborts with a message and a non-zero exit code (#22, D-0007). The implicit
# top-level handler in `main` reports it via zap_uncaught. The harness checks the
# exit code (exceptions_uncaught.exit) and that stderr contains the message
# (exceptions_uncaught.stderr); stdout is matched as usual.
def main() -> int64:
    print("start")
    raise "fatal error"
    return 0

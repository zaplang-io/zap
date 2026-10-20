# Golden fixture: logging — configurable line format + stream redirect
# (ticket #101). Output is directed to stdout (sys.stdout_fd()) so the exact
# formatted lines are compared directly; no `print()` is mixed in (print uses
# buffered stdio, logging writes raw, so they must not share a stream here).
def main():
    logging.set_stream(sys.stdout_fd())
    logging.set_level(logging.DEBUG)
    logging.set_format("[%(levelname)s] %(message)s")
    logging.debug("starting")
    logging.info("hello")
    logging.warning("careful")

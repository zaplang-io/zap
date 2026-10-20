# Golden fixture: logging — leveled emit + threshold filtering (ticket #101).
# Default stream is stderr and the default format is "%(levelname)s:%(message)s"
# (no timestamp, deterministic). The log lines are asserted via the `.stderr`
# companion; stdout carries only the final `get_level()` readback.
def main():
    logging.set_level(logging.DEBUG)
    logging.debug("d1")
    logging.info("i1")
    logging.warning("w1")
    # Raise the threshold: debug/info now dropped, warning and above kept.
    logging.set_level(logging.WARNING)
    logging.debug("drop_me")
    logging.info("drop_me_too")
    logging.warning("w2")
    logging.error("e2")
    logging.log(logging.CRITICAL, "c2")
    print(logging.get_level())

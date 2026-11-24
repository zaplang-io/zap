# Golden fixture (#1409): stop() ends the stream. We receive the first tick, call
# stop(), then watch for another tick against a generous timeout. The interval
# (50ms) is small relative to the guard (500ms) and stop() lands ~an interval
# before the next tick, so a working stop() means the guard fires first. If stop
# left the firing loop running, a tick would arrive well within 500ms and the
# WRONG branch would print.
import time

def main():
    auto tk = time.ticker(time.millis(50))
    auto first = <-tk.chan
    tk.stop()
    select:
        case late = <-tk.chan:
            print("WRONG: tick after stop")
        case guard = <-time.after(time.millis(500)):
            print("stopped")

# Golden fixture (#1395): a timer that is never received must not delay program
# exit. `time.after` with a one-hour duration arms a firing coroutine, but it is
# `spawn detached` (no owning scope), so it is terminated when `main` returns and
# is NOT part of the program-exit join. The program therefore prints and exits
# immediately; if the pending timer stalled exit, this fixture would time out
# instead of producing its single line.
import time

def main():
    auto timer = time.after(time.seconds(3600))
    print("exited without waiting")

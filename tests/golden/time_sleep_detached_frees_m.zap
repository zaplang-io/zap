# Golden fixture (#1360): a coroutine with NO owning scope still parks its G on
# the fd-less scheduler timer and frees its worker M. A `spawn detached`
# coroutine opts out of scope ownership (g->scope == NULL), so before #1360 its
# `time.sleep` took the blocking nanosleep and pinned the worker; #1360 widened
# the cooperative timer park (added for in-scope sleeps in #1357) to EVERY
# coroutine sleep, scope or not.
#
# Pinned to ONE worker (MAXPROCS=1, see the .env). The detached sleeper announces
# it is about to sleep (`entered`, buffered) and then sleeps 100 ms. `main` then
# spawns an owned `sibling` onto the same single worker; the sibling records
# whether the sleeper has already woken. If the sleep freed the M, the sibling
# runs DURING the sleep and sees `sleeper_woke == 0`; if the sleep pinned the M
# (the pre-#1360 blocking path), the sibling cannot run until the sleeper wakes
# at 100 ms and would see `1`. The recorded flag is the assertion.
import time

def sleeper(channel[int64] entered, channel[int64] woke, Atomic[int64] sleeper_woke):
    entered <- 1
    time.sleep_for(time.millis(100))
    sleeper_woke.store(1)
    woke <- 1

def sibling(Atomic[int64] sleeper_woke, Atomic[int64] sib_saw, channel[int64] done):
    sib_saw.store(sleeper_woke.load())
    done <- 1

def main():
    Atomic[int64] sleeper_woke = Atomic(0)
    Atomic[int64] sib_saw = Atomic(0)
    auto entered = channel[int64](1)
    auto woke = channel[int64](1)
    auto done = channel[int64](1)
    scope:
        spawn detached sleeper(entered, woke, sleeper_woke)
        auto e = <-entered
        spawn sibling(sleeper_woke, sib_saw, done)
        auto d = <-done
    auto w = <-woke
    if sib_saw.load() == 0:
        print("detached sleeper freed its worker")
    else:
        print("WRONG: detached sleeper pinned its worker")

# Golden fixture: stress the `select` claim/resume race behind `time.ticker`
# under multiple worker Ms (regression for the aarch64 merge-queue flake where
# the golden suite aborted with glibc `tpp.c:86 (__pthread_tpp_change_priority)`).
#
# Each `time.ticker` spawns the prelude firing loop `__ticker_fire`, whose body
# runs a `select` over its stop channel and a fresh `time.after` timer every
# iteration. When a channel counterpart CLAIMS a select case it wakes the
# select'ing coroutine (`coready(sel->pw)`) while still holding the select's
# STACK-LOCAL park mutex `sel.mu`, releasing it only afterwards. With more than
# one worker M the woken coroutine can resume on another M and return from
# `zap_select` — reclaiming and reusing that stack frame — before the claimer's
# trailing unlock runs, so the unlock scribbles pthread-mutex bytes over the
# reused frame. On x86 that surfaced as a corrupted allocation size in the next
# spawn; on aarch64 the reused mutex's `__kind` reads as PTHREAD_PRIO_PROTECT and
# glibc aborts in `__pthread_tpp_change_priority`.
#
# Many rounds of many concurrent tickers keep a dense spawn/select/stop storm
# running under several worker Ms, which is what drives the race. It is
# inherently probabilistic, not deterministic: pre-fix this fixture aborted on
# the order of a third to a half of runs under AddressSanitizer at MAXPROCS=8
# (comparable to `time_ticker_many`), and never aborts post-fix. Needs
# MAXPROCS > 1 — set in the sibling `.env`; the output is exact, so any
# abort/mismatch fails the fixture, and a reintroduced regression is caught
# within a few runs.
import time

def waiter(channel[int64] done):
    auto tk = time.ticker(time.millis(10))
    auto t = <-tk.chan
    tk.stop()
    done <- 1

def main():
    auto rounds = 20
    auto n = 100
    auto r = 0
    while r < rounds:
        auto done = channel[int64](128)
        scope:
            auto i = 0
            while i < n:
                spawn waiter(done)
                i = i + 1
            auto got = 0
            while got < n:
                auto d = <-done
                got = got + 1
        r = r + 1
    print("ticker select stress:", rounds * n)

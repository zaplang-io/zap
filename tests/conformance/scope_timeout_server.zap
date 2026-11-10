# claim: SCOPE-9
# #825 server scenario: one "endpoint" (its own `scope(timeout=,grace=)`) whose
# child ignores cancellation times out and returns an error (504) — the orphan is
# severed and leaked — WHILE a concurrent healthy endpoint runs to completion
# (200). The process never exits: one faulty request can no longer take the whole
# server (and every concurrent healthy request) down. The two endpoints run
# concurrently in an outer `scope:`; the severed orphan is detached from the inner
# scope, so the outer scope does not wait on it. MAXPROCS=4.
def faulty_handler(channel[int64] ready):
    ready <- 1
    time.sleep_ns(time.millis(400))  # non-draining child

def faulty_endpoint() -> int64:
    auto ready = channel[int64](1)
    try:
        scope(timeout=100, grace=20):
            spawn faulty_handler(ready)
            auto a = <-ready
    except ScopeTimeout:
        return 504
    return 200

def healthy_endpoint() -> int64:
    scope:
        auto work = 2 + 2
    return 200

def run_faulty(channel[int64] out):
    out <- faulty_endpoint()

def run_healthy(channel[int64] out):
    out <- healthy_endpoint()

def main():
    auto faulty_out = channel[int64](1)
    auto healthy_out = channel[int64](1)
    scope:
        spawn run_faulty(faulty_out)
        spawn run_healthy(healthy_out)
    print("faulty:", <-faulty_out)
    print("healthy:", <-healthy_out)
    print("server up")

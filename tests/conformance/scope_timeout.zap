# claim: SCOPE-2
# Scope deadlines cancel blocked children through the shared cancellation path.
struct Guard:
    str name
    def __enter__(self) -> Guard:
        return self
    def __exit__(self):
        print(self.name, "with")

def receiver(channel[int64] ready, channel[int64] ch):
    try:
        with Guard("receiver"):
            ready <- 1
            auto value = <-ch
            print("wrong receive", value)
    finally:
        print("receiver finally")

def sender(channel[int64] ready, channel[int64] ch):
    try:
        with Guard("sender"):
            ready <- 1
            ch <- 1
            print("wrong send")
    finally:
        print("sender finally")

def fail():
    raise ValueError("child failure")

def main():
    scope(timeout=1000):
        auto done = 1
    print("finished before deadline")
    auto ready = channel[int64]()
    auto ch = channel[int64]()
    try:
        scope(timeout=1000, grace=5000):
            spawn receiver(ready, ch)
            auto started = <-ready
            gc.collect()
    except ScopeTimeout:
        print("receiver timed out")
    try:
        scope(timeout=1000, on_error=OnError.Wait, grace=5000):
            spawn sender(ready, ch)
            auto started = <-ready
            gc.collect()
            spawn fail()
    except ValueError as e:
        print("failure wins", e)
    try:
        scope(timeout=1000):
            scope:
                spawn receiver(ready, ch)
                auto started = <-ready
                gc.collect()
    except ScopeTimeout:
        print("nested timed out")
    try:
        scope(timeout=0):
            auto done = 1
    except ScopeTimeout:
        print("immediate deadline")
    try:
        scope(timeout=-1):
            auto done = 1
    except ValueError:
        print("invalid duration")

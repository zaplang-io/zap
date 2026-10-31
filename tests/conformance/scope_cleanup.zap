# claim: SCOPE-1
# SPEC §12 (I#475): eager argument capture, joins, failure and early-exit cleanup.
struct Guard:
    str label
    def __enter__(self) -> Guard:
        return self
    def __exit__(self):
        print(self.label)

def send(channel[int64] ch, int64 value):
    ch <- value

def wait_for_cancel(channel[int64] ready):
    try:
        with Guard("child with"):
            ready <- 1
            while True:
                checkpoint()
    finally:
        print("child finally")

def return_from_scope() -> int64:
    auto ready = channel[int64]()
    try:
        scope:
            spawn wait_for_cancel(ready)
            auto started = <-ready
            return 7
    finally:
        print("parent finally")
    return 0

def fail():
    with Guard("failure with"):
        try:
            raise ValueError("child failed")
        finally:
            print("failure finally")

def main():
    auto result = channel[int64](1)
    int64 value = 4
    scope(on_error=OnError.Wait):
        spawn send(result, value)
        value = 9
    print("captured", <-result, value)
    print("returned", return_from_scope())
    auto sibling_ready = channel[int64]()
    try:
        scope:
            spawn wait_for_cancel(sibling_ready)
            auto started = <-sibling_ready
            spawn fail()
    except ValueError as e:
        print("caught", e)
    auto ready = channel[int64]()
    while True:
        scope:
            spawn wait_for_cancel(ready)
            auto started = <-ready
            break
    print("after break")
    int64 count = 0
    while count < 1:
        scope:
            spawn wait_for_cancel(ready)
            auto started = <-ready
            count = count + 1
            continue
    print("after continue")

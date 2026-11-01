# claim: SCOPE-4
# ScopeTimeout reports EVERY coroutine still outstanding at the deadline, not a
# sample (reporting a subset would imply the others succeeded). Two children are
# blocked on a channel when the deadline fires; both must appear in the message.
def alpha(channel[int64] ready, channel[int64] ch):
    ready <- 1
    auto value = <-ch
    print("wrong receive", value)

def beta(channel[int64] ready, channel[int64] ch):
    ready <- 1
    auto value = <-ch
    print("wrong receive", value)

def main():
    auto ready = channel[int64]()
    auto ch = channel[int64]()
    try:
        scope(timeout=1000):
            spawn alpha(ready, ch)
            spawn beta(ready, ch)
            auto first = <-ready
            auto second = <-ready
    except ScopeTimeout as e:
        print(e)

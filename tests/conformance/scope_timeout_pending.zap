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
        # grace= lets the channel-parked children drain cooperatively (raising
        # Cancelled at `<-ch`, so neither reaches its post-receive `print`) rather
        # than being severed by the #825 immediate-sever backstop a bare timeout
        # now applies. The pending report is captured at the deadline regardless,
        # so the asserted message is unchanged.
        scope(timeout=1000, grace=5000):
            spawn alpha(ready, ch)
            spawn beta(ready, ch)
            auto first = <-ready
            auto second = <-ready
    except ScopeTimeout as e:
        print(e)

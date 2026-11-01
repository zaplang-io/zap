# claim: SCOPE-3
# Deadline grace bounds the drain of a child that never cooperates.
def uncooperative(channel[int64] ready):
    ready <- 1
    while True:
        auto spinning = 1

def main():
    auto ready = channel[int64]()
    scope(timeout=1000, grace=50):
        spawn uncooperative(ready)
        auto started = <-ready

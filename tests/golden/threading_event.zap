# Golden (I#175): cross-thread Event signaling. A worker thread blocks in
# e.wait() until main calls e.set_(); the join then guarantees the worker ran,
# so the shared cell reads 7 deterministically regardless of which side reaches
# the event first.
def waiter(Event e, list[int64] out):
    e.wait()
    out[0] = 7

def main() -> int64:
    auto e = threading.Event()
    list[int64] out = [0]
    auto t = threading.Thread(target=lambda: waiter(e, out))
    t.start()
    e.set_()
    t.join()
    print(out[0])
    return 0

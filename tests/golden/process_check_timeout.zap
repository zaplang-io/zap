# Golden fixture: process.check_run + timeout + spawn-failure negatives (#976,
# D-0237). check_run raises RuntimeError on a non-zero exit; a timeout SIGKILLs
# the child and raises TimeoutError; a missing binary raises on spawn.
def main():
    # check_run on a zero exit returns the tuple like run.
    auto c0, o0, e0 = process.check_run(["/bin/true"])
    print(c0)  # 0

    # check_run on a non-zero exit (/bin/false -> 1) raises RuntimeError.
    try:
        auto c1, o1, e1 = process.check_run(["/bin/false"])
        print("no-raise")
    except RuntimeError:
        print("check-raised")

    # timeout kills a long sleeper and raises TimeoutError.
    try:
        auto c2, o2, e2 = process.run(["/bin/sleep", "5"], timeout=0.2)
        print("no-timeout")
    except TimeoutError:
        print("timed-out")

    # a missing binary raises on spawn (catchable).
    try:
        auto c3, o3, e3 = process.run(["/nonexistent/zap-not-a-binary"])
        print("no-spawn-error")
    except Exception:
        print("spawn-failed")

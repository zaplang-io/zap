# Golden fixture: streaming process.Popen (#976, D-0237). Incremental stdin
# write + read-to-EOF, a payload larger than a pipe buffer (no deadlock), a
# separate stderr stream, poll()/wait()/returncode(), and terminate(). POSIX
# (streaming is not yet available on Windows); outputs are deterministic.
def main():
    # Echo round-trip: write to stdin, signal EOF, read all of stdout.
    auto p = process.Popen(["/bin/cat"])
    auto n = p.write("hello")
    print(n)  # 5
    p.close_stdin()
    auto out = p.read_stdout_all()
    print(out)  # hello
    print(p.wait())  # 0

    # 100_000 bytes of stdout — far past one pipe buffer. The netpoller-parked
    # reader drains it without deadlocking the writer.
    auto big = process.Popen(["/bin/sh", "-c", "yes x | head -c 100000"])
    auto blob = big.read_stdout_all()
    print(len(blob))  # 100000
    print(big.wait())  # 0

    # Separate stderr stream (printf -> no trailing newline).
    auto es = process.Popen(["/bin/sh", "-c", "printf E 1>&2"])
    auto errout = es.read_stderr_all()
    print(errout)  # E
    print(es.wait())  # 0

    # poll() before exit is the running sentinel; returncode after wait is 0.
    auto sl = process.Popen(["/bin/sleep", "1"])
    print(sl.poll())  # -256
    print(sl.wait())  # 0
    print(sl.returncode())  # 0

    # terminate() a long sleeper: killed by SIGTERM -> 128 + 15.
    auto doomed = process.Popen(["/bin/sleep", "30"])
    doomed.terminate()
    print(doomed.wait())  # 143

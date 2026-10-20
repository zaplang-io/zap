# Golden fixture: io — fd-based open/write/read/close (ticket #97). `close` is a
# reserved keyword (channel close), so the fd close is `io.close_fd`.
def main():
    auto d = fs.mkdtemp()
    auto f = os.path_join(d, "io.txt")
    auto w = io.open(f, "w")
    print(io.write(w, "abcdef"))
    io.close_fd(w)
    auto r = io.open(f, "r")
    print(io.read(r, 3))
    print(io.read(r, 100))
    io.close_fd(r)
    os.remove(f)
    os.rmdir(d)

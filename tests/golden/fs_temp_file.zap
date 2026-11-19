# Golden fixture: fs.temp_file (ticket #1077, D-0242) — the named temporary file
# (Python tempfile.NamedTemporaryFile) built on mkstemp (O_EXCL create) + the io
# buffered Writer (#1071). Exercises the write/flush/close_ surface, the public
# on-disk `name`, delete-on-close (the default) vs delete=False persistence, the
# `with` auto-close + auto-delete form, idempotent close, prefix/suffix/dir, and
# the catchable create-error path. Prints only deterministic results (bools,
# counts, contents) — never the random temp path. Cleans up after itself.
import io
import os
import fs


def main():
    # delete=True (the default): the file exists while open and is unlinked by
    # close_. `name` is the public on-disk path.
    auto t = fs.temp_file()
    print(len(t.name) > 0)  # True
    print(fs.exists(t.name))  # True
    auto saved = t.name
    print(t.write("hello"))  # 5
    t.flush()
    t.close_()
    print(fs.exists(saved))  # False

    # close_ is idempotent — a second close_ (writer already closed, file already
    # gone) is a harmless no-op, never a raise.
    t.close_()
    print("second close ok")  # second close ok

    # `with ... as` form: __exit__ closes and (delete=True) deletes on scope
    # exit. The path is captured before the block so the post-exit check can see
    # that the file is gone.
    auto wt = fs.temp_file()
    auto wname = wt.name
    with wt as w2:
        w2.write("a")
        w2.write("bc")
        print(fs.exists(wname))  # True
    print(fs.exists(wname))  # False

    # delete=False: the file persists after close_; its content reads back; the
    # caller owns cleanup.
    auto k = fs.temp_file(delete=False)
    k.write("keep-me")
    auto kname = k.name
    k.close_()
    print(fs.exists(kname))  # True
    print(fs.read_file(kname))  # keep-me
    os.remove(kname)

    # prefix / suffix are honored (checked on the basename, not the random run).
    auto s = fs.temp_file("build-", ".log", delete=False)
    auto base = os.path_basename(s.name)
    print(base.startswith("build-"))  # True
    print(base.endswith(".log"))  # True
    auto sname = s.name
    s.close_()
    os.remove(sname)

    # An explicit dir places the file there; buffered writes are flushed to disk
    # by close_, so the round-tripped content is the complete stream.
    auto d = fs.mkdtemp()
    auto f = fs.temp_file("x", "", d, delete=False)
    print(os.path_dirname(f.name) == d)  # True
    f.write("buffered-")
    f.write("content")
    auto fname = f.name
    f.close_()
    print(fs.read_file(fname))  # buffered-content
    fs.rmtree(d)

    # The create is a throwing call, so a bad dir raises a catchable error.
    try:
        auto bad = fs.temp_file("x", "", "/no/such/zap/dir/zzz1077")
        print("not reached")
    except Exception as e:
        print("temp_file caught")  # temp_file caught

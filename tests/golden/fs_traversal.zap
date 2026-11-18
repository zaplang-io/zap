# Golden fixture: directory traversal — fs.scandir / fs.walk / fs.copytree
# (#1032, D-0224). Builds a small tree in a fresh mkdtemp sandbox and exercises:
# scandir type flags on a mixed directory, a top-down walk (incl. an empty dir
# and a nested dir), and copytree into a new then an existing (merged) dst.
# Entries are returned SORTED, and the ephemeral temp path is never printed, so
# the output is deterministic. Cleans up with rmtree.
import os
import fs
import io


def main():
    auto root = fs.mkdtemp()
    auto sub = os.path_join(root, "sub")
    auto empty = os.path_join(root, "empty")
    fs.makedirs(sub)
    fs.makedirs(empty)
    fs.write_file(os.path_join(root, "a.txt"), "A")
    fs.write_file(os.path_join(root, "b.txt"), "B")
    fs.write_file(os.path_join(sub, "c.txt"), "C")

    # 1) scandir: one level, sorted, with type flags (no per-entry stat).
    print("scandir:")
    for e in fs.scandir(root):
        auto kind = "file"
        if e.is_dir:
            kind = "dir"
        print(e.name + " " + kind)

    # 2) walk: top-down, sorted. The dirpath holds the temp path, so print only
    # the deterministic counts and the sorted entry names.
    print("walk:")
    for (dirpath, dirs, files) in fs.walk(root):
        print("dirs=" + str(len(dirs)) + " files=" + str(len(files)))
        for d in dirs:
            print("  d:" + d)
        for f in files:
            print("  f:" + f)

    # 3) copytree into a NEW dst: copy root/sub -> root/copy, then list it.
    auto dst = os.path_join(root, "copy")
    fs.copytree(sub, dst)
    print("copytree-new:")
    for e in fs.scandir(dst):
        print(e.name)

    # 4) copytree into an EXISTING dst (merge): copy again; still one entry.
    fs.copytree(sub, dst)
    print("copytree-existing:")
    print(str(len(fs.scandir(dst))))

    fs.rmtree(root)

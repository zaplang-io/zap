# Golden fixture: fs — write/read/size + exists/is_file/is_dir predicates and
# os.remove/rmdir, over a unique temp dir from fs.mkdtemp() (ticket #97). Output
# is independent of the temp path, so it is deterministic across collectors/OSes.
def main():
    auto d = fs.mkdtemp()
    auto f = os.path_join(d, "hello.txt")
    fs.write_file(f, "hello world")
    print(fs.read_file(f))
    print(fs.size(f))
    if fs.exists(f):
        print("exists")
    if fs.is_file(f):
        print("is_file")
    if fs.is_dir(f):
        print("file_is_dir")
    else:
        print("file_not_dir")
    if fs.is_dir(d):
        print("dir_is_dir")
    os.remove(f)
    if fs.exists(f):
        print("still")
    else:
        print("gone")
    os.rmdir(d)

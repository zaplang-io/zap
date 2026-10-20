# Golden fixture: os.listdir over a temp dir (ticket #97). Entry order is
# unspecified, so the fixture asserts only the count.
def main():
    auto d = fs.mkdtemp()
    fs.write_file(os.path_join(d, "a"), "1")
    fs.write_file(os.path_join(d, "b"), "2")
    print(len(os.listdir(d)))
    os.remove(os.path_join(d, "a"))
    os.remove(os.path_join(d, "b"))
    os.rmdir(d)

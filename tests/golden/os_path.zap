# Golden fixture: os — path join/basename/dirname (flat names; #97).
def main():
    print(os.path_join("a", "b"))
    print(os.path_join("a/", "b"))
    print(os.path_join("a", "/abs"))
    print(os.path_basename("/foo/bar/baz.txt"))
    print(os.path_dirname("/foo/bar/baz.txt"))
    print(os.path_basename("plain"))
    print(os.path_dirname("plain"))

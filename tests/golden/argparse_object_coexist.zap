# Golden fixture: the flat `argparse.*` singleton (the #101 back-compat shim) and
# the new ArgumentParser object coexist in one program; an object parse does not
# disturb the singleton's state (#997).
def main():
    argparse.add_flag("verbose")
    argparse.add_option("name", "anon")
    argparse.parse(["prog", "--verbose", "--name", "flat"])
    print(argparse.get_flag("verbose"))
    print(argparse.get("name"))
    auto p = argparse.ArgumentParser(prog="obj")
    p.add_argument("--name", default="objdefault")
    auto ns = p.parse_args(["--name", "obj"])
    print(ns.get("name"))
    print(argparse.get("name"))

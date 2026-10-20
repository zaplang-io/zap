# Golden fixture: argparse — defaults when an option is omitted, plus the
# auto-generated usage/help text (ticket #101).
def main():
    argparse.set_prog("demo")
    argparse.set_description("A demo program.")
    argparse.add_flag("verbose")
    argparse.add_option("name", "anon")
    argparse.add_positional("src")
    auto argv = ["demo", "file.txt"]
    argparse.parse(argv)
    print(argparse.get("name"))
    if argparse.get_flag("verbose"):
        print("v")
    else:
        print("no-v")
    print(argparse.get("src"))
    print(argparse.format_help())

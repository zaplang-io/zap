# Golden fixture: argparse — a boolean flag, a string option, an int option and
# a positional, parsed from a literal argv (ticket #101). argv[0] ("prog") is
# the program name and is skipped, mirroring Python's parse_args(sys.argv[1:]).
def main():
    argparse.add_flag("verbose")
    argparse.add_option("name", "anon")
    argparse.add_int("count", 1)
    argparse.add_positional("src")
    auto argv = ["prog", "--verbose", "--name", "zap", "--count", "3", "input.txt"]
    argparse.parse(argv)
    if argparse.get_flag("verbose"):
        print("verbose=on")
    else:
        print("verbose=off")
    print(argparse.get("name"))
    print(argparse.get_int("count"))
    print(argparse.get("src"))

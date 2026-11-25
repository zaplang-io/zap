# Golden fixture: an object-binding (replacing) function decorator (#1509).
#
# A *dotted* decorator whose receiver method RETURNS a value binds the decorated
# name to that returned object, Click-style, instead of registering-and-
# discarding (the void case, see fn_decorator_register). The decision is
# return-type-driven (mechanism A, decisions/lang/1509-decorator-binds-object.md):
# `Cli.command(...) -> Command` returns a value, so `greet` becomes the Command.
#
#     @cli.command                       auto cli = Cli()
#     @cli.option("--count")             auto greet = cli.command(          # the
#     @cli.argument("greet")        ==>      cli.option("--count",          # name
#     def greet(str name) -> str:            cli.argument("greet",          # binds
#         ...                                    greet$impl)))               # to the
#     def main():                        greet.run()                       # object
#         greet.run()
#
# Stacked decorators compose bottom-up into one nested call: the decorator
# closest to `def` is innermost, and the `@option`/`@argument` arguments ride
# along as the library methods' arguments, so their metadata ("--count",
# "greet") survives for the command object to use. The original function is kept,
# renamed `greet$impl`, and handed to the chain as a typed `fn(str) -> str`
# callable (D-0067) — the command stores it and invokes it from `run()`.

struct Command:
    str label
    str opt
    fn(str) -> str handler

    def run(self):
        fn(str) -> str h = self.handler
        print("run " + self.label)
        print("opt " + self.opt)
        print(h("World"))

struct Cli:
    def argument(self, str label, fn(str) -> str handler) -> Command:
        return Command(label, "", handler)

    def option(self, str flag, Command c) -> Command:
        return Command(c.label, flag, c.handler)

    def command(self, Command c) -> Command:
        return Command("cmd:" + c.label, c.opt, c.handler)

@cli.command
@cli.option("--count")
@cli.argument("greet")
def greet(str name) -> str:
    return "Hello " + name

def main():
    auto cli = Cli()
    # The replacing decorators run here, after `cli` is bound: `greet` is now the
    # Command object, so `greet.run()` dispatches to Command.run.
    greet.run()

# #1527: a library-defined returning decorator accepts a concrete Zap type as a
# statically typed argument. `int64` and `str` are first-class type values in the
# decorator-argument position (they were `ZAP3001: unknown name 'int64'` before),
# flowing through the nested calls generated for stacked imported decorators,
# alongside the named `default=` metadata. The stack desugars to
#   greet = cli.command(cli.option("--count", int64,
#                                   cli.argument("name", str, greet$impl),
#                                   default=7))
# so `greet` is the returned `Command`: `greet.run()` dispatches through the
# chain and `greet.deflt` reads the metadata the type-carrying decorator kept.
import cli

@cli.command
@cli.option("--count", int64, default=7)
@cli.argument("name", str)
def greet(str flag) -> str:
    return "greet " + flag

def main():
    print(greet.run())
    print(greet.deflt)

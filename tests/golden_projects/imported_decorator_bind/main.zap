# #1519: a dotted decorator whose receiver is an IMPORTED package function binds
# the decorated name to the function's returned object (mechanism A across a
# package boundary, the cli#4 surface). `@cli.command` desugars to
#   auto greet = cli.command(cli.option("--count", cli.argument("World", greet$impl)))
# so `greet` is the returned `Command`: `greet.run()` dispatches through the
# chain, and the stacked `@option`/`@argument` metadata is retained on the
# command object. Run end-to-end to prove rebind + field reads + method dispatch.
import cli

@cli.command
@cli.option("--count")
@cli.argument("World")
def greet(str name) -> str:
    return "Hello " + name

def main():
    print(greet.run())
    print(greet.opt)
    print(greet.arg)

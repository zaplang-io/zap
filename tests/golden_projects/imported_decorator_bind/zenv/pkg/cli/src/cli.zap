# Dependency package `cli` for the #1519 imported-decorator golden. Its exported
# free functions are dotted-decorator receivers across a package boundary:
# `@cli.command` / `@cli.option(...)` / `@cli.argument(...)` each return a
# `Command`, so the decorated name is rebound to the returned object and the
# stacked decorators compose, preserving their typed metadata arguments.

struct Command:
    str opt
    str arg
    fn(str) -> str handler

    def run(self) -> str:
        auto handler = self.handler
        return handler(self.arg)

def argument(str name, fn(str) -> str handler) -> Command:
    return Command(opt="", arg=name, handler=handler)

def option(str flag, Command c) -> Command:
    return Command(opt=flag, arg=c.arg, handler=c.handler)

def command(Command c) -> Command:
    return Command(opt=c.opt, arg=c.arg, handler=c.handler)

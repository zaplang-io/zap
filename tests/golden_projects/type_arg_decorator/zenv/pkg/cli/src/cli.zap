# Dependency package `cli` for the #1527 type-argument golden. Its exported free
# functions are dotted-decorator receivers across a package boundary (the #1519
# surface) that additionally accept a concrete Zap *type* as an argument — the
# mechanism #1527 adds. `@cli.option("--count", int64, default=7)` passes the
# first-class type value `int64` and the named metadata `default=`; `@cli.argument
# ("name", str)` passes `str`. Each returns a `Command`, so the decorated name is
# rebound to the returned object (mechanism A, #1509) and the stacked decorators
# compose inner→outer, carrying their type and metadata arguments.

struct Command:
    str flag
    int64 deflt
    fn(str) -> str handler

    def run(self) -> str:
        auto handler = self.handler
        return handler(self.flag)

# The innermost decorator: takes the positional name, a type value `kind`, and
# the original handler the desugar appends.
def argument[T](str name, T kind, fn(str) -> str handler) -> Command:
    return Command(flag=name, deflt=0, handler=handler)

# A stacked decorator carrying a type value AND the named `default=` metadata.
# The desugar appends the composed inner `Command` as the trailing positional,
# so `c` binds to it while `default` binds by keyword.
def option[T](str flag, T kind, Command c, int64 default) -> Command:
    return Command(flag=flag, deflt=default, handler=c.handler)

def command(Command c) -> Command:
    return Command(flag=c.flag, deflt=c.deflt, handler=c.handler)

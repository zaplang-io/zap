# A generic type argument is inferred from an *omitted* parameter's typed
# default (#1704): the no-argument call returns the default value, while an
# explicit positional or keyword argument keeps its normal precedence and
# selects its own specialization. This pins the runtime value round-trip that
# the host-only `omitted_default_infer` lowering tests cannot observe.

const int64 LEVEL = 7


def setting[T](T value = False) -> T:
    return value


def pick[T](T value = -1) -> T:
    return value


def labelled[T](T value = LEVEL) -> T:
    return value


struct Holder:
    int seed

    def choose[T](self, T value = False) -> T:
        return value


def main():
    # Omitted: inferred from the `False` default.
    print(setting())
    # Explicit keyword and positional arguments win over the default.
    print(setting(value=True))
    print(setting(value="message"))
    print(setting(42))
    # A signed-literal default and a typed `const` default.
    print(pick())
    print(labelled())
    # Omitted default on a generic method.
    auto h = Holder(0)
    print(h.choose())
    print(h.choose(value="named"))

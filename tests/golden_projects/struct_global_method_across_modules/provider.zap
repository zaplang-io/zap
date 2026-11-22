# Regression for zaplang-io/zap#1258: a method call on a module-level struct (or
# enum) *constant* must keep working when the module is imported. Standalone this
# module always compiled; across an import the struct/enum *method* name was
# mangled with the module prefix whenever it collided with one of this module's
# own top-level symbols (here every method name deliberately shadows a free
# function of the same name), so the method was emitted under one name while the
# call site resolved it under another and lowering rejected the call (ZAP6001).
#
# Every public free function here reaches a method on a module global, so an
# importing module exercises the whole matrix through ordinary cross-module
# calls (see main.zap): a field read, a computed value, a fieldless struct, one
# global's method calling another global's method, two globals of the same
# struct type, and an enum global with a `self` method.

struct Provider:
    int64 id

    # Returns a field. Name collides with the free `result` below.
    def result(self) -> int64:
        return self.id


struct Counter:
    int64 base

    # Returns a computed value, not a bare field. Name collides with `value`.
    def value(self) -> int64:
        return self.base * 2


struct Empty:
    # A fieldless struct still dispatches a method on a global. Collides with `tag`.
    def tag(self) -> int64:
        return 7


struct Aggregator:
    int64 bump

    # A struct global's method that reads *another* module global and calls its
    # method. Collides with `total`.
    def total(self) -> int64:
        return _default.result() + _other.result() + self.bump


enum Level:
    LOW
    HIGH

    # An enum global with a `self` method. Collides with `weight`.
    def weight(self) -> int64:
        match self:
            case Level.LOW:
                return 1
            case Level.HIGH:
                return 10


const Provider _default = Provider(42)
# A second global of the *same* struct type, to prove multiple globals of one
# type each dispatch correctly.
const Provider _other = Provider(100)
const Counter _counter = Counter(10)
const Empty _empty = Empty()
const Aggregator _agg = Aggregator(5)
const Level _lvl = Level.HIGH


def result() -> int64:
    return _default.result()


def other() -> int64:
    return _other.result()


def value() -> int64:
    return _counter.value()


def tag() -> int64:
    return _empty.tag()


# 42 + 100 + 5 = 147
def total() -> int64:
    return _agg.total()


def weight() -> int64:
    return _lvl.weight()

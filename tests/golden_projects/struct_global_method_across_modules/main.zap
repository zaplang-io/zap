# The importing side of zaplang-io/zap#1258. Each call reaches a method on a
# module-level struct/enum constant declared in `provider`.
import provider


def main():
    # Field read on a struct const; the method name collides with a free fn.
    assert provider.result() == 42
    # A second global of the same struct type.
    assert provider.other() == 100
    # Computed value, not a bare field.
    assert provider.value() == 20
    # Fieldless struct const.
    assert provider.tag() == 7
    # A global whose method calls two other globals' methods: 42 + 100 + 5.
    assert provider.total() == 147
    # Enum global with a `self` method.
    assert provider.weight() == 10
    print(provider.result())
    print(provider.other())
    print(provider.value())
    print(provider.tag())
    print(provider.total())
    print(provider.weight())

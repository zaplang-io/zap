# Golden fixture: methods on a tier-1 (unboxed-integer) enum lower and dispatch
# directly (#731). Before, any method call on a tier-1 enum value raised
# `error[ZAP6001]: method call on a non-struct value` — tier-1 enum methods did
# not lower at all (only tier-3 did, via the tagged-struct path). The real method
# takes `self` by value as the backing integer; `match self` reads the member,
# and a method returning the enum type chains.

enum Dir:
    North
    East
    South
    West

    def turn_right(self) -> Dir:
        match self:
            case Dir.North:
                return Dir.East
            case Dir.East:
                return Dir.South
            case Dir.South:
                return Dir.West
            case _:
                return Dir.North

    def degrees(self) -> int:
        match self:
            case Dir.North:
                return 0
            case Dir.East:
                return 90
            case Dir.South:
                return 180
            case _:
                return 270

def main():
    print(Dir.North.degrees())  # 0
    print(Dir.East.degrees())  # 90
    print(Dir.West.degrees())  # 270
    # A method returning the enum, then chained.
    Dir d = Dir.North.turn_right()  # East
    print(d.degrees())  # 90
    print(d.turn_right().degrees())  # 180  (East -> South)
    print(Dir.West.turn_right().degrees())  # 0  (West -> North)

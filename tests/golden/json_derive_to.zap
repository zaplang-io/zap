# Golden fixture: @derive(Json) encode direction (D-0059, I#100). The derive
# generates a compile-time `to_json(self) -> str` producing the struct's compact
# JSON object. Covers scalar fields (int/float/bool/str) and a nested
# @derive(Json) struct. String values are JSON-escaped.

@derive(Json)
struct Point:
    int64 x
    int64 y

@derive(Json)
struct Player:
    str name
    int64 level
    float64 score
    bool active
    Point pos

def main():
    auto p = Point(x=3, y=-4)
    print(p.to_json())                      # {"x":3,"y":-4}

    auto pl = Player(name="A\"da", level=7, score=1.5, active=True, pos=Point(x=1, y=2))
    print(pl.to_json())                     # {"name":"A\"da","level":7,"score":1.5,"active":true,"pos":{"x":1,"y":2}}

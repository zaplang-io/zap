# Golden fixture: the default renderer routes every field through the one value
# renderer in repr mode (#274, D-0124), so a float renders %g-style, a bool
# renders True/False (not the old -1 bitpattern), a str renders QUOTED
# (Python repr: a string inside a struct/container is quoted), a tier-1 enum
# renders its qualified variant name, and a tuple field renders the Python
# (v0, v1) form with its own elements in repr mode.
enum Color:
    RED
    GREEN

struct Mixed:
    float64 f
    bool b
    str s
    Color c
    (int64, str) pair

def main():
    auto m = Mixed(f=1.5, b=True, s="hi", c=Color.GREEN, pair=(7, "z"))
    print(m)  # Mixed(f=1.5, b=True, s='hi', c=Color.GREEN, pair=(7, 'z'))
    print(f"{m}")

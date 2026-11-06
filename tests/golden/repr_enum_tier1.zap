# Golden fixture: a tier-1 enum value renders its qualified canonical variant
# name `EnumName.VARIANT` under the general Display protocol (#274, D-0124),
# consistently across print(), str(), and f-strings. Before #274, print(enum)
# emitted the raw discriminant integer and str()/f-string rejected it.
enum Color:
    RED
    GREEN = 10
    BLUE

def main():
    print(Color.RED)  # Color.RED
    print(Color.GREEN)  # Color.GREEN
    print(Color.BLUE)  # Color.BLUE
    print(str(Color.GREEN))  # Color.GREEN
    print(f"c={Color.BLUE}")  # c=Color.BLUE

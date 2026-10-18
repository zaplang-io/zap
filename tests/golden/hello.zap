# Golden fixture: strings — concatenation, str params/returns, print(str).
def greet(str name) -> str:
    return "Hello, " + name

def main():
    print(greet("Zap"))
    print("done")

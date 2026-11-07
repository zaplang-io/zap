# diff: match
# Regression (epic #374, FIXED B6): len(str) is the Unicode codepoint count
# (D-0025), matching CPython, not the UTF-8 byte length. Non-ASCII strings make
# the two differ. CPython prints: 5, 3, 10.
def main():
    print(len("héllo"))
    print(len("日本語"))
    print(len("naïve café"))

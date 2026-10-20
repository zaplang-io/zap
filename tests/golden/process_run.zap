# Golden fixture: process.run spawns a subprocess and captures (code, stdout,
# stderr) as a tuple (ticket #97).
def main():
    auto (code, out, err) = process.run(["/bin/echo", "hello", "world"])
    print(code)
    print(out)

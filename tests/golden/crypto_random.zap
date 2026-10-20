# Golden fixture: crypto secure random / secrets (ticket #105).
#
# The output is unpredictable (OS CSPRNG), so this asserts the deterministic
# shape: `random_bytes(n)` yields n bytes, `token_hex(n)` yields 2n hex chars,
# and n == 0 is well-defined (empty).
def main():
    print(len(crypto.random_bytes(16)))
    print(len(crypto.random_bytes(0)))
    print(len(crypto.token_hex(16)))
    print(len(crypto.token_hex(0)))

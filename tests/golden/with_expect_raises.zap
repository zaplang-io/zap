def boom():
    raise "kaboom"

def main():
    with expect_raises(ValueError):
        boom()
    print("raise was caught")

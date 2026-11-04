import conf

# A same-named const in the importer stays independent of conf.LIMIT.
const int64 LIMIT = 5

def main():
    print(conf.LIMIT)
    print(conf.cap())
    print(LIMIT)
    print(conf.LIMIT + LIMIT)

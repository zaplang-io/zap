import wkday

def main():
    # Imported constant called with an explicit argument.
    auto last = wkday.MO(-1)
    print(last.day)
    print(last.occurrence)
    # Imported constant called relying on its default argument.
    auto dflt = wkday.MO()
    print(dflt.occurrence)
    # Imported constant used as a plain value.
    print(wkday.MO.day)

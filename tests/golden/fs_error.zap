# Golden fixture: a fallible stdlib call raises a catchable Zap exception
# (ticket #97) — reading a missing file is caught by try/except.
def main():
    try:
        print(fs.read_file("/no/such/zap/file/zzz97"))
    except Exception as e:
        print("caught")

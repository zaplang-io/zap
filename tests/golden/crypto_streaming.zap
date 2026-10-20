# Golden fixture: crypto streaming hashing (ticket #105).
#
# A `_new` handle accepts incremental `update`s; `hexdigest`/`digest` finalize a
# copy, so a handle is non-destructive — it can be digested repeatedly and keeps
# accepting updates. Streaming "ab" then "c" must equal the one-shot of "abc",
# and after another "d" must equal sha256("abcd").
def main():
    auto h = crypto.sha256_new()
    crypto.update(h, "ab")
    crypto.update(h, "c")
    print(crypto.hexdigest(h))
    print(crypto.hexdigest(h))
    print(len(crypto.digest(h)))
    crypto.update(h, "d")
    print(crypto.hexdigest(h))

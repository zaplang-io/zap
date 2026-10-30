# Repro module (I#451, both axes): a generator method defined in an imported
# library module that YIELDS A STRUCT (`Page`). This is the exact shape the AWS
# SDK paginators emit (a `self`-bearing `yield`-method in a per-service module
# yielding a `Response` struct). Mirrors the single-file control shape so the
# only differences from the passing golden are (1) the import boundary and
# (2) the yielded struct.
struct Page:
    int64 start

struct Pager:
    int64 total
    int64 page_size

    def next_token(self, int64 t) -> int64:
        return t + self.page_size

    def pages(self, int64 token):
        auto t = token
        while t < self.total:
            yield Page(start=t)
            t = self.next_token(t)

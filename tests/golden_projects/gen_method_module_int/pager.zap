# Bisection module (I#451, axis = imported module): the control
# `generator_method_paginator.zap` generator method, moved verbatim into an
# imported library module. Yields a bare int64 (NOT a struct), so this fixture
# isolates the "generator method lives in an imported module" axis from the
# "yields a struct" axis.
struct Pager:
    int64 total
    int64 page_size

    def next_token(self, int64 t) -> int64:
        return t + self.page_size

    def pages(self, int64 token):
        auto t = token
        while t < self.total:
            yield t
            t = self.next_token(t)

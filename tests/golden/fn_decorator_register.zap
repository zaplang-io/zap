# Golden fixture: a registering function decorator (I#457, D-0087).
#
# `@app.get("/path")` is a *dotted* decorator name — a registering callable
# target. Instead of erroring ZAP5032 ("unknown decorator"), it desugars (before
# sema, so the result is fully type-checked) into the method-call registration
# the receiver already accepts:
#
#     @app.get("/hello")                 auto app = App()
#     def hello(str body) -> str:   ==>  app.get("/hello", hello)   # inserted
#         ...                            def hello(str body) -> str: ...
#
# which is exactly the surface fastzap v1 uses (`app.get(path, handler)`,
# zap#312). The registration is spliced in right after the receiver's binding,
# in route-declaration order, so `@app.get` is pure sugar over the existing
# engine. The handler is a top-level `def` passed as a first-class fn value
# (D-0067). Per-parameter typed injection is a separate capability (zap#458).

struct Route:
    str path
    fn(str) -> str handler

struct App:
    list[Route] routes = []

    def get(self, str path, fn(str) -> str handler):
        self.routes.append(Route(path=path, handler=handler))
        print("registered " + path)

    def dispatch(self, str path, str body) -> str:
        int64 i = 0
        while i < len(self.routes):
            auto r = self.routes[i]
            if r.path == path:
                fn(str) -> str h = r.handler
                return h(body)
            i = i + 1
        return "404"

@app.get("/hello")
def hello(str body) -> str:
    return "hello " + body

@app.get("/bye")
def bye(str body) -> str:
    return "bye " + body

def main():
    auto app = App()
    # The two `@app.get` registrations run here, right after `app` is bound.
    print(app.dispatch("/hello", "world"))
    print(app.dispatch("/bye", "now"))
    print(app.dispatch("/missing", "x"))

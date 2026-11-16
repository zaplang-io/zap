# Golden fixture: request-source parameter injection (I#959, D-0198) — the
# extension of the reflection-driven handler adapter (D-0092) that lets a
# framework declare request-derived parameter sources.
#
# A framework marks its extraction resolvers with `@request_source` (a core
# marker; the core recognizes the annotation, never the resolver names, so it
# stays framework-agnostic). A handler then declares a request-sourced parameter
# the FastAPI-canonical way — a default that calls such a resolver, e.g.
# `str id = Path()`, `int64 page = Num()`, `str sort = Query("asc")`. At the
# `@app.get` decoration site the adapter injects each by calling the resolver
# with `(request, "<param-name>", <marker args...>)`, drops the parameter from
# the `fn(Request) -> Response` seam, and tail-calls the handler — composing with
# `Depends(...)` in the same signature. The resolver gets the handler's
# pass-through Request; the parameter's declared type flows into the synthesized
# call so a non-`str` source (here `int64 page`) is typed end to end.
#
# `App.get` invokes the registered adapter with a constructed Request to prove
# the values are injected (not pulled off the request in the handler body):
#   * id   = Path(req, "id")        -> "abc"        (path param)
#   * page = Num(req, "page")       -> 7            (typed int64 source)
#   * sort = Query(req, "sort", …)  -> "desc"       (present query value)
#   * opt  = Query(req, "opt", …)   -> "fallback"   (absent -> forwarded default)
#   * db   = Depends(get_db)        -> DB("db42")   (dependency injection)

struct Request:
    str path
    dict[str, str] path_params
    dict[str, int64] nums
    dict[str, str] query

struct Response:
    str body

struct DB:
    str name

def get_db() -> DB:
    return DB("db42")

@request_source
def Path(Request req, str name) -> str:
    return req.path_params[name]

@request_source
def Num(Request req, str name) -> int64:
    return req.nums[name]

@request_source
def Query(Request req, str name, str fallback) -> str:
    if name in req.query:
        return req.query[name]
    return fallback

struct App:
    def get(self, str path, fn(Request) -> Response h):
        auto pp = {"id": "abc"}
        auto nm = {"page": 7}
        auto qq = {"sort": "desc"}
        Request req = Request(path, pp, nm, qq)
        fn(Request) -> Response f = h
        Response r = f(req)
        print(path + " -> " + r.body)

@app.get("/items/{id}")
def get_item(Request req, str id = Path(), int64 page = Num(), str sort = Query("asc"), str opt = Query("fallback"), DB db = Depends(get_db)) -> Response:
    return Response(id + "|" + str(page) + "|" + sort + "|" + opt + "|" + db.name)

def main():
    auto app = App()
    print("ready")

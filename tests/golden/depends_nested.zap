# Golden fixture: nested `Depends(...)` — a provider with its own dependency
# (I#466, D-0092).
#
# `get_db` itself declares `Settings s = Depends(get_settings)`, so resolving the
# handler's `db` must recursively resolve `get_settings` first (deps-first) and
# pass it in. The synthesized adapter is
#
#     def handler$adapter(Request req) -> Response:
#         Settings get_settings$dep = get_settings()
#         DB get_db$dep = get_db(get_settings$dep)
#         return handler(req, get_db$dep)
#
# `get_settings()` yields 7; `get_db` multiplies by 10 → the Response carries 70,
# proving the nested dependency resolved in the right order and its value flowed
# through the chain.

struct Request:
    str path

struct Response:
    int64 status

struct Settings:
    int64 n

struct DB:
    int64 conn

def get_settings() -> Settings:
    return Settings(7)

def get_db(Settings s = Depends(get_settings)) -> DB:
    return DB(s.n * 10)

struct App:
    def get(self, str path, fn(Request) -> Response h):
        Request req = Request(path)
        fn(Request) -> Response f = h
        Response r = f(req)
        print(path + " -> " + str(r.status))

@app.get("/x")
def handler(Request req, DB db = Depends(get_db)) -> Response:
    return Response(db.conn)

def main():
    auto app = App()
    print("ready")

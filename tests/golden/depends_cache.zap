# Golden fixture: per-request caching — a shared dependency resolves once
# (I#466, D-0092).
#
# Two handler parameters both `Depends(get_db)`. The adapter body is the
# per-request scope, so the provider is resolved into ONE local and reused:
#
#     def handler$adapter(Request req) -> Response:
#         DB get_db$dep = get_db()          # runs once
#         return handler(req, get_db$dep, get_db$dep)
#
# `get_db` prints "build db" each time it actually runs. If caching holds it
# prints exactly once per request; both parameters see the same DB, so
# `a.conn + b.conn == 2`. (FastAPI's default `use_cache=True`.)

struct Request:
    str path

struct Response:
    int64 status

struct DB:
    int64 conn

def get_db() -> DB:
    print("build db")
    return DB(1)

struct App:
    def get(self, str path, fn(Request) -> Response h):
        Request req = Request(path)
        fn(Request) -> Response f = h
        Response r = f(req)
        print(path + " -> " + str(r.status))

@app.get("/x")
def handler(Request req, DB a = Depends(get_db), DB b = Depends(get_db)) -> Response:
    return Response(a.conn + b.conn)

def main():
    auto app = App()
    print("ready")

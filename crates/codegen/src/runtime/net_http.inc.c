/* zap runtime fragment: net_http.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * net / http (ticket #104, D-0056). Plaintext TCP sockets + HTTP/1.1 client and
 * server, built on the netpoller (#80) and the M:N scheduler (#47): every
 * socket operation is non-blocking and PARKS the coroutine on a would-block
 * (freeing its worker M) via `zap_netpoll_wait`, rather than blocking an OS
 * thread or busy-polling. TLS/`https` is handled by the `tls` section below
 * (ticket #147); UDP is out of scope (plaintext TCP only). Keep-alive, chunked
 * transfer-encoding and graceful shutdown are follow-ups (noted in the PR /
 * design/0005).
 *
 * GC discipline (precise Immix): a socket op parks, and the collector traces a
 * parked G by its shadow chain only — NOT the green-stack C locals of this
 * runtime. So no GC pointer is held across a park: input payloads are copied
 * into `malloc` scratch up front, all I/O uses that scratch, and the GC result
 * objects (ZapStr, the handles below) are built only at the end with no park
 * after. This mirrors the "malloc scratch -> copy into a GC buffer" rule the
 * process (#97) and zlib (#106) runtime sections already follow.
 * ========================================================================== */
#include <errno.h>
#include <stddef.h> /* offsetof */
#if defined(ZAP_PLATFORM_POSIX)
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/types.h>
#else
/* Windows TCP sockets run on Winsock2 (<winsock2.h>/<ws2tcpip.h>, included at the
 * top under the platform seam). getaddrinfo/sockaddr_in/ntohs and the SOCKET type
 * come from there; the fd model is the netpoller's int64_t-carries-SOCKET
 * convention (A2 #196), so sockets created here are IOCP/AFD-pollable. */
#endif

#ifndef MSG_NOSIGNAL
#define MSG_NOSIGNAL 0 /* fallback: SIGPIPE possible on peer close, but compiles */
#endif

/* ---- socket compatibility seam (I#198) ------------------------------------ *
 * On POSIX a socket is an int fd and errors come from errno; on Windows it is a
 * SOCKET (pointer-width — so the handle type is zap_fd_t, carried into the
 * netpoller as the int64_t that A2's IOCP/AFD backend expects) and errors come
 * from WSAGetLastError with the WSAE* constants. These predicates keep the shared
 * accept/connect/recv/send loops below single-shape: on POSIX they reduce to the
 * exact errno checks the Linux build always used. `socket()`/`accept()` return
 * standard Winsock sockets, which SIO_BASE_HANDLE unwraps, so they are pollable
 * by the A2 netpoller with no change to the fd model. */
#if defined(ZAP_PLATFORM_WINDOWS)
typedef SOCKET zap_fd_t;
#define ZAP_INVALID_FD INVALID_SOCKET /* SOCKET is unsigned: socket() errors give
                                       * INVALID_SOCKET, never a negative value */
#define ZAP_FD_VALID(fd) ((fd) != INVALID_SOCKET)
#define ZAP_SOCK_ERR() WSAGetLastError()
#define ZAP_SOCK_EINTR(e) ((e) == WSAEINTR)
#define ZAP_SOCK_WOULDBLOCK(e) ((e) == WSAEWOULDBLOCK)
#define ZAP_SOCK_INPROGRESS(e) ((e) == WSAEWOULDBLOCK || (e) == WSAEINPROGRESS)
#define ZAP_SOCK_ALREADY(e) ((e) == WSAEALREADY || (e) == WSAEWOULDBLOCK)
#define ZAP_SOCK_ISCONN(e) ((e) == WSAEISCONN)
static void zap_sock_close_fd(zap_fd_t fd) { closesocket(fd); }
static int zap_sock_make_nonblocking(zap_fd_t fd) {
    u_long m = 1;
    return ioctlsocket(fd, FIONBIO, &m);
}
/* Winsock needs WSAStartup before any socket call; refcounted + idempotent. */
static void zap_net_startup_once(void) {
    WSADATA wsa;
    WSAStartup(MAKEWORD(2, 2), &wsa);
}
static pthread_once_t g_net_startup_once = PTHREAD_ONCE_INIT;
static void zap_net_startup(void) { pthread_once(&g_net_startup_once, zap_net_startup_once); }
#else
typedef int zap_fd_t;
#define ZAP_INVALID_FD (-1)
#define ZAP_FD_VALID(fd) ((fd) >= 0)
#define ZAP_SOCK_ERR() errno
#define ZAP_SOCK_EINTR(e) ((e) == EINTR)
#define ZAP_SOCK_WOULDBLOCK(e) ((e) == EAGAIN || (e) == EWOULDBLOCK)
#define ZAP_SOCK_INPROGRESS(e) ((e) == EINPROGRESS)
#define ZAP_SOCK_ALREADY(e) ((e) == EALREADY)
#define ZAP_SOCK_ISCONN(e) ((e) == EISCONN)
static void zap_sock_close_fd(zap_fd_t fd) { close(fd); }
static int zap_sock_make_nonblocking(zap_fd_t fd) {
    int fl = fcntl(fd, F_GETFL, 0);
    if (fl < 0) {
        return -1;
    }
    return fcntl(fd, F_SETFL, fl | O_NONBLOCK);
}
static void zap_net_startup(void) {}
#endif

/* First occurrence of `needle` (nlen bytes) in `hay` (hlen bytes), or NULL.
 * A dependency-free stand-in for memmem (no _GNU_SOURCE requirement). */
static const char *zap_memfind(const char *hay, size_t hlen, const char *needle, size_t nlen) {
    if (nlen == 0 || hlen < nlen) {
        return NULL;
    }
    for (size_t i = 0; i + nlen <= hlen; i++) {
        if (hay[i] == needle[0] && memcmp(hay + i, needle, nlen) == 0) {
            return hay + i;
        }
    }
    return NULL;
}

/* Raise a catchable Zap exception from a C message (NUL-terminated). */
static void zap_net_panic(const char *msg) {
    zap_panic(zap_str_new(msg, (int64_t)strlen(msg)));
}

/* NUL-terminated malloc copy of a ZapStr's bytes (host/port/URL parsing). The
 * caller frees. Returns NULL only on allocation failure. */
static char *zap_net_dupz(const ZapStr *s) {
    char *out = (char *)malloc((size_t)s->len + 1);
    if (!out) {
        return NULL;
    }
    if (s->len > 0) {
        memcpy(out, s->data, (size_t)s->len);
    }
    out[s->len] = '\0';
    return out;
}

/* ---- connection transport tag (I#556) ------------------------------------- *
 * `http.read_request` / `http.respond` accept EITHER a plaintext (`net`,
 * `ZapSock`) or a TLS (`tls.accept`, `ZapTls`) connection — at the Zap type level
 * both are the one opaque `bytes` handle, so the parser/emitter must tell them
 * apart at runtime. Both handles lead with this `int kind` tag (its only aliased
 * field, read through `ZapConnHdr` — the C common-initial-sequence rule makes
 * reading a shared leading `int` well defined even where later fields differ in
 * size, e.g. `fd` is `int` vs `SOCKET` across platforms). The server transport
 * seam dispatches on it; everything else casts to the concrete handle type. */
enum { ZAP_CONN_PLAIN = 0, ZAP_CONN_TLS = 1 };
typedef struct {
    int kind;
} ZapConnHdr;

/* Store a read deadline on either connection flavor (defined once `ZapTls` is in
 * scope). Lets `net.set_read_timeout`, declared above the TLS section, reach the
 * TLS handle's own `read_timeout_ms` field. */
static void zap_conn_set_read_timeout(void *c_, int64_t ms);
/* The whole-operation read-deadline twins (I#567): store / read the per-handle
 * total-request budget on either transport, dispatching on the same tag. */
static void zap_conn_set_read_deadline(void *c_, int64_t ms);
static int64_t zap_conn_read_deadline_ms(void *c_);

/* ---- socket handle -------------------------------------------------------- *
 * A listener or connection. `pd` is the netpoller descriptor (plain malloc
 * infra, never GC) or NULL when the netpoller is unavailable (the defensive
 * blocking fallback). The handle is ATOMIC — `fd` is a scalar (int on POSIX,
 * SOCKET on Windows) and `pd` is non-GC, so nothing to trace. */
typedef struct {
    int kind; /* ZAP_CONN_PLAIN; shared leading tag with ZapTls (I#556). */
    zap_fd_t fd;
    void *pd;
    /* Per-read deadline in milliseconds set via `net.set_read_timeout` (I#485);
     * 0 = no timeout (park forever, the default). Consulted by `recv` and by
     * `http.read_request`'s recv loop, which bound each wait-for-readiness. */
    int64_t read_timeout_ms;
    /* Whole-operation read budget in milliseconds set via `net.set_read_deadline`
     * (I#567); 0 = none (the default). Unlike `read_timeout_ms` (recomputed each
     * park), this is turned into ONE absolute deadline at the start of a read
     * (`recv`, or a whole `http.read_request`) so it bounds the entire operation
     * regardless of byte pacing — a slow-dribble peer can no longer keep resetting
     * the per-park clock forever. The two compose: each park waits the nearer of
     * the per-park timeout and this whole-op deadline. */
    int64_t read_deadline_ms;
    /* Address family of a UDP socket (AF_INET / AF_INET6), set by the datagram
     * constructors (I#991) so `udp_bind` / `udp_connect` / `sendto` resolve the
     * target constrained to the socket's family. AF_UNSPEC (0) for the TCP
     * stream sockets, which never consult it. Appended at the tail so the
     * aliased leading `kind` and the `read_timeout_ms` / `read_deadline_ms`
     * offsets the TLS seam reads are unchanged. */
    int family;
} ZapSock;

static const ZapTypeDesc zap_desc_sock = {(int64_t)sizeof(ZapSock), ZAP_TD_ATOMIC, 0, NULL};

/* Wrap an owned fd: make it non-blocking and register it with the netpoller.
 * Returns a GC handle. No park happens here, so building the GC object is safe. */
static ZapSock *zap_sock_wrap(zap_fd_t fd) {
    zap_sock_make_nonblocking(fd);
    void *pd = zap_netpoll_open((int64_t)fd); /* NULL => blocking-poll fallback */
    ZapSock *s = (ZapSock *)zap_obj(&zap_desc_sock);
    s->kind = ZAP_CONN_PLAIN; /* ATOMIC objects are not zeroed; init explicitly. */
    s->fd = fd;
    s->pd = pd;
    s->read_timeout_ms = 0;  /* ATOMIC objects are not zeroed; init explicitly. */
    s->read_deadline_ms = 0; /* ATOMIC objects are not zeroed; init explicitly. */
    s->family = AF_UNSPEC;   /* ATOMIC objects are not zeroed; init explicitly. */
    return s;
}

/* Park until `fd` is ready for `mode` (ZAP_POLL_READ / ZAP_POLL_WRITE), bounded by
 * BOTH a per-park relative `timeout_ms` (SO_RCVTIMEO-style: recomputed into a
 * fresh absolute deadline on each call — I#485) AND an absolute whole-operation
 * `whole_deadline_ns` (monotonic ns, computed ONCE at the start of a read, 0 =
 * none — I#567). The effective deadline is the NEARER of the two non-zero bounds;
 * either firing is a timeout (both raise the same TimeoutError, so they need not
 * be told apart). With the netpoller this frees the M; without it (pd == NULL)
 * fall back to a blocking poll() bracketed by enter/leave_blocking so the
 * collector treats us as stopped. Returns 0 on readiness, -1 if the descriptor
 * was closed under us, -2 on a timeout. Writers (connect/accept/send) pass 0/0
 * and only test `< 0`. */
static int zap_sock_park2(zap_fd_t fd, void *pd, int mode, int64_t timeout_ms,
                          int64_t whole_deadline_ns) {
    /* Per-park relative timeout -> a fresh absolute deadline; then take the nearer
     * of it and the whole-op deadline (0 = unbounded on either side). */
    int64_t eff = (timeout_ms > 0) ? zap_deadline_after_ms(timeout_ms) : 0;
    if (whole_deadline_ns > 0 && (eff == 0 || whole_deadline_ns < eff)) {
        eff = whole_deadline_ns;
    }
    if (pd) {
        int64_t r = zap_netpoll_wait(pd, mode, eff);
        if (r == ZAP_POLL_CANCELLED) return -3;
        if (r == ZAP_POLL_CLOSED) return -1;
        return (r == ZAP_POLL_TIMEOUT) ? -2 : 0;
    }
    /* Defensive no-netpoller fallback: bounded polling lets cancellation be
     * observed without abandoning a socket/worker. Recompute after EINTR. */
    for (;;) {
        int status = zap_io_status(NULL, eff);
        if (status) return status;
        int poll_ms = 10;
        if (eff) {
            int64_t remaining = eff - np_now_ns();
            int64_t ms = remaining > 0 ? 1 + (remaining - 1) / 1000000LL : 0;
            if (ms < poll_ms) poll_ms = (int)ms;
        }
#if defined(ZAP_PLATFORM_WINDOWS)
        WSAPOLLFD pfd;
        pfd.fd = fd;
        pfd.events = (SHORT)((mode == ZAP_POLL_READ) ? POLLRDNORM : POLLWRNORM);
        pfd.revents = 0;
        zap_enter_blocking();
        int n = WSAPoll(&pfd, 1, poll_ms);
        zap_leave_blocking();
#else
        struct pollfd pfd;
        pfd.fd = fd;
        pfd.events = (short)((mode == ZAP_POLL_READ) ? POLLIN : POLLOUT);
        pfd.revents = 0;
        zap_enter_blocking();
        int n = poll(&pfd, 1, poll_ms);
        int saved = errno;
        zap_leave_blocking();
        if (n < 0 && saved == EINTR) continue;
#endif
        if (n < 0) return -1;
        if (n > 0) return zap_io_status(NULL, eff);
    }
}

/* Park bounded only by the per-park timeout (no whole-op deadline). The common
 * case: connect/accept/send pass 0 (unbounded), and the per-park read paths that
 * do not layer a whole-op budget call through here unchanged. */
static int zap_sock_park(zap_fd_t fd, void *pd, int mode, int64_t timeout_ms) {
    return zap_sock_park2(fd, pd, mode, timeout_ms, 0);
}

/* Raise the catchable timeout exception (I#485): a socket read exceeded its
 * `net.set_read_timeout` deadline. Distinct type (TimeoutError) so it is told
 * apart from a clean EOF; thrown via the typed-panic/invoke path so it is caught
 * inside a `try` rather than aborting. */
static void zap_net_timeout_panic(void) {
    zap_panic_typed(ZAP_EXC_TIMEOUT_ERROR, zap_str_new("net: read timed out", 19));
}

static void zap_io_panic(int status, const char *message) {
    if (status == -3 || g_is_cancelled()) { zap_raise(cancelled_exc()); return; }
    if (status == -2) {
        const char *timeout = "net: I/O deadline exceeded";
        zap_panic_typed(ZAP_EXC_TIMEOUT_ERROR, zap_str_new(timeout, (int64_t)strlen(timeout)));
        return;
    }
    zap_net_panic(message);
}

/* ---- net.listen / connect / accept / recv / send / port / set_read_timeout /
 * close --------------------------------------------------------------------- */

/* Resolve host:port into an addrinfo list for a STREAM socket. `passive` adds
 * AI_PASSIVE (bind side). getaddrinfo can block on real DNS, so it is bracketed
 * by enter/leave_blocking; numeric hosts (127.0.0.1, ::1) resolve without I/O.
 * On failure raises a Zap exception. Caller freeaddrinfo()s the result. */
static struct addrinfo *zap_net_resolve_flags(const ZapStr *host, int64_t port, int passive, int numeric) {
    char *hz = zap_net_dupz(host);
    if (!hz) {
        zap_net_panic("net: out of memory");
        return NULL;
    }
    char portbuf[16];
    snprintf(portbuf, sizeof portbuf, "%ld", (long)port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    if (passive) hints.ai_flags |= AI_PASSIVE;
    if (numeric) hints.ai_flags |= AI_NUMERICHOST;
    const char *node = (host->len == 0 && passive) ? NULL : hz;
    struct addrinfo *res = NULL;
    zap_enter_blocking();
    int rc = getaddrinfo(node, portbuf, &hints, &res);
    zap_leave_blocking();
    free(hz);
    if (rc != 0) {
        char msg[256];
        snprintf(msg, sizeof msg, "net: resolve failed: %s", gai_strerror(rc));
        zap_net_panic(msg);
        return NULL;
    }
    if (g_is_cancelled()) {
        freeaddrinfo(res);
        zap_raise(cancelled_exc());
        return NULL;
    }
    return res;
}

static struct addrinfo *zap_net_resolve(const ZapStr *host, int64_t port, int passive) {
    return zap_net_resolve_flags(host, port, passive, 0);
}

/* net.listen(host, port) -> listener handle. Binds + listens; SO_REUSEADDR so a
 * server can rebind an ephemeral port promptly across test runs. Port 0 asks the
 * OS for an ephemeral port (read back with net.port). */
void *zap_net_listen(void *host_, int64_t port) {
    zap_net_startup();
    struct addrinfo *res = zap_net_resolve((const ZapStr *)host_, port, 1);
    zap_fd_t fd = ZAP_INVALID_FD;
    for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
        fd = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
        if (!ZAP_FD_VALID(fd)) {
            continue;
        }
        int one = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, (const char *)&one, sizeof one);
        if (bind(fd, ai->ai_addr, (socklen_t)ai->ai_addrlen) == 0 &&
            listen(fd, 128) == 0) {
            break;
        }
        zap_sock_close_fd(fd);
        fd = ZAP_INVALID_FD;
    }
    freeaddrinfo(res);
    if (!ZAP_FD_VALID(fd)) {
        zap_net_panic("net: listen failed");
        return NULL;
    }
    return zap_sock_wrap(fd);
}

/* net.connect(host, port) -> connection handle. Non-blocking connect: on
 * EINPROGRESS park for writability, then check SO_ERROR. Tries each resolved
 * address in turn. */
static void *zap_net_connect_impl(void *host_, int64_t port, int64_t deadline_ns, int numeric) {
    zap_check_cancel();
    if (deadline_ns < 0 || (numeric && (port < 1 || port > 65535))) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("net: invalid connect deadline or port", sizeof("net: invalid connect deadline or port") - 1));
        return NULL;
    }
    if (numeric) {
        const ZapStr *host = host_;
        char address[INET6_ADDRSTRLEN];
        unsigned char binary[16];
        if (host->len <= 0 || host->len >= (int64_t)sizeof(address)) {
            zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("net: bounded connect requires a numeric IP", sizeof("net: bounded connect requires a numeric IP") - 1));
            return NULL;
        }
        memcpy(address, host->data, (size_t)host->len);
        address[host->len] = '\0';
        if (memchr(address, '\0', (size_t)host->len) ||
            (inet_pton(AF_INET, address, binary) != 1 && inet_pton(AF_INET6, address, binary) != 1)) {
            zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("net: bounded connect requires a numeric IP", sizeof("net: bounded connect requires a numeric IP") - 1));
            return NULL;
        }
    }
    int outcome = zap_io_status(NULL, deadline_ns);
    if (outcome) { zap_io_panic(outcome, "net: connect failed"); return NULL; }
    zap_net_startup();
    struct addrinfo *res = zap_net_resolve_flags((const ZapStr *)host_, port, 0, numeric);
    zap_fd_t fd = ZAP_INVALID_FD;
    for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
        outcome = zap_io_status(NULL, deadline_ns);
        if (outcome) break;
        fd = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
        if (!ZAP_FD_VALID(fd)) continue;
        zap_sock_make_nonblocking(fd);
        void *pd = zap_netpoll_open((int64_t)fd);
        int ok = 0;
        for (;;) {
            outcome = zap_io_status(pd, deadline_ns);
            if (outcome) break;
            int rc = connect(fd, ai->ai_addr, (socklen_t)ai->ai_addrlen);
            if (rc == 0) { ok = 1; break; }
            int e = ZAP_SOCK_ERR();
            if (ZAP_SOCK_EINTR(e)) continue;
            if (ZAP_SOCK_INPROGRESS(e) || ZAP_SOCK_ALREADY(e)) {
                outcome = zap_sock_park2(fd, pd, ZAP_POLL_WRITE, 0, deadline_ns);
                if (outcome < 0) break;
                int soerr = 0;
                socklen_t sl = sizeof soerr;
                if (getsockopt(fd, SOL_SOCKET, SO_ERROR, (char *)&soerr, &sl) < 0 || soerr != 0) break;
                /* A stale/early writable latch is not proof of connection.
                 * SO_ERROR may still be zero while SYN is pending. */
                struct sockaddr_storage peer;
                socklen_t peer_len = sizeof peer;
                if (getpeername(fd, (struct sockaddr *)&peer, &peer_len) != 0) continue;
                ok = 1; break;
            }
            if (ZAP_SOCK_ISCONN(e)) ok = 1;
            break;
        }
        if (ok) {
            outcome = zap_io_status(pd, deadline_ns);
            if (outcome) ok = 0;
        }
        if (ok) {
            freeaddrinfo(res);
            ZapSock *s = (ZapSock *)zap_obj(&zap_desc_sock);
            s->kind = ZAP_CONN_PLAIN; s->fd = fd; s->pd = pd;
            s->read_timeout_ms = 0; s->read_deadline_ms = 0;
            s->family = AF_UNSPEC;
            return s;
        }
        if (pd) zap_netpoll_close(pd);
        zap_sock_close_fd(fd);
        fd = ZAP_INVALID_FD;
        if (outcome == -2 || outcome == -3) break;
    }
    freeaddrinfo(res);
    zap_io_panic(outcome, "net: connect failed");
    return NULL;
}

void *zap_net_connect(void *host_, int64_t port) {
    return zap_net_connect_impl(host_, port, 0, 0);
}

void *zap_net_connect_ip_deadline(void *ip_, int64_t port, int64_t deadline_ns) {
    return zap_net_connect_impl(ip_, port, deadline_ns, 1);
}

/* net.accept(listener) -> connection handle. Parks on readability until a peer
 * connects. */
void *zap_net_accept(void *l_) {
    ZapSock *l = (ZapSock *)l_;
    zap_fd_t lfd = l->fd;
    void *lpd = l->pd;
    for (;;) {
        zap_check_cancel();
        zap_fd_t cfd = accept(lfd, NULL, NULL);
        if (ZAP_FD_VALID(cfd)) {
            return zap_sock_wrap(cfd);
        }
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            int status = zap_sock_park(lfd, lpd, ZAP_POLL_READ, 0);
            if (status < 0) {
                zap_io_panic(status, "net: accept on a closed listener");
                return NULL;
            }
            continue;
        }
        zap_net_panic("net: accept failed");
        return NULL;
    }
}

/* net.recv(conn, n) -> str. Reads up to `n` bytes (one readiness-gated read);
 * returns an empty str on EOF. The scratch buffer is malloc (lives across the
 * park); the result is copied into a GC str at the end. */
void *zap_net_recv(void *c_, int64_t n) {
    ZapSock *c = (ZapSock *)c_;
    zap_fd_t fd = c->fd;
    void *pd = c->pd;
    int64_t timeout_ms = c->read_timeout_ms;
    /* Whole-operation read deadline (I#567): one absolute deadline for this recv,
     * composed with the per-park timeout at each park. 0 = none (unchanged). */
    int64_t whole_deadline_ns =
        (c->read_deadline_ms > 0) ? zap_deadline_after_ms(c->read_deadline_ms) : 0;
    if (n < 0) {
        n = 0;
    }
    int initial = zap_io_status(pd, whole_deadline_ns);
    if (initial) { zap_io_panic(initial, "net: recv failed"); return NULL; }
    char *buf = (char *)malloc((size_t)n + 1);
    if (!buf) {
        zap_net_panic("net: out of memory");
        return NULL;
    }
    ssize_t got = 0;
    for (;;) {
        int status = zap_io_status(pd, whole_deadline_ns);
        if (status) { free(buf); zap_io_panic(status, "net: recv failed"); return NULL; }
        ssize_t r = (ssize_t)recv(fd, buf, (size_t)n, 0);
        if (r >= 0) {
            got = r;
            break;
        }
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            int p = zap_sock_park2(fd, pd, ZAP_POLL_READ, timeout_ms, whole_deadline_ns);
            if (p == -2 || p == -3) {
                free(buf);
                zap_io_panic(p, "net: recv failed");
                return NULL;
            }
            if (p < 0) {
                got = 0;
                break;
            }
            continue;
        }
        free(buf);
        zap_io_panic(-1, "net: recv failed");
        return NULL;
    }
    void *out = zap_str_new(buf, (int64_t)got);
    free(buf);
    zap_check_cancel();
    return out;
}

/* Write all `len` bytes of `data` to `fd`, parking on EAGAIN. MSG_NOSIGNAL
 * suppresses SIGPIPE on a peer close (the #77 signal subsystem is left alone).
 * `data` is malloc scratch, so it survives the parks. Returns bytes written, or
 * -1 on a hard error. */
static ssize_t zap_sock_send_all(zap_fd_t fd, void *pd, const char *data, size_t len) {
    size_t off = 0;
    int initial = zap_io_status(pd, 0);
    if (initial) return initial;
    while (off < len) {
        int status = zap_io_status(pd, 0);
        if (status) return status;
        ssize_t w = (ssize_t)send(fd, data + off, len - off, MSG_NOSIGNAL);
        if (w > 0) {
            off += (size_t)w;
            continue;
        }
        if (w == 0) return -1;
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            int status = zap_sock_park(fd, pd, ZAP_POLL_WRITE, 0);
            if (status < 0) return status;
            continue;
        }
        return -1;
    }
    return (ssize_t)off;
}

/* net.send(conn, data) -> int64 bytes written. Copies the payload to malloc
 * scratch first so no GC pointer is held across the write parks. */
int64_t zap_net_send(void *c_, void *data_) {
    ZapSock *c = (ZapSock *)c_;
    zap_fd_t fd = c->fd;
    void *pd = c->pd;
    int status = zap_io_status(pd, 0);
    if (status) { zap_io_panic(status, "net: send failed"); return 0; }
    const ZapStr *data = (const ZapStr *)data_;
    size_t len = (size_t)(data->len < 0 ? 0 : data->len);
    char *scratch = (char *)malloc(len + 1);
    if (!scratch) {
        zap_net_panic("net: out of memory");
        return 0;
    }
    if (len > 0) {
        memcpy(scratch, data->data, len);
    }
    ssize_t w = zap_sock_send_all(fd, pd, scratch, len);
    free(scratch);
    if (w < 0) {
        zap_io_panic((int)w, "net: send failed");
        return 0;
    }
    return (int64_t)w;
}

/* net.port(listener) -> int64. The locally bound port (host byte order), so an
 * ephemeral `net.listen(host, 0)` can be discovered and dialed. */
int64_t zap_net_port(void *s_) {
    ZapSock *s = (ZapSock *)s_;
    struct sockaddr_storage ss;
    socklen_t sl = sizeof ss;
    if (getsockname(s->fd, (struct sockaddr *)&ss, &sl) != 0) {
        return 0;
    }
    if (ss.ss_family == AF_INET6) {
        return (int64_t)ntohs(((struct sockaddr_in6 *)&ss)->sin6_port);
    }
    return (int64_t)ntohs(((struct sockaddr_in *)&ss)->sin_port);
}

/* Close a raw fd + netpoller descriptor. Used both by net.close and by the HTTP
 * client, which holds only fd+pd (never the GC handle) across its I/O parks. */
static void zap_sock_shutdown(zap_fd_t fd, void *pd) {
    if (pd) {
        zap_netpoll_close(pd);
    }
    if (ZAP_FD_VALID(fd)) {
        zap_sock_close_fd(fd);
    }
}

/* net.close_conn / net.close_listener. Deregisters from the netpoller (waking
 * any parked waiter with CLOSED) and closes the fd. Idempotent-ish: a second
 * close is a harmless EBADF. */
void zap_net_close(void *s_) {
    ZapSock *s = (ZapSock *)s_;
    if (s->fd < 0) {
        return;
    }
    zap_sock_shutdown(s->fd, s->pd);
    s->pd = NULL;
    s->fd = -1;
}

/* net.set_read_timeout(conn, ms) (I#485). Store a per-read deadline on the socket
 * handle: `recv` and `http.read_request` bound each wait-for-readiness by it and
 * raise a catchable TimeoutError when it elapses. `ms == 0` restores the default
 * (no timeout, park forever). A negative `ms` is a ValueError (mirrors
 * `scope(timeout=...)`). Setting it on a listener is harmless (only reads consult
 * it). */
void zap_net_set_read_timeout(void *s_, int64_t ms) {
    if (ms < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("net: read timeout must be non-negative", 39));
        return;
    }
    /* Dispatch on the transport tag so it also works on a TLS connection (I#556):
     * `http.read_request` over TLS honors the same deadline because the TLS fill
     * path reads `ZapTls.read_timeout_ms`, which lives at the handle's own offset. */
    zap_conn_set_read_timeout(s_, ms);
}

/* net.set_read_deadline(conn, ms) (I#567). Store a WHOLE-OPERATION read budget on
 * the connection handle: a `recv` or a whole `http.read_request` (headers+body)
 * that takes longer than `ms` in total raises a catchable TimeoutError, no matter
 * how the bytes are paced — so a slow-dribble (slowloris) peer that keeps resetting
 * the per-park `set_read_timeout` clock is still bounded. `ms == 0` restores the
 * default (no whole-op deadline). A negative `ms` is a ValueError (mirrors
 * `set_read_timeout`). The two compose: with both set, each park waits the nearer
 * of the per-park timeout and the whole-op deadline. Dispatches on the transport
 * tag so it applies over plaintext AND TLS (through the I#556 server seam). */
void zap_net_set_read_deadline(void *s_, int64_t ms) {
    if (ms < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("net: read deadline must be non-negative", 39));
        return;
    }
    zap_conn_set_read_deadline(s_, ms);
}

/* ===========================================================================
 * net: UDP datagram sockets (ticket #991, epic #985). A connectionless
 * SOCK_DGRAM socket on the same netpoller as the TCP side: every send/recv is
 * non-blocking and PARKS on a would-block (freeing the worker M), identical in
 * shape to the stream paths above. The handle is the same `ZapSock` (so
 * `net.port`, `net.set_read_timeout`, `net.set_read_deadline`,
 * `net.set_deadline_ns` and `net.close_conn` all apply unchanged); its `family`
 * field records AF_INET / AF_INET6 so `udp_bind` / `udp_connect` / `sendto`
 * resolve a target constrained to the socket's family (no accidental v4/v6
 * cross-wiring — a mismatch raises, exactly as Python's `socket` does).
 *
 * GC discipline (precise Immix), identical to the stream section: a payload is
 * copied into malloc scratch BEFORE any park, all I/O uses that scratch, and the
 * GC result objects (the ZapStr payload, the ZapDatagram below) are built only
 * at the very end with no park after.
 * ========================================================================== */

/* The result of net.recvfrom: one datagram's payload plus the numeric sender
 * address. A precise GC object (two leading pointer fields traced; the trailing
 * `port` scalar is not) whose three fields the lowerer reads back into a
 * `(str, str, int64)` tuple — the same aggregate path `process.run` uses for its
 * `ZapProc` -> `(int64, str, str)`. */
typedef struct {
    ZapStr *data;
    ZapStr *host;
    int64_t port;
} ZapDatagram;

static const int32_t zap_off_dgram[] = {
    (int32_t)offsetof(ZapDatagram, data),
    (int32_t)offsetof(ZapDatagram, host),
};
static const ZapTypeDesc zap_desc_dgram = {(int64_t)sizeof(ZapDatagram), ZAP_TD_PRECISE,
                                           2, zap_off_dgram};

/* Resolve host:port into an addrinfo list for a DATAGRAM socket, constrained to
 * `family` (AF_INET / AF_INET6; AF_UNSPEC lets getaddrinfo pick). `passive` adds
 * AI_PASSIVE (the bind side; an empty host then means the wildcard address).
 * getaddrinfo can block on real DNS, so it is bracketed by enter/leave_blocking;
 * numeric hosts resolve without I/O. Raises a Zap exception on failure. Caller
 * freeaddrinfo()s the result. */
static struct addrinfo *zap_udp_resolve(const ZapStr *host, int64_t port, int family,
                                        int passive) {
    char *hz = zap_net_dupz(host);
    if (!hz) {
        zap_net_panic("net: out of memory");
        return NULL;
    }
    char portbuf[16];
    snprintf(portbuf, sizeof portbuf, "%ld", (long)port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = family;
    hints.ai_socktype = SOCK_DGRAM;
    hints.ai_protocol = IPPROTO_UDP;
    if (passive) hints.ai_flags |= AI_PASSIVE;
    const char *node = (host->len == 0 && passive) ? NULL : hz;
    struct addrinfo *res = NULL;
    zap_enter_blocking();
    int rc = getaddrinfo(node, portbuf, &hints, &res);
    zap_leave_blocking();
    free(hz);
    if (rc != 0) {
        char msg[256];
        snprintf(msg, sizeof msg, "net: resolve failed: %s", gai_strerror(rc));
        zap_net_panic(msg);
        return NULL;
    }
    if (g_is_cancelled()) {
        freeaddrinfo(res);
        zap_raise(cancelled_exc());
        return NULL;
    }
    return res;
}

/* Non-raising DGRAM resolve used by net.sendto, which has already copied its
 * payload into malloc scratch before this point: it must be able to FREE that
 * scratch before raising, so this variant reports failure via `*rc_out` /
 * `*cancelled` instead of longjmping. `*rc_out` is a getaddrinfo code (0 ok),
 * `*cancelled` is set when a scope cancellation was observed. Returns the
 * addrinfo list (caller freeaddrinfo()s) or NULL. */
static struct addrinfo *zap_udp_getaddr(const ZapStr *host, int64_t port, int family,
                                        int *rc_out, int *cancelled) {
    *rc_out = 0;
    *cancelled = 0;
    char *hz = zap_net_dupz(host);
    if (!hz) {
        *rc_out = EAI_MEMORY;
        return NULL;
    }
    char portbuf[16];
    snprintf(portbuf, sizeof portbuf, "%ld", (long)port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = family;
    hints.ai_socktype = SOCK_DGRAM;
    hints.ai_protocol = IPPROTO_UDP;
    struct addrinfo *res = NULL;
    zap_enter_blocking();
    int rc = getaddrinfo(hz, portbuf, &hints, &res);
    zap_leave_blocking();
    free(hz);
    if (rc != 0) {
        *rc_out = rc;
        return NULL;
    }
    if (g_is_cancelled()) {
        freeaddrinfo(res);
        *cancelled = 1;
        return NULL;
    }
    return res;
}

/* Create an unbound UDP socket of `family`, register it with the netpoller, and
 * return the GC handle. Shared by net.udp_socket (AF_INET) and net.udp_socket6
 * (AF_INET6). No park happens here, so building the GC object is safe. */
static void *zap_udp_socket_family(int family, const char *what) {
    zap_net_startup();
    zap_fd_t fd = socket(family, SOCK_DGRAM, IPPROTO_UDP);
    if (!ZAP_FD_VALID(fd)) {
        zap_net_panic(what);
        return NULL;
    }
    ZapSock *s = zap_sock_wrap(fd);
    s->family = family;
    return s;
}

/* net.udp_socket() -> sock. An unbound IPv4 (AF_INET) datagram socket. */
void *zap_udp_socket(void) {
    return zap_udp_socket_family(AF_INET, "net: udp_socket failed");
}

/* net.udp_socket6() -> sock. An unbound IPv6 (AF_INET6) datagram socket. */
void *zap_udp_socket6(void) {
    return zap_udp_socket_family(AF_INET6, "net: udp_socket6 failed");
}

/* net.udp_bind(sock, host, port). Bind a datagram socket to a local address so
 * it can receive. Port 0 asks the OS for an ephemeral port (read back with
 * net.port). An empty host is the wildcard address. SO_REUSEADDR so a rebind on
 * an ephemeral port is prompt across test runs, mirroring net.listen. Resolves
 * constrained to the socket's family, so binding an IPv6 literal on an IPv4
 * socket (or vice versa) raises rather than silently misbehaving. */
void zap_udp_bind(void *sock_, void *host_, int64_t port) {
    ZapSock *s = (ZapSock *)sock_;
    struct addrinfo *res = zap_udp_resolve((const ZapStr *)host_, port, s->family, 1);
    int bound = 0;
    for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
        int one = 1;
        setsockopt(s->fd, SOL_SOCKET, SO_REUSEADDR, (const char *)&one, sizeof one);
        if (bind(s->fd, ai->ai_addr, (socklen_t)ai->ai_addrlen) == 0) {
            bound = 1;
            break;
        }
    }
    freeaddrinfo(res);
    if (!bound) {
        zap_net_panic("net: udp_bind failed");
    }
}

/* net.udp_connect(sock, host, port). Set the datagram socket's default peer so
 * net.send / net.recv (and the shared deadlines) work on it — Python's
 * `s.connect` for a SOCK_DGRAM. A UDP connect only records the peer (no
 * handshake, no network round-trip), so it does not park. Resolves constrained
 * to the socket's family. */
void zap_udp_connect(void *sock_, void *host_, int64_t port) {
    ZapSock *s = (ZapSock *)sock_;
    struct addrinfo *res = zap_udp_resolve((const ZapStr *)host_, port, s->family, 0);
    int ok = 0;
    for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
        if (connect(s->fd, ai->ai_addr, (socklen_t)ai->ai_addrlen) == 0) {
            ok = 1;
            break;
        }
    }
    freeaddrinfo(res);
    if (!ok) {
        zap_net_panic("net: udp_connect failed");
    }
}

/* net.sendto(sock, data, host, port) -> int64 bytes sent. Send ONE datagram to
 * host:port. The payload is copied to malloc scratch up front so no GC pointer
 * is held across the resolve/send parks. Resolves constrained to the socket's
 * family; parks for writability on a would-block. A datagram is sent whole, so a
 * short sendto is a hard error, not a partial write to retry. */
int64_t zap_udp_sendto(void *sock_, void *data_, void *host_, int64_t port) {
    ZapSock *s = (ZapSock *)sock_;
    zap_fd_t fd = s->fd;
    void *pd = s->pd;
    const ZapStr *data = (const ZapStr *)data_;
    size_t len = (size_t)(data->len < 0 ? 0 : data->len);
    char *scratch = (char *)malloc(len + 1);
    if (!scratch) {
        zap_net_panic("net: out of memory");
        return 0;
    }
    if (len > 0) {
        memcpy(scratch, data->data, len);
    }
    /* Resolve after the payload is safely in malloc scratch (no GC pointer held
     * across getaddrinfo's blocking window). The non-raising variant lets us free
     * scratch before raising on a resolve failure. */
    int rc = 0, cancelled = 0;
    struct addrinfo *res = zap_udp_getaddr((const ZapStr *)host_, port, s->family, &rc, &cancelled);
    if (!res) {
        free(scratch);
        if (cancelled) {
            zap_raise(cancelled_exc());
            return 0;
        }
        char msg[256];
        snprintf(msg, sizeof msg, "net: resolve failed: %s", gai_strerror(rc));
        zap_net_panic(msg);
        return 0;
    }
    int initial = zap_io_status(pd, 0);
    if (initial) {
        free(scratch);
        freeaddrinfo(res);
        zap_io_panic(initial, "net: sendto failed");
        return 0;
    }
    ssize_t sent = -1;
    int hard_err = 0;
    for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
        for (;;) {
            int status = zap_io_status(pd, 0);
            if (status) {
                free(scratch);
                freeaddrinfo(res);
                zap_io_panic(status, "net: sendto failed");
                return 0;
            }
            ssize_t w = (ssize_t)sendto(fd, scratch, len, MSG_NOSIGNAL, ai->ai_addr,
                                        (socklen_t)ai->ai_addrlen);
            if (w >= 0) {
                sent = w;
                break;
            }
            int e = ZAP_SOCK_ERR();
            if (ZAP_SOCK_EINTR(e)) {
                continue;
            }
            if (ZAP_SOCK_WOULDBLOCK(e)) {
                int p = zap_sock_park(fd, pd, ZAP_POLL_WRITE, 0);
                if (p < 0) {
                    free(scratch);
                    freeaddrinfo(res);
                    zap_io_panic(p, "net: sendto failed");
                    return 0;
                }
                continue;
            }
            hard_err = 1; /* this address family/route failed; try the next ai */
            break;
        }
        if (sent >= 0) {
            break;
        }
    }
    free(scratch);
    freeaddrinfo(res);
    if (sent < 0) {
        zap_net_panic(hard_err ? "net: sendto failed" : "net: sendto: unresolved address");
        return 0;
    }
    return (int64_t)sent;
}

/* net.recvfrom(sock, n) -> (data, host, port). Receive ONE datagram (up to `n`
 * bytes; any excess is discarded, standard SOCK_DGRAM truncation) and report the
 * numeric sender address. Honors net.set_read_timeout / net.set_read_deadline
 * exactly as net.recv (same park path). A zero-length datagram is a valid empty
 * payload (not EOF); a socket closed under a parked recvfrom yields an empty
 * datagram with an empty host, mirroring net.recv's EOF. The scratch buffer is
 * malloc (lives across the park); the ZapStr payload, the numeric host and the
 * ZapDatagram are built only at the end with no park after. */
void *zap_udp_recvfrom(void *sock_, int64_t n) {
    ZapSock *s = (ZapSock *)sock_;
    zap_fd_t fd = s->fd;
    void *pd = s->pd;
    int64_t timeout_ms = s->read_timeout_ms;
    int64_t whole_deadline_ns =
        (s->read_deadline_ms > 0) ? zap_deadline_after_ms(s->read_deadline_ms) : 0;
    if (n < 0) {
        n = 0;
    }
    int initial = zap_io_status(pd, whole_deadline_ns);
    if (initial) {
        zap_io_panic(initial, "net: recvfrom failed");
        return NULL;
    }
    char *buf = (char *)malloc((size_t)n + 1);
    if (!buf) {
        zap_net_panic("net: out of memory");
        return NULL;
    }
    struct sockaddr_storage ss;
    memset(&ss, 0, sizeof ss);
    ssize_t got = 0;
    int have = 0; /* 0 until a datagram (possibly empty) is received */
    for (;;) {
        int status = zap_io_status(pd, whole_deadline_ns);
        if (status) {
            free(buf);
            zap_io_panic(status, "net: recvfrom failed");
            return NULL;
        }
        socklen_t slen = sizeof ss;
        ssize_t r = (ssize_t)recvfrom(fd, buf, (size_t)n, 0, (struct sockaddr *)&ss, &slen);
        if (r >= 0) {
            got = r;
            have = 1;
            break;
        }
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            int p = zap_sock_park2(fd, pd, ZAP_POLL_READ, timeout_ms, whole_deadline_ns);
            if (p == -2 || p == -3) {
                free(buf);
                zap_io_panic(p, "net: recvfrom failed");
                return NULL;
            }
            if (p < 0) {
                got = 0;
                have = 0; /* closed under us -> empty datagram, empty host */
                break;
            }
            continue;
        }
        free(buf);
        zap_io_panic(-1, "net: recvfrom failed");
        return NULL;
    }

    /* Format the numeric sender address into a stack buffer (no allocation, no
     * park). getaddrinfo is not needed on the receive side. */
    char hostbuf[INET6_ADDRSTRLEN];
    hostbuf[0] = '\0';
    int64_t sport = 0;
    if (have) {
        if (ss.ss_family == AF_INET6) {
            struct sockaddr_in6 *a6 = (struct sockaddr_in6 *)&ss;
            inet_ntop(AF_INET6, &a6->sin6_addr, hostbuf, sizeof hostbuf);
            sport = (int64_t)ntohs(a6->sin6_port);
        } else if (ss.ss_family == AF_INET) {
            struct sockaddr_in *a4 = (struct sockaddr_in *)&ss;
            inet_ntop(AF_INET, &a4->sin_addr, hostbuf, sizeof hostbuf);
            sport = (int64_t)ntohs(a4->sin_port);
        }
    }

    /* Build the GC results last, no park after (precise-Immix discipline): the
     * container first, then each field is filled from a fresh allocation and
     * stored immediately — the same order process.run uses for ZapProc. */
    ZapDatagram *out = (ZapDatagram *)zap_obj(&zap_desc_dgram);
    out->data = NULL;
    out->host = NULL;
    out->port = sport;
    out->data = (ZapStr *)zap_str_new(buf, (int64_t)got);
    free(buf);
    out->host = (ZapStr *)zap_str_new(hostbuf, (int64_t)strlen(hostbuf));
    zap_check_cancel();
    return out;
}

/* net.recvfrom field getters (infallible), unpacked into the (str, str, int64)
 * tuple by the lowerer. */
void *zap_udp_dgram_data(void *d_) { return ((ZapDatagram *)d_)->data; }
void *zap_udp_dgram_host(void *d_) { return ((ZapDatagram *)d_)->host; }
int64_t zap_udp_dgram_port(void *d_) { return ((ZapDatagram *)d_)->port; }


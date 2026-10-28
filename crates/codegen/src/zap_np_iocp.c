/* =============================================================================
 * Windows IOCP netpoller backend — mechanism (ticket #196, platform epic #158 /
 * Area A2; design/0020-windows-iocp-netpoller-a2.md).
 *
 * Self-contained AFD-readiness-over-IOCP poller. See zap_np_iocp.h for the ABI
 * and the model-fit rationale. Only <winsock2.h>/<windows.h>/<winternl.h> and
 * freestanding headers — no other runtime headers, exactly like zap_ctx_win_x64.c
 * and zap_win_a3.c — so it links into the Windows program build AND compiles
 * standalone in zap_np_iocp_test.c.
 *
 * AFD poll (`\Device\Afd` + IOCTL_AFD_POLL via NtDeviceIoControlFile) is the
 * readiness mechanism wepoll/libuv/mio use; the structures are only semi-documented
 * and live in ntdll, so NtCreateFile/NtDeviceIoControlFile are resolved at runtime
 * from ntdll.dll (no import-library dependency) and the AFD structs/IOCTL are
 * declared here from the well-known public values.
 * ========================================================================== */
#if defined(_WIN32)

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
/* GetQueuedCompletionStatusEx / CancelIoEx / SetFileCompletionNotificationModes
 * need Vista+ (0x0600). MinGW-w64 defaults high enough, but pin it so the build
 * is deterministic regardless of the toolchain's default target. */
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601
#endif
#include <winsock2.h>
#include <windows.h>
#include <winternl.h> /* NTSTATUS, IO_STATUS_BLOCK, OBJECT_ATTRIBUTES, UNICODE_STRING */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* SIO_BASE_HANDLE lives in <mstcpip.h> in the Windows SDK and in MinGW-w64, but
 * its header has moved across SDK versions; define it from its well-known value
 * if the headers above did not provide it (the _WSAIOR macro is in winsock2.h).
 * SIO_BSP_HANDLE_POLL / SIO_BSP_HANDLE are the layered-LSP fallbacks (I#221) and
 * use the same mstcpip.h values / _WSAIOR convention. */
#ifndef SIO_BASE_HANDLE
#define SIO_BASE_HANDLE _WSAIOR(IOC_WS2, 34)
#endif
#ifndef SIO_BSP_HANDLE_POLL
#define SIO_BSP_HANDLE_POLL _WSAIOR(IOC_WS2, 29)
#endif
#ifndef SIO_BSP_HANDLE
#define SIO_BSP_HANDLE _WSAIOR(IOC_WS2, 27)
#endif

/* NtCreateFile disposition, provided by the DDK headers; define from the public
 * value if <winternl.h> did not. */
#ifndef FILE_OPEN
#define FILE_OPEN 0x00000001
#endif

/* ---- AFD poll ABI (public well-known values; mirrors wepoll/mio afd) -------- */
#define ZAP_AFD_POLL_RECEIVE 0x0001
#define ZAP_AFD_POLL_RECEIVE_EXPEDITED 0x0002
#define ZAP_AFD_POLL_SEND 0x0004
#define ZAP_AFD_POLL_DISCONNECT 0x0008 /* graceful close (readable EOF)         */
#define ZAP_AFD_POLL_ABORT 0x0010      /* reset                                 */
#define ZAP_AFD_POLL_LOCAL_CLOSE 0x0020
#define ZAP_AFD_POLL_ACCEPT 0x0080
#define ZAP_AFD_POLL_CONNECT_FAIL 0x0100

#define ZAP_IOCTL_AFD_POLL 0x00012024

#ifndef STATUS_SUCCESS
#define STATUS_SUCCESS ((NTSTATUS)0x00000000L)
#endif
#ifndef STATUS_PENDING
#define STATUS_PENDING ((NTSTATUS)0x00000103L)
#endif
#ifndef STATUS_CANCELLED
#define STATUS_CANCELLED ((NTSTATUS)0xC0000120L)
#endif

typedef struct {
    HANDLE Handle;
    ULONG Events;
    NTSTATUS Status;
} ZAP_AFD_POLL_HANDLE_INFO;

typedef struct {
    LARGE_INTEGER Timeout;
    ULONG NumberOfHandles;
    ULONG Exclusive;
    ZAP_AFD_POLL_HANDLE_INFO Handles[1];
} ZAP_AFD_POLL_INFO;

/* ntdll entry points resolved at runtime (no -lntdll needed). */
typedef NTSTATUS(NTAPI *zap_NtCreateFile_t)(PHANDLE, ACCESS_MASK, POBJECT_ATTRIBUTES,
                                            PIO_STATUS_BLOCK, PLARGE_INTEGER, ULONG,
                                            ULONG, ULONG, ULONG, PVOID, ULONG);
/* The ApcRoutine parameter is typed PVOID (we always pass NULL) to avoid a
 * dependency on PIO_APC_ROUTINE's header; NULL is ABI-identical. */
typedef NTSTATUS(NTAPI *zap_NtDeviceIoControlFile_t)(HANDLE, HANDLE, PVOID, PVOID,
                                                     PIO_STATUS_BLOCK, ULONG, PVOID, ULONG,
                                                     PVOID, ULONG);
typedef ULONG(NTAPI *zap_RtlNtStatusToDosError_t)(NTSTATUS);

static zap_NtCreateFile_t p_NtCreateFile;
static zap_NtDeviceIoControlFile_t p_NtDeviceIoControlFile;
static zap_RtlNtStatusToDosError_t p_RtlNtStatusToDosError;

/* ---- per-direction one-shot poll request ---------------------------------- */
/* `ov` MUST be the first member: the IOCP completion returns its address and we
 * recover the request with a plain cast (no CONTAINING_RECORD arithmetic needed,
 * but we keep a back-pointer + dir for clarity). */
typedef struct zap_iocp_req {
    OVERLAPPED ov;
    ZAP_AFD_POLL_INFO info; /* in+out buffer for IOCTL_AFD_POLL                 */
    struct zap_iocp_pd *owner;
    int dir;   /* ZAP_IOCP_READ / ZAP_IOCP_WRITE                               */
    int armed; /* a poll request is in flight                                  */
} zap_iocp_req;

typedef struct zap_iocp_pd {
    zap_iocp_req reqs[2]; /* [ZAP_IOCP_READ], [ZAP_IOCP_WRITE]                  */
    SOCKET base;          /* base socket handle (SIO_BASE_HANDLE)              */
    void *userkey;        /* echoed to zap_iocp_wait (the owning PollDesc *)   */
    int closing;          /* zap_iocp_unregister has been called              */
    int pending;          /* in-flight reqs not yet reaped (deferred free)    */
} zap_iocp_pd;

#include "zap_np_iocp.h"

/* ---- global state --------------------------------------------------------- */
static HANDLE g_iocp = NULL; /* completion port                                */
static HANDLE g_afd = NULL;  /* \Device\Afd helper handle, associated w/ g_iocp */
static CRITICAL_SECTION g_mu; /* serializes arm/is_armed/unregister/wait reap  */
static int g_mu_init = 0;
static volatile LONG g_last_error = 0;

/* PostQueuedCompletionStatus sentinel: a NULL overlapped marks a kick. */

static void zap_iocp_set_error(int code) {
    InterlockedExchange(&g_last_error, (LONG)code);
}

int zap_iocp_last_error(void) {
    return (int)InterlockedCompareExchange(&g_last_error, 0, 0);
}

static int zap_iocp_resolve_ntdll(void) {
    HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    if (!ntdll) {
        zap_iocp_set_error((int)GetLastError());
        return -1;
    }
    p_NtCreateFile = (zap_NtCreateFile_t)(void *)GetProcAddress(ntdll, "NtCreateFile");
    p_NtDeviceIoControlFile =
        (zap_NtDeviceIoControlFile_t)(void *)GetProcAddress(ntdll, "NtDeviceIoControlFile");
    p_RtlNtStatusToDosError =
        (zap_RtlNtStatusToDosError_t)(void *)GetProcAddress(ntdll, "RtlNtStatusToDosError");
    if (!p_NtCreateFile || !p_NtDeviceIoControlFile) {
        zap_iocp_set_error((int)GetLastError());
        return -1;
    }
    return 0;
}

static void zap_iocp_record_nt(NTSTATUS st) {
    int code = p_RtlNtStatusToDosError ? (int)p_RtlNtStatusToDosError(st) : (int)st;
    zap_iocp_set_error(code);
}

/* Open the \Device\Afd helper handle used to issue IOCTL_AFD_POLL. The name after
 * "\Device\Afd" is arbitrary (ignored by AFD); a distinctive suffix just aids
 * debugging, like wepoll's "\Device\Afd\Wepoll". */
static int zap_iocp_open_afd(void) {
    static WCHAR afd_name[] = L"\\Device\\Afd\\Zap";
    UNICODE_STRING name;
    name.Buffer = afd_name;
    name.Length = (USHORT)(sizeof(afd_name) - sizeof(WCHAR)); /* bytes, no NUL   */
    name.MaximumLength = (USHORT)sizeof(afd_name);

    /* InitializeObjectAttributes is a DDK macro that is not always visible via
     * <winternl.h>; fill the struct directly (Length + ObjectName, rest zeroed). */
    OBJECT_ATTRIBUTES oa;
    memset(&oa, 0, sizeof oa);
    oa.Length = sizeof oa;
    oa.ObjectName = &name;

    IO_STATUS_BLOCK iosb;
    memset(&iosb, 0, sizeof iosb);
    NTSTATUS st = p_NtCreateFile(&g_afd, SYNCHRONIZE, &oa, &iosb, NULL, 0,
                                 FILE_SHARE_READ | FILE_SHARE_WRITE, FILE_OPEN, 0, NULL, 0);
    if (st != STATUS_SUCCESS) {
        zap_iocp_record_nt(st);
        g_afd = NULL;
        return -1;
    }
    return 0;
}

int zap_iocp_global_open(void) {
    if (zap_iocp_resolve_ntdll() != 0) {
        return -1;
    }
    g_iocp = CreateIoCompletionPort(INVALID_HANDLE_VALUE, NULL, 0, 0);
    if (!g_iocp) {
        zap_iocp_set_error((int)GetLastError());
        return -1;
    }
    if (zap_iocp_open_afd() != 0) {
        CloseHandle(g_iocp);
        g_iocp = NULL;
        return -1;
    }
    /* Associate the AFD helper with the port; completions for its async IOCTLs
     * post the issuing OVERLAPPED. Skip the needless event-handle signal. */
    if (CreateIoCompletionPort(g_afd, g_iocp, 0, 0) == NULL) {
        zap_iocp_set_error((int)GetLastError());
        CloseHandle(g_afd);
        g_afd = NULL;
        CloseHandle(g_iocp);
        g_iocp = NULL;
        return -1;
    }
    SetFileCompletionNotificationModes(g_afd, FILE_SKIP_SET_EVENT_ON_HANDLE);
    InitializeCriticalSection(&g_mu);
    g_mu_init = 1;
    return 0;
}

void zap_iocp_global_close(void) {
    if (g_afd) {
        CloseHandle(g_afd);
        g_afd = NULL;
    }
    if (g_iocp) {
        CloseHandle(g_iocp);
        g_iocp = NULL;
    }
    if (g_mu_init) {
        DeleteCriticalSection(&g_mu);
        g_mu_init = 0;
    }
}

/* One WSAIoctl base/BSP-handle unwrap; returns the handle, or INVALID_SOCKET on
 * failure (the caller inspects WSAGetLastError when it needs the reason). */
static SOCKET zap_iocp_ioctl_handle(SOCKET s, DWORD ioctl_code) {
    SOCKET out = INVALID_SOCKET;
    DWORD bytes = 0;
    if (WSAIoctl(s, ioctl_code, NULL, 0, &out, sizeof out, &bytes, NULL, NULL) !=
        SOCKET_ERROR) {
        return out;
    }
    return INVALID_SOCKET;
}

/* Unwrap the pollable *base* socket handle — the one at the bottom of any layered
 * service provider (LSP) stack, which is what IOCTL_AFD_POLL must target.
 *
 * v1 (#196) used SIO_BASE_HANDLE only and, per I#196 Q2, failed loudly if that did
 * not yield a base handle rather than hanging. I#221 adds the layered-LSP fallback
 * chain, mirroring wepoll's `ws_get_base_socket` ordering, for the rare non-IFS
 * LSPs (antivirus / VPN) that break SIO_BASE_HANDLE:
 *
 *   - SIO_BASE_HANDLE on the current handle. If it returns a handle that differs
 *     from the one queried, that is the real base below a layer -> use it.
 *   - If it returns the *same* handle, that is the base for a plain (non-layered)
 *     socket (the common case) — but a Komodia-style LSP can also make it echo the
 *     layered handle. Remember it as a fallback, then try to descend one layer
 *     (SIO_BSP_HANDLE_POLL, then SIO_BSP_HANDLE) and retry SIO_BASE_HANDLE lower.
 *   - If SIO_BASE_HANDLE fails outright (a broken LSP intercepting it), descend the
 *     same way and retry on the lower handle.
 *
 * The loop ends when SIO_BASE_HANDLE yields a lower base, when we can descend no
 * further (return the best same-as-queried base we saw — the plain-socket case), or
 * when nothing is pollable. In that last case we keep the v1 contract: record the
 * error and return INVALID_SOCKET so zap_iocp_register returns NULL and the caller
 * reports it. We never hang. A small hop cap guards against a malformed LSP that
 * returns a descent cycle instead of strictly descending.
 */
static SOCKET zap_iocp_base_handle(SOCKET s) {
    SOCKET cur = s;
    SOCKET best = INVALID_SOCKET; /* lowest "base == queried" handle seen         */
    int last_err = 0;
    for (int hop = 0; hop < 16; hop++) {
        SOCKET base = zap_iocp_ioctl_handle(cur, SIO_BASE_HANDLE);
        if (base != INVALID_SOCKET) {
            if (base != cur) {
                return base; /* real base one or more layers down                */
            }
            best = base; /* plain-socket base, or a layered handle to fall back on */
        } else {
            last_err = WSAGetLastError();
            if (last_err == WSAENOTSOCK) {
                break; /* not a socket at all — nothing to unwrap                 */
            }
        }
        /* Descend one layer toward the base: SIO_BSP_HANDLE_POLL, then the older
         * SIO_BSP_HANDLE, and retry SIO_BASE_HANDLE on whatever is lower. */
        SOCKET lower = zap_iocp_ioctl_handle(cur, SIO_BSP_HANDLE_POLL);
        if (lower == INVALID_SOCKET || lower == cur) {
            lower = zap_iocp_ioctl_handle(cur, SIO_BSP_HANDLE);
        }
        if (lower != INVALID_SOCKET && lower != cur) {
            cur = lower;
            continue;
        }
        break; /* cannot descend further                                         */
    }
    if (best != INVALID_SOCKET) {
        return best; /* plain socket, or best layered guess (still pollable)      */
    }
    zap_iocp_set_error(last_err ? last_err : WSAGetLastError());
    return INVALID_SOCKET;
}

void *zap_iocp_register(uint64_t sock, void *userkey) {
    SOCKET base = zap_iocp_base_handle((SOCKET)sock);
    if (base == INVALID_SOCKET) {
        return NULL;
    }
    zap_iocp_pd *pd = (zap_iocp_pd *)calloc(1, sizeof *pd);
    if (!pd) {
        zap_iocp_set_error((int)ERROR_NOT_ENOUGH_MEMORY);
        return NULL;
    }
    pd->base = base;
    pd->userkey = userkey;
    pd->closing = 0;
    pd->pending = 0;
    pd->reqs[ZAP_IOCP_READ].owner = pd;
    pd->reqs[ZAP_IOCP_READ].dir = ZAP_IOCP_READ;
    pd->reqs[ZAP_IOCP_WRITE].owner = pd;
    pd->reqs[ZAP_IOCP_WRITE].dir = ZAP_IOCP_WRITE;
    return pd;
}

/* Issue a one-shot IOCTL_AFD_POLL for `req`'s direction. Caller holds g_mu. */
static int zap_iocp_arm_locked(zap_iocp_pd *pd, zap_iocp_req *req) {
    ULONG events = ZAP_AFD_POLL_ABORT | ZAP_AFD_POLL_DISCONNECT |
                   ZAP_AFD_POLL_LOCAL_CLOSE | ZAP_AFD_POLL_CONNECT_FAIL;
    if (req->dir == ZAP_IOCP_READ) {
        events |= ZAP_AFD_POLL_RECEIVE | ZAP_AFD_POLL_RECEIVE_EXPEDITED | ZAP_AFD_POLL_ACCEPT;
    } else {
        events |= ZAP_AFD_POLL_SEND;
    }

    memset(&req->ov, 0, sizeof req->ov);
    req->info.Timeout.QuadPart = INT64_MAX; /* infinite; our wait() owns timing  */
    req->info.NumberOfHandles = 1;
    req->info.Exclusive = FALSE;
    req->info.Handles[0].Handle = (HANDLE)pd->base;
    req->info.Handles[0].Status = 0;
    req->info.Handles[0].Events = events;

    /* The IO_STATUS_BLOCK aliases the OVERLAPPED's Internal/InternalHigh, and the
     * OVERLAPPED is passed as the ApcContext so the completion posts it to the
     * port (the canonical wepoll idiom). */
    IO_STATUS_BLOCK *iosb = (IO_STATUS_BLOCK *)&req->ov.Internal;
    iosb->Status = STATUS_PENDING;

    NTSTATUS st = p_NtDeviceIoControlFile(g_afd, NULL, NULL, &req->ov, iosb,
                                          ZAP_IOCTL_AFD_POLL, &req->info, sizeof req->info,
                                          &req->info, sizeof req->info);
    if (st == STATUS_SUCCESS || st == STATUS_PENDING) {
        req->armed = 1;
        pd->pending++;
        return 0;
    }
    zap_iocp_record_nt(st);
    return -1;
}

int zap_iocp_arm(void *pd_, int dir) {
    zap_iocp_pd *pd = (zap_iocp_pd *)pd_;
    if (dir != ZAP_IOCP_READ && dir != ZAP_IOCP_WRITE) {
        return -1;
    }
    EnterCriticalSection(&g_mu);
    int rc = 0;
    if (!pd->closing && !pd->reqs[dir].armed) {
        rc = zap_iocp_arm_locked(pd, &pd->reqs[dir]);
    }
    LeaveCriticalSection(&g_mu);
    return rc;
}

int zap_iocp_is_armed(void *pd_, int dir) {
    zap_iocp_pd *pd = (zap_iocp_pd *)pd_;
    if (dir != ZAP_IOCP_READ && dir != ZAP_IOCP_WRITE) {
        return 0;
    }
    EnterCriticalSection(&g_mu);
    int a = pd->reqs[dir].armed;
    LeaveCriticalSection(&g_mu);
    return a;
}

void zap_iocp_unregister(void *pd_) {
    zap_iocp_pd *pd = (zap_iocp_pd *)pd_;
    EnterCriticalSection(&g_mu);
    pd->closing = 1;
    /* Cancel in-flight polls; each cancellation delivers exactly one completion,
     * reaped (and dropped) by the poller, which then frees pd when pending hits 0.
     * CancelIoEx targets the handle the op was issued on (g_afd) by OVERLAPPED. */
    for (int d = 0; d < 2; d++) {
        if (pd->reqs[d].armed) {
            CancelIoEx(g_afd, &pd->reqs[d].ov);
        }
    }
    int can_free = (pd->pending == 0);
    LeaveCriticalSection(&g_mu);
    if (can_free) {
        free(pd);
    }
}

int zap_iocp_wait(zap_iocp_ready *out, int max, int timeout_ms) {
    OVERLAPPED_ENTRY entries[64];
    ULONG want = (ULONG)(max < 64 ? max : 64);
    if (want == 0) {
        want = 1;
    }
    ULONG n = 0;
    DWORD to = (timeout_ms < 0) ? INFINITE : (DWORD)timeout_ms;
    BOOL ok = GetQueuedCompletionStatusEx(g_iocp, entries, want, &n, to, FALSE);
    if (!ok) {
        DWORD e = GetLastError();
        if (e == WAIT_TIMEOUT) {
            return 0;
        }
        zap_iocp_set_error((int)e);
        return -1;
    }

    int count = 0;
    for (ULONG i = 0; i < n; i++) {
        if (entries[i].lpOverlapped == NULL) {
            continue; /* kick (PostQueuedCompletionStatus with NULL overlapped)  */
        }
        zap_iocp_req *req = (zap_iocp_req *)entries[i].lpOverlapped; /* ov is first */
        EnterCriticalSection(&g_mu);
        zap_iocp_pd *pd = req->owner;
        req->armed = 0;
        pd->pending--;
        if (pd->closing) {
            int can_free = (pd->pending == 0);
            LeaveCriticalSection(&g_mu);
            if (can_free) {
                free(pd);
            }
            continue; /* torn down: reap, do not report                         */
        }
        ULONG events = req->info.Handles[0].Events;
        void *userkey = pd->userkey;
        LeaveCriticalSection(&g_mu);

        int err = (events & (ZAP_AFD_POLL_ABORT | ZAP_AFD_POLL_DISCONNECT |
                             ZAP_AFD_POLL_LOCAL_CLOSE | ZAP_AFD_POLL_CONNECT_FAIL)) != 0;
        int readable = err || (req->dir == ZAP_IOCP_READ &&
                               (events & (ZAP_AFD_POLL_RECEIVE | ZAP_AFD_POLL_RECEIVE_EXPEDITED |
                                          ZAP_AFD_POLL_ACCEPT)) != 0);
        int writable = err || (req->dir == ZAP_IOCP_WRITE && (events & ZAP_AFD_POLL_SEND) != 0);
        int closed = (events & (ZAP_AFD_POLL_ABORT | ZAP_AFD_POLL_LOCAL_CLOSE)) != 0;

        if (count < max) {
            out[count].userkey = userkey;
            out[count].readable = readable;
            out[count].writable = writable;
            out[count].closed = closed;
            count++;
        }
    }
    return count;
}

void zap_iocp_kick(void) {
    if (g_iocp) {
        PostQueuedCompletionStatus(g_iocp, 0, 0, NULL);
    }
}

#else
/* Non-Windows: empty translation unit (this file is only added to the Windows
 * link arm of build_executable; the guard keeps a stray compile a clean no-op). */
typedef int zap_np_iocp_translation_unit_not_empty;
#endif /* _WIN32 */

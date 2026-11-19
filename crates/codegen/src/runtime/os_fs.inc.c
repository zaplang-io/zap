/* zap runtime fragment: os_fs.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * os / sys / io / fs / process (ticket #97, D-0056). The system, I/O and
 * process core of the bundled stdlib, dispatched by flat name through the
 * module seam (#121): `os`/`sys`/`io`/`fs`/`process` are compiler-known
 * namespaced modules (`import` accepted but not required), exactly like
 * `signal`/`gc`/`runtime`.
 *
 * Design (flagged in the PR):
 *   - Files are raw int64 fds (no handle object / prelude type); the stdio
 *     handles `sys.std{in,out,err}_fd()` are the constants 0/1/2 (lowered
 *     inline, no runtime call here).
 *   - `fs.stat`-style queries are separate scalar/bool entry points
 *     (`fs.size/is_dir/is_file/exists`) so no stat struct is needed.
 *   - `process.run(list[str])` captures stdout/stderr into temp files (NOT
 *     pipes) and `waitpid`s, so there is no pipe-buffer deadlock and no fork;
 *     it returns a GC `ZapProc` whose three fields the lowerer reads back into
 *     a `(int64, str, str)` tuple = (exit_code, stdout, stderr).
 *   - Fallible calls raise a catchable Zap exception via `zap_panic` (the
 *     message is "<op>: <errno text>"), matching the rest of the runtime.
 *
 * GC safety: a collection only ever runs at a compiled-code safepoint (see
 * zapgc.inc.c), never inside a runtime C call, so GC objects built here and
 * held in C locals stay live for the duration of the call without rooting.
 * The one blocking point (`waitpid`) is wrapped in zap_enter/leave_blocking so
 * a concurrent stop-the-world GC need not wait for this thread (its roots are
 * already flushed; no un-flushed Zap temporaries are live across the wait).
 * ========================================================================== */
#include <dirent.h> /* MinGW-w64 ships a dirent shim over FindFirstFile          */
#include <errno.h>
#include <fcntl.h>
#include <stddef.h> /* offsetof */
#include <sys/stat.h>
#if defined(ZAP_PLATFORM_POSIX)
#include <poll.h>     /* poll: process streaming no-netpoller fallback           */
#include <signal.h>   /* kill / SIGTERM / SIGKILL: process.run timeout + Popen    */
#include <spawn.h>    /* posix_spawn: process.run (POSIX)                        */
#include <sys/wait.h> /* waitpid                                                 */
#include <time.h>     /* nanosleep: process.run timeout poll                     */
#endif
#if defined(__APPLE__)
#include <crt_externs.h> /* _NSGetArgc / _NSGetArgv */
#endif

#if defined(ZAP_PLATFORM_POSIX)
extern char **environ; /* process.run passes it to posix_spawnp (Windows inherits
                        * the environment via CreateProcessW, so it is unused) */
#endif

/* Windows opens file descriptors in TEXT mode by default, translating CRLF and
 * stopping at ^Z — corrupting binary data and byte counts. Zap's io/fs surface is
 * byte-exact (Python-faithful binary-safe strings), so every open() ORs in
 * O_BINARY on Windows; it is a no-op (0) on POSIX, keeping those paths unchanged. */
#if defined(ZAP_PLATFORM_WINDOWS)
#define ZAP_O_BINARY O_BINARY
#else
#define ZAP_O_BINARY 0
#endif

/* NUL-terminated copy of a ZapStr for the C-string syscall APIs (ZapStr data is
 * length-prefixed, not NUL-terminated). The buffer is a GC leaf; no safepoint
 * runs inside a runtime call, so holding it in a C local is safe. */
static char *zap_sys_cstr(const ZapStr *s) {
    char *buf = (char *)zap_buf((int64_t)s->len + 1);
    if (s->len > 0) {
        memcpy(buf, s->data, (size_t)s->len);
    }
    buf[s->len] = '\0';
    return buf;
}

/* Raise a catchable Zap exception "<op>: <detail>" (never returns). */
static void zap_sys_fail(const char *op, const char *detail) {
    char buf[512];
    int n = snprintf(buf, sizeof buf, "%s: %s", op, detail);
    if (n < 0) {
        n = 0;
    }
    if (n >= (int)sizeof buf) {
        n = (int)sizeof buf - 1;
    }
    zap_panic(zap_str_new(buf, (int64_t)n));
}

/* --- os: environment -------------------------------------------------------- */

/* os.getenv(name) -> str: the variable's value, or "" when unset (use
 * os.hasenv to distinguish unset from empty; there is no None in v1). */
void *zap_os_getenv(void *name_) {
    const char *v = getenv(zap_sys_cstr((ZapStr *)name_));
    return v ? zap_str_new(v, (int64_t)strlen(v)) : zap_str_new("", 0);
}

/* os.hasenv(name) -> bool. */
int64_t zap_os_hasenv(void *name_) {
    return getenv(zap_sys_cstr((ZapStr *)name_)) != NULL ? 1 : 0;
}

/* os.setenv(name, value) (overwrites). */
void zap_os_setenv(void *name_, void *val_) {
    if (zap_setenv_compat(zap_sys_cstr((ZapStr *)name_), zap_sys_cstr((ZapStr *)val_),
                          1) != 0) {
        zap_sys_fail("os.setenv", strerror(errno));
    }
}

/* os.getpid() -> int64. */
int64_t zap_os_getpid(void) { return (int64_t)getpid(); }

/* --- os: working directory & entries ---------------------------------------- */

/* os.getcwd() -> str. */
void *zap_os_getcwd(void) {
    char buf[4096];
    if (getcwd(buf, sizeof buf) == NULL) {
        zap_sys_fail("os.getcwd", strerror(errno));
    }
    return zap_str_new(buf, (int64_t)strlen(buf));
}

/* os.chdir(path). */
void zap_os_chdir(void *path_) {
    if (chdir(zap_sys_cstr((ZapStr *)path_)) != 0) {
        zap_sys_fail("os.chdir", strerror(errno));
    }
}

/* os.mkdir(path) (mode 0777 before umask on POSIX; Windows has no mode arg and
 * applies the default ACL — _mkdir takes the path only). */
void zap_os_mkdir(void *path_) {
#if defined(ZAP_PLATFORM_WINDOWS)
    if (_mkdir(zap_sys_cstr((ZapStr *)path_)) != 0) {
#else
    if (mkdir(zap_sys_cstr((ZapStr *)path_), 0777) != 0) {
#endif
        zap_sys_fail("os.mkdir", strerror(errno));
    }
}

/* os.remove(path): unlink a file. */
void zap_os_remove(void *path_) {
    if (unlink(zap_sys_cstr((ZapStr *)path_)) != 0) {
        zap_sys_fail("os.remove", strerror(errno));
    }
}

/* os.rmdir(path): remove an empty directory. */
void zap_os_rmdir(void *path_) {
    if (rmdir(zap_sys_cstr((ZapStr *)path_)) != 0) {
        zap_sys_fail("os.rmdir", strerror(errno));
    }
}

/* os.listdir(path) / fs.listdir(path) -> list[str]: entry names (excluding
 * "." and ".."), in unspecified order. */
void *zap_os_listdir(void *path_) {
    char *path = zap_sys_cstr((ZapStr *)path_);
    DIR *d = opendir(path);
    if (d == NULL) {
        zap_sys_fail("os.listdir", strerror(errno));
    }
    void *out = zap_list_new();
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
            continue;
        }
        zap_list_push_ptr(out, zap_str_new(e->d_name, (int64_t)strlen(e->d_name)));
    }
    closedir(d);
    return out;
}

/* --- os.path helpers (flat names: Zap dispatch is `module.method`) ---------- */

/* Is `c` a path separator? Zap uses '/' as its one portable, Python-readable
 * separator on every platform; on Windows a '\\' that appears in user input (or
 * a native API result) is ALSO a separator, so join recognises it rather than
 * doubling it. POSIX is unchanged ('/' only). */
static int zap_path_is_sep(char c) {
#if defined(ZAP_PLATFORM_WINDOWS)
    return c == '/' || c == '\\';
#else
    return c == '/';
#endif
}

/* Is `b` absolute (so a join should discard the left operand)? POSIX: a leading
 * '/'. Windows (Python-faithful): a leading separator OR a drive-qualified path
 * like "C:\\x" / "C:/x" (os.path.join("a", "C:\\x") -> "C:\\x"). */
static int zap_path_is_abs(const ZapStr *b) {
    if (b->len > 0 && zap_path_is_sep(b->data[0])) {
        return 1;
    }
#if defined(ZAP_PLATFORM_WINDOWS)
    if (b->len >= 2 && b->data[1] == ':' &&
        ((b->data[0] >= 'A' && b->data[0] <= 'Z') ||
         (b->data[0] >= 'a' && b->data[0] <= 'z'))) {
        return 1;
    }
#endif
    return 0;
}

/* os.path_join(a, b): join with a single '/'. An absolute `b` replaces `a`.
 * The inserted separator is always '/' (Zap's portable, Python-readable choice,
 * accepted by every Windows API); an existing trailing '/' or '\\' on `a` is not
 * doubled. */
void *zap_os_path_join(void *a_, void *b_) {
    ZapStr *a = (ZapStr *)a_;
    ZapStr *b = (ZapStr *)b_;
    if (b->len > 0 && zap_path_is_abs(b)) {
        return zap_str_new(b->data, b->len);
    }
    if (a->len == 0) {
        return zap_str_new(b->data, b->len);
    }
    int need_sep = !zap_path_is_sep(a->data[a->len - 1]);
    int64_t n = a->len + (need_sep ? 1 : 0) + b->len;
    char *buf = (char *)zap_buf(n);
    memcpy(buf, a->data, (size_t)a->len);
    int64_t off = a->len;
    if (need_sep) {
        buf[off++] = '/';
    }
    memcpy(buf + off, b->data, (size_t)b->len);
    return zap_str_new(buf, n);
}

/* os.path_exists(path) -> bool. */
int64_t zap_os_path_exists(void *path_) {
    return access(zap_sys_cstr((ZapStr *)path_), F_OK) == 0 ? 1 : 0;
}

/* os.path_basename(path): the final component ("" for a trailing-slash path). */
void *zap_os_path_basename(void *path_) {
    ZapStr *p = (ZapStr *)path_;
    int64_t i = p->len;
    while (i > 0 && p->data[i - 1] == '/') {
        i--; /* ignore trailing slashes, like POSIX basename of "a/" -> "a" */
    }
    int64_t end = i;
    while (i > 0 && p->data[i - 1] != '/') {
        i--;
    }
    return zap_str_new(p->data + i, end - i);
}

/* os.path_dirname(path): everything up to the last '/' (no trailing slash);
 * "." when there is no '/'. */
void *zap_os_path_dirname(void *path_) {
    ZapStr *p = (ZapStr *)path_;
    int64_t i = p->len;
    while (i > 0 && p->data[i - 1] == '/') {
        i--;
    }
    while (i > 0 && p->data[i - 1] != '/') {
        i--;
    }
    while (i > 1 && p->data[i - 1] == '/') {
        i--; /* collapse the trailing separator(s), keeping a lone leading "/" */
    }
    if (i == 0) {
        return zap_str_new(".", 1);
    }
    return zap_str_new(p->data, i);
}

/* --- path split / splitext (#1030, D-0219) ---------------------------------
 *
 * CPython `os.path.split` / `os.path.splitext`, returned as a `(str, str)`
 * tuple built by the lowerer from these getters. These deliberately differ from
 * `path_basename`/`path_dirname` above (POSIX-command semantics): `split`
 * strips trailing separators from the head only when the head is not all
 * separators, keeps the tail empty for a trailing-slash path, and gives an empty
 * head (not ".") when there is no separator — the exact `posixpath.split`
 * contract. */

/* os.path_split head: everything up to and excluding the last '/', with trailing
 * separators stripped unless the head is all separators (posixpath.split[0]). */
void *zap_os_path_split_head(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    int64_t i = p->len;
    while (i > 0 && p->data[i - 1] != '/') {
        i--; /* i = index just past the last '/', or 0 when there is none */
    }
    int64_t hlen = i;
    int all_slash = 1;
    for (int64_t k = 0; k < hlen; k++) {
        if (p->data[k] != '/') {
            all_slash = 0;
            break;
        }
    }
    if (hlen > 0 && !all_slash) {
        while (hlen > 0 && p->data[hlen - 1] == '/') {
            hlen--;
        }
    }
    return zap_str_new(p->data, hlen);
}

/* os.path_split tail: the final component (posixpath.split[1]); "" for a
 * trailing-slash path, the whole string when there is no '/'. */
void *zap_os_path_split_tail(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    int64_t i = p->len;
    while (i > 0 && p->data[i - 1] != '/') {
        i--;
    }
    return zap_str_new(p->data + i, p->len - i);
}

/* Byte offset of the extension-splitting '.' (posixpath.splitext), or -1 when
 * there is no extension: the last '.' that comes after the last '/' and is not
 * part of a run of leading dots in the final component. */
static int64_t zap_os_splitext_dot(const ZapStr *p) {
    int64_t sep = -1, dot = -1;
    for (int64_t i = 0; i < p->len; i++) {
        if (p->data[i] == '/') {
            sep = i;
        } else if (p->data[i] == '.') {
            dot = i;
        }
    }
    if (dot > sep) {
        int64_t fi = sep + 1;
        while (fi < dot) {
            if (p->data[fi] != '.') {
                return dot;
            }
            fi++;
        }
    }
    return -1;
}

/* os.path_splitext root: the path without its extension (posixpath.splitext[0]). */
void *zap_os_path_splitext_root(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    int64_t d = zap_os_splitext_dot(p);
    return d < 0 ? zap_str_new(p->data, p->len) : zap_str_new(p->data, d);
}

/* os.path_splitext ext: the extension incl. the leading '.', or "" when none
 * (posixpath.splitext[1]). */
void *zap_os_path_splitext_ext(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    int64_t d = zap_os_splitext_dot(p);
    return d < 0 ? zap_str_new("", 0) : zap_str_new(p->data + d, p->len - d);
}

/* --- sys -------------------------------------------------------------------- */

#if defined(__APPLE__)
static int zap_prog_argc(void) { return *_NSGetArgc(); }
static char **zap_prog_argv(void) { return *_NSGetArgv(); }
#elif defined(ZAP_PLATFORM_WINDOWS)
/* The MSVCRT startup populates the __argc / __argv globals (declared by
 * <stdlib.h>) before main runs — the Windows analogue of the ELF init-array
 * (argc, argv) capture below. */
static int zap_prog_argc(void) { return __argc; }
static char **zap_prog_argv(void) { return __argv; }
#else
/* On ELF platforms the dynamic loader passes (argc, argv, envp) to init-array
 * constructors; capture them for sys.argv (there is no argv parameter on the
 * compiled `main`, which has the C signature `int main(void)`). */
static int zap_saved_argc = 0;
static char **zap_saved_argv = NULL;
__attribute__((constructor)) static void zap_capture_argv(int argc, char **argv,
                                                          char **envp) {
    (void)envp;
    zap_saved_argc = argc;
    zap_saved_argv = argv;
}
static int zap_prog_argc(void) { return zap_saved_argc; }
static char **zap_prog_argv(void) { return zap_saved_argv; }
#endif

/* sys.argv() -> list[str]: the process arguments (argv[0] is the program). */
void *zap_sys_argv(void) {
    int argc = zap_prog_argc();
    char **argv = zap_prog_argv();
    void *out = zap_list_new();
    for (int i = 0; i < argc && argv != NULL && argv[i] != NULL; i++) {
        zap_list_push_ptr(out, zap_str_new(argv[i], (int64_t)strlen(argv[i])));
    }
    return out;
}

/* sys.exit(code): terminate the process immediately with `code`. */
void zap_sys_exit(int64_t code) { exit((int)code); }

/* sys.platform() -> str. Mirrors CPython's sys.platform values ("win32" on all
 * Windows, 64-bit included, per Python convention). */
void *zap_sys_platform(void) {
#if defined(__APPLE__)
    return zap_str_new("darwin", 6);
#elif defined(__linux__)
    return zap_str_new("linux", 5);
#elif defined(ZAP_PLATFORM_WINDOWS)
    return zap_str_new("win32", 5);
#else
    return zap_str_new("unknown", 7);
#endif
}

/* --- io: fd-based file I/O -------------------------------------------------- */

/* io.open(path, mode) -> fd. Modes mirror C stdio: r r+ w w+ a a+. */
int64_t zap_io_open(void *path_, void *mode_) {
    const char *mode = zap_sys_cstr((ZapStr *)mode_);
    int flags;
    if (strcmp(mode, "r") == 0) {
        flags = O_RDONLY;
    } else if (strcmp(mode, "r+") == 0) {
        flags = O_RDWR;
    } else if (strcmp(mode, "w") == 0) {
        flags = O_WRONLY | O_CREAT | O_TRUNC;
    } else if (strcmp(mode, "w+") == 0) {
        flags = O_RDWR | O_CREAT | O_TRUNC;
    } else if (strcmp(mode, "a") == 0) {
        flags = O_WRONLY | O_CREAT | O_APPEND;
    } else if (strcmp(mode, "a+") == 0) {
        flags = O_RDWR | O_CREAT | O_APPEND;
    } else {
        zap_sys_fail("io.open", "invalid mode (want one of: r r+ w w+ a a+)");
        return -1; /* unreachable */
    }
    int fd = open(zap_sys_cstr((ZapStr *)path_), flags | ZAP_O_BINARY, 0644);
    if (fd < 0) {
        zap_sys_fail("io.open", strerror(errno));
    }
    return (int64_t)fd;
}

/* io.read(fd, n) -> str: up to `n` bytes ("" at EOF). */
void *zap_io_read(int64_t fd, int64_t n) {
    if (n < 0) {
        n = 0;
    }
    char *buf = (char *)zap_buf(n > 0 ? n : 1);
    ssize_t got;
    do {
        got = read((int)fd, buf, (size_t)n);
    } while (got < 0 && errno == EINTR);
    if (got < 0) {
        zap_sys_fail("io.read", strerror(errno));
    }
    return zap_str_new(buf, (int64_t)got);
}

/* io.write(fd, data) -> int64: bytes written (the full string). */
int64_t zap_io_write(int64_t fd, void *data_) {
    ZapStr *s = (ZapStr *)data_;
    size_t off = 0;
    while (off < (size_t)s->len) {
        ssize_t w = write((int)fd, s->data + off, (size_t)s->len - off);
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            zap_sys_fail("io.write", strerror(errno));
        }
        off += (size_t)w;
    }
    return (int64_t)off;
}

/* io.close(fd). */
void zap_io_close(int64_t fd) {
    if (close((int)fd) != 0) {
        zap_sys_fail("io.close", strerror(errno));
    }
}

/* --- fs: path-oriented convenience + metadata ------------------------------- */

/* Read an entire open fd to a fresh ZapStr (for regular files; uses the file
 * size when seekable, otherwise grows). Caller handles open/close. */
static void *zap_read_all_fd(int fd) {
    off_t end = lseek(fd, 0, SEEK_END);
    int64_t cap = (end > 0) ? (int64_t)end : 4096;
    if (end >= 0) {
        lseek(fd, 0, SEEK_SET);
    }
    char *buf = (char *)zap_buf(cap);
    int64_t got = 0;
    for (;;) {
        if (got == cap) {
            int64_t ncap = cap * 2;
            char *nb = (char *)zap_buf(ncap);
            memcpy(nb, buf, (size_t)got);
            buf = nb;
            cap = ncap;
        }
        ssize_t r = read(fd, buf + got, (size_t)(cap - got));
        if (r < 0) {
            if (errno == EINTR) {
                continue;
            }
            zap_sys_fail("fs.read_file", strerror(errno));
        }
        if (r == 0) {
            break;
        }
        got += (int64_t)r;
    }
    return zap_str_new(buf, got);
}

/* fs.read_file(path) -> str: the whole file. */
void *zap_fs_read_file(void *path_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_RDONLY | ZAP_O_BINARY);
    if (fd < 0) {
        zap_sys_fail("fs.read_file", strerror(errno));
    }
    void *s = zap_read_all_fd(fd);
    close(fd);
    return s;
}

/* fs.write_file(path, data): create/truncate and write the whole string. */
void zap_fs_write_file(void *path_, void *data_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_WRONLY | O_CREAT | O_TRUNC | ZAP_O_BINARY,
                  0644);
    if (fd < 0) {
        zap_sys_fail("fs.write_file", strerror(errno));
    }
    (void)zap_io_write((int64_t)fd, data_);
    close(fd);
}

/* fs.exists(path) -> bool. */
int64_t zap_fs_exists(void *path_) {
    struct stat st;
    return stat(zap_sys_cstr((ZapStr *)path_), &st) == 0 ? 1 : 0;
}

/* fs.is_dir(path) -> bool. */
int64_t zap_fs_is_dir(void *path_) {
    struct stat st;
    if (stat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        return 0;
    }
    return S_ISDIR(st.st_mode) ? 1 : 0;
}

/* fs.is_file(path) -> bool. */
int64_t zap_fs_is_file(void *path_) {
    struct stat st;
    if (stat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        return 0;
    }
    return S_ISREG(st.st_mode) ? 1 : 0;
}

/* fs.size(path) -> int64: file size in bytes (raises if it does not exist). */
int64_t zap_fs_size(void *path_) {
    struct stat st;
    if (stat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        zap_sys_fail("fs.size", strerror(errno));
    }
    return (int64_t)st.st_size;
}

/* The temp directory: $TMPDIR else /tmp on POSIX (unchanged); $TMP / $TEMP else
 * the Windows default on Windows — the Python tempfile search order per platform.
 * '/' works as a separator on Windows too, so the "%s/zap..." templates below are
 * unchanged across platforms. */
static const char *zap_tempdir(void) {
#if defined(ZAP_PLATFORM_WINDOWS)
    const char *t = getenv("TMP");
    if (t == NULL || *t == '\0') {
        t = getenv("TEMP");
    }
    if (t == NULL || *t == '\0') {
        t = "C:/Windows/Temp";
    }
    return t;
#else
    const char *t = getenv("TMPDIR");
    if (t == NULL || *t == '\0') {
        t = "/tmp";
    }
    return t;
#endif
}

#if defined(ZAP_PLATFORM_WINDOWS)
/* Fill the trailing "XXXXXX" of `tmpl` in place with a pseudo-unique suffix. The
 * replacements for POSIX mkstemp/mkdtemp, which MinGW-w64 does not provide; the
 * O_EXCL / EEXIST retry below is what makes them atomic-enough for temp use. */
static void zap_win_fill_template(char *tmpl) {
    static const char cs[] = "abcdefghijklmnopqrstuvwxyz0123456789";
    size_t len = strlen(tmpl);
    static volatile LONG seq = 0;
    unsigned long long r = (unsigned long long)GetCurrentProcessId();
    r ^= (unsigned long long)GetTickCount64();
    r ^= (unsigned long long)InterlockedIncrement(&seq) * 2654435761ULL;
    for (int i = 0; i < 6 && len >= 6; i++) {
        tmpl[len - 6 + i] = cs[r % 36];
        r = r / 36 + 0x9e3779b97f4a7c15ULL;
    }
}

/* mkstemp replacement: create+open a unique file (O_EXCL), returning an fd. */
static int zap_win_mkstemp(char *tmpl) {
    for (int attempt = 0; attempt < 256; attempt++) {
        zap_win_fill_template(tmpl);
        int fd = _open(tmpl, _O_RDWR | _O_CREAT | _O_EXCL | _O_BINARY,
                       _S_IREAD | _S_IWRITE);
        if (fd >= 0) {
            return fd;
        }
        if (errno != EEXIST) {
            return -1;
        }
    }
    return -1;
}

/* mkdtemp replacement: create a unique directory, returning tmpl or NULL. */
static char *zap_win_mkdtemp(char *tmpl) {
    for (int attempt = 0; attempt < 256; attempt++) {
        zap_win_fill_template(tmpl);
        if (_mkdir(tmpl) == 0) {
            return tmpl;
        }
        if (errno != EEXIST) {
            return NULL;
        }
    }
    return NULL;
}
#endif /* ZAP_PLATFORM_WINDOWS */

/* fs.mkdtemp() -> str: create a unique temp directory (under the platform temp
 * dir) and return its path. Powers hermetic temp-dir workflows (and the goldens). */
void *zap_fs_mkdtemp(void) {
    char tmpl[4096];
    snprintf(tmpl, sizeof tmpl, "%s/zapXXXXXX", zap_tempdir());
#if defined(ZAP_PLATFORM_WINDOWS)
    if (zap_win_mkdtemp(tmpl) == NULL) {
#else
    if (mkdtemp(tmpl) == NULL) {
#endif
        zap_sys_fail("fs.mkdtemp", strerror(errno));
    }
    return zap_str_new(tmpl, (int64_t)strlen(tmpl));
}

/* ===========================================================================
 * stdlib: os/fs filesystem gaps (ticket #968, D-0211)
 * ---------------------------------------------------------------------------
 * Metadata (the prelude `Stat` value struct + `mtime`), mutation (rename/move/
 * makedirs/copy/rmtree and binary read/write/append), environment enumeration,
 * and the path helpers (abspath/realpath/relpath/normpath/isabs/expanduser/sep).
 * Traversal (`walk`/`scandir`), matching (`glob`/`fnmatch`), `copytree`, and the
 * tuple-returning `path_split`/`path_splitext` are documented follow-ups. All
 * `cstr`/`sys_fail` conventions and the `/`-separator policy match the existing
 * os/fs surface (#97).
 * =========================================================================== */

/* fs.stat getters (#968): each runs stat()/lstat() and returns one field, the
 * `KeyInfo` pattern — re-stat per field is a cheap syscall and keeps the struct
 * build uniform. Every getter raises (catchable) if the path cannot be stat'd,
 * so an eager `fs.stat` surfaces ENOENT at the call. `is_symlink` uses lstat
 * (the others follow symlinks, like Python `os.stat`). */
static void zap_fs_do_stat(void *path_, struct stat *st, const char *op) {
    if (stat(zap_sys_cstr((ZapStr *)path_), st) != 0) {
        zap_sys_fail(op, strerror(errno));
    }
}
int64_t zap_fs_stat_size(void *path_) {
    struct stat st;
    zap_fs_do_stat(path_, &st, "fs.stat");
    return (int64_t)st.st_size;
}
int64_t zap_fs_stat_mtime(void *path_) {
    struct stat st;
    zap_fs_do_stat(path_, &st, "fs.stat");
    return (int64_t)st.st_mtime;
}
int64_t zap_fs_stat_ctime(void *path_) {
    struct stat st;
    zap_fs_do_stat(path_, &st, "fs.stat");
    return (int64_t)st.st_ctime;
}
int64_t zap_fs_stat_mode(void *path_) {
    struct stat st;
    zap_fs_do_stat(path_, &st, "fs.stat");
    return (int64_t)st.st_mode;
}
int64_t zap_fs_stat_is_dir(void *path_) {
    struct stat st;
    zap_fs_do_stat(path_, &st, "fs.stat");
    return S_ISDIR(st.st_mode) ? 1 : 0;
}
int64_t zap_fs_stat_is_file(void *path_) {
    struct stat st;
    zap_fs_do_stat(path_, &st, "fs.stat");
    return S_ISREG(st.st_mode) ? 1 : 0;
}
int64_t zap_fs_stat_is_symlink(void *path_) {
#if defined(ZAP_PLATFORM_WINDOWS)
    /* no lstat on the MSVC CRT; a symlink test is a follow-up — report false. */
    (void)path_;
    return 0;
#else
    struct stat st;
    if (lstat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        zap_sys_fail("fs.stat", strerror(errno));
    }
    return S_ISLNK(st.st_mode) ? 1 : 0;
#endif
}

/* fs.mtime(path) -> int64 (seconds since the epoch). */
int64_t zap_fs_mtime(void *path_) { return zap_fs_stat_mtime(path_); }

/* fs.rename(src, dst): atomic within a filesystem (POSIX rename). */
void zap_fs_rename(void *src_, void *dst_) {
    char *src = zap_sys_cstr((ZapStr *)src_);
    char *dst = zap_sys_cstr((ZapStr *)dst_);
    if (rename(src, dst) != 0) {
        zap_sys_fail("fs.rename", strerror(errno));
    }
}

/* Copy a regular file `src` -> `dst` (create/truncate, 0644). Shared by
 * `fs.copy` and `fs.move`'s cross-device fallback. Returns 0 on success, or sets
 * errno and returns -1. */
static int zap_fs_copy_file(const char *src, const char *dst) {
    int in = open(src, O_RDONLY | ZAP_O_BINARY);
    if (in < 0) {
        return -1;
    }
    int out = open(dst, O_WRONLY | O_CREAT | O_TRUNC | ZAP_O_BINARY, 0644);
    if (out < 0) {
        int e = errno;
        close(in);
        errno = e;
        return -1;
    }
    char buf[65536];
    ssize_t r;
    int rc = 0;
    while ((r = read(in, buf, sizeof buf)) > 0) {
        ssize_t w = 0;
        while (w < r) {
            ssize_t k = write(out, buf + w, (size_t)(r - w));
            if (k < 0) {
                rc = -1;
                break;
            }
            w += k;
        }
        if (rc != 0) {
            break;
        }
    }
    if (r < 0) {
        rc = -1;
    }
    int e = errno;
    close(in);
    close(out);
    errno = e;
    return rc;
}

/* fs.copy(src, dst): copy a regular file's contents. */
void zap_fs_copy(void *src_, void *dst_) {
    char *src = zap_sys_cstr((ZapStr *)src_);
    char *dst = zap_sys_cstr((ZapStr *)dst_);
    if (zap_fs_copy_file(src, dst) != 0) {
        zap_sys_fail("fs.copy", strerror(errno));
    }
}

/* fs.move(src, dst): rename, falling back to copy+unlink across filesystems
 * (EXDEV) for a regular file (Python `shutil.move`, file case). */
void zap_fs_move(void *src_, void *dst_) {
    char *src = zap_sys_cstr((ZapStr *)src_);
    char *dst = zap_sys_cstr((ZapStr *)dst_);
    if (rename(src, dst) == 0) {
        return;
    }
#if !defined(ZAP_PLATFORM_WINDOWS)
    if (errno == EXDEV) {
        if (zap_fs_copy_file(src, dst) == 0 && unlink(src) == 0) {
            return;
        }
    }
#endif
    zap_sys_fail("fs.move", strerror(errno));
}

/* fs.makedirs(path): create `path` and every missing parent (Python
 * `os.makedirs`, Go `MkdirAll`). An already-existing directory is not an error;
 * an existing non-directory component is. */
void zap_fs_makedirs(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    int64_t n = p->len;
    if (n == 0) {
        zap_sys_fail("fs.makedirs", "empty path");
    }
    char *buf = (char *)zap_buf(n + 1);
    memcpy(buf, p->data, (size_t)n);
    buf[n] = '\0';
    /* create each prefix ending at a '/', then the whole path. */
    for (int64_t i = 1; i <= n; i++) {
        if (i == n || buf[i] == '/') {
            char saved = buf[i];
            buf[i] = '\0';
            if (buf[0] != '\0' && !(i == 1 && buf[0] == '/')) {
#if defined(ZAP_PLATFORM_WINDOWS)
                int rc = _mkdir(buf);
#else
                int rc = mkdir(buf, 0777);
#endif
                if (rc != 0 && errno != EEXIST) {
                    zap_sys_fail("fs.makedirs", strerror(errno));
                }
            }
            buf[i] = saved;
        }
    }
}

/* fs.rmtree(path): recursively remove a directory tree (Python `shutil.rmtree`).
 * A plain file is unlinked. */
static void zap_fs_rmtree_cstr(const char *path) {
    struct stat st;
    /* lstat (don't follow a symlink to a dir — unlink it) on POSIX; the MSVC CRT
     * has no lstat, so Windows uses stat (symlink-dir edge case is a follow-up,
     * like `fs.stat`'s is_symlink). */
#if defined(ZAP_PLATFORM_WINDOWS)
    if (stat(path, &st) != 0) {
#else
    if (lstat(path, &st) != 0) {
#endif
        zap_sys_fail("fs.rmtree", strerror(errno));
    }
    if (!S_ISDIR(st.st_mode)) {
        if (unlink(path) != 0) {
            zap_sys_fail("fs.rmtree", strerror(errno));
        }
        return;
    }
    DIR *d = opendir(path);
    if (d == NULL) {
        zap_sys_fail("fs.rmtree", strerror(errno));
    }
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
            continue;
        }
        size_t need = strlen(path) + 1 + strlen(e->d_name) + 1;
        char *child = (char *)malloc(need);
        if (child == NULL) {
            closedir(d);
            zap_sys_fail("fs.rmtree", "out of memory");
        }
        snprintf(child, need, "%s/%s", path, e->d_name);
        zap_fs_rmtree_cstr(child);
        free(child);
    }
    closedir(d);
    if (rmdir(path) != 0) {
        zap_sys_fail("fs.rmtree", strerror(errno));
    }
}
void zap_fs_rmtree(void *path_) {
    zap_fs_rmtree_cstr(zap_sys_cstr((ZapStr *)path_));
}

/* --- directory traversal: scandir / walk / copytree (#1032, D-0224) --------- *
 * All three classify each entry as dir(1) / file(2) / other(0). The classifier
 * prefers readdir's `d_type` (no per-entry stat) and falls back to stat() only
 * when the filesystem reports DT_UNKNOWN or a symlink (so `is_dir` follows a
 * symlink, like CPython's DirEntry default). Entries are returned SORTED by name
 * (Go `filepath.WalkDir` order) so results are deterministic across platforms —
 * a deliberate difference from CPython's unordered `os.scandir`/`os.walk`. */

/* 1 = directory, 2 = regular file, 0 = anything else. `dirpath` is the parent,
 * needed to build the full path for the stat fallback. */
static int zap_fs_entry_kind(const char *dirpath, struct dirent *e) {
#if defined(DT_DIR)
    if (e->d_type == DT_DIR) {
        return 1;
    }
    if (e->d_type == DT_REG) {
        return 2;
    }
    if (e->d_type != DT_UNKNOWN && e->d_type != DT_LNK) {
        return 0;
    }
    /* DT_UNKNOWN / DT_LNK: stat the full path (follow symlinks). */
#endif
    {
        size_t need = strlen(dirpath) + 1 + strlen(e->d_name) + 1;
        char *full = (char *)malloc(need);
        if (full == NULL) {
            return 0;
        }
        snprintf(full, need, "%s/%s", dirpath, e->d_name);
        struct stat st;
        int ok = stat(full, &st) == 0;
        free(full);
        if (!ok) {
            return 0;
        }
        if (S_ISDIR(st.st_mode)) {
            return 1;
        }
        if (S_ISREG(st.st_mode)) {
            return 2;
        }
        return 0;
    }
}

/* One directory entry: an owned NUL-terminated name plus its kind. */
typedef struct {
    char *name;
    int kind;
} ZapDirRec;

static int zap_dirrec_cmp(const void *a, const void *b) {
    return strcmp(((const ZapDirRec *)a)->name, ((const ZapDirRec *)b)->name);
}

/* Read `dirpath` into a malloc'd array of (name, kind), sorted by name, skipping
 * "."/"..". Returns 0 and sets *out/*n_out on success (caller frees each `name`
 * and the array); -1 if the directory cannot be opened. Raises (never returns)
 * only on out-of-memory, after closing the handle. */
static int zap_fs_read_dir_sorted(const char *dirpath, const char *op, ZapDirRec **out,
                                  int64_t *n_out) {
    DIR *d = opendir(dirpath);
    if (d == NULL) {
        return -1;
    }
    ZapDirRec *arr = NULL;
    int64_t n = 0, cap = 0;
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
            continue;
        }
        if (n == cap) {
            cap = cap ? cap * 2 : 16;
            ZapDirRec *grown = (ZapDirRec *)realloc(arr, (size_t)cap * sizeof(ZapDirRec));
            if (grown == NULL) {
                for (int64_t k = 0; k < n; k++) {
                    free(arr[k].name);
                }
                free(arr);
                closedir(d);
                zap_sys_fail(op, "out of memory");
            }
            arr = grown;
        }
        size_t nl = strlen(e->d_name);
        char *nm = (char *)malloc(nl + 1);
        if (nm == NULL) {
            for (int64_t k = 0; k < n; k++) {
                free(arr[k].name);
            }
            free(arr);
            closedir(d);
            zap_sys_fail(op, "out of memory");
        }
        memcpy(nm, e->d_name, nl + 1);
        arr[n].name = nm;
        arr[n].kind = zap_fs_entry_kind(dirpath, e);
        n++;
    }
    closedir(d);
    qsort(arr, (size_t)n, sizeof(ZapDirRec), zap_dirrec_cmp);
    *out = arr;
    *n_out = n;
    return 0;
}

static void zap_fs_dirrecs_free(ZapDirRec *arr, int64_t n) {
    for (int64_t i = 0; i < n; i++) {
        free(arr[i].name);
    }
    free(arr);
}

/* fs.copytree(src, dst): recursively copy the tree rooted at `src` into `dst`
 * (Python `shutil.copytree`). `dst` and missing parents are created; an existing
 * `dst` is merged into (the `dirs_exist_ok=True` behaviour). Regular files are
 * copied with `zap_fs_copy_file`; a symlink is copied as the content of its
 * target (the file-copy policy of #968). */
static void zap_fs_copytree_cstr(const char *src, const char *dst) {
    struct stat st;
    if (stat(src, &st) != 0) {
        zap_sys_fail("fs.copytree", strerror(errno));
    }
    if (!S_ISDIR(st.st_mode)) {
        if (zap_fs_copy_file(src, dst) != 0) {
            zap_sys_fail("fs.copytree", strerror(errno));
        }
        return;
    }
#if defined(ZAP_PLATFORM_WINDOWS)
    int rc = _mkdir(dst);
#else
    int rc = mkdir(dst, 0777);
#endif
    if (rc != 0 && errno != EEXIST) {
        zap_sys_fail("fs.copytree", strerror(errno));
    }
    ZapDirRec *recs = NULL;
    int64_t n = 0;
    if (zap_fs_read_dir_sorted(src, "fs.copytree", &recs, &n) != 0) {
        zap_sys_fail("fs.copytree", strerror(errno));
    }
    for (int64_t i = 0; i < n; i++) {
        size_t sneed = strlen(src) + 1 + strlen(recs[i].name) + 1;
        size_t dneed = strlen(dst) + 1 + strlen(recs[i].name) + 1;
        char *cs = (char *)malloc(sneed);
        char *cd = (char *)malloc(dneed);
        if (cs == NULL || cd == NULL) {
            free(cs);
            free(cd);
            zap_fs_dirrecs_free(recs, n);
            zap_sys_fail("fs.copytree", "out of memory");
        }
        snprintf(cs, sneed, "%s/%s", src, recs[i].name);
        snprintf(cd, dneed, "%s/%s", dst, recs[i].name);
        zap_fs_copytree_cstr(cs, cd);
        free(cs);
        free(cd);
    }
    zap_fs_dirrecs_free(recs, n);
}
void zap_fs_copytree(void *src_, void *dst_) {
    zap_fs_copytree_cstr(zap_sys_cstr((ZapStr *)src_), zap_sys_cstr((ZapStr *)dst_));
}

/* fs.scandir(path) -> handle (Python `os.scandir`): read one directory level
 * into a handle carrying a parallel list of names (str) and kinds (i64), which
 * the lowering enumerates into a `list[DirEntry]`. `is_dir`/`is_file` come from
 * `d_type` where the filesystem provides it — no per-entry stat in the common
 * case. */
typedef struct {
    void *names; /* ZapList[str] */
    void *kinds; /* ZapList[i64]: 1 dir / 2 file / 0 other */
} ZapScan;
static const int32_t zap_off_scan[] = {
    (int32_t)offsetof(ZapScan, names),
    (int32_t)offsetof(ZapScan, kinds),
};
static const ZapTypeDesc zap_desc_scan = {(int64_t)sizeof(ZapScan), ZAP_TD_PRECISE, 2,
                                          zap_off_scan};

void *zap_fs_scandir(void *path_) {
    char *path = zap_sys_cstr((ZapStr *)path_);
    ZapDirRec *recs = NULL;
    int64_t n = 0;
    if (zap_fs_read_dir_sorted(path, "fs.scandir", &recs, &n) != 0) {
        zap_sys_fail("fs.scandir", strerror(errno));
    }
    void *names = zap_list_new();
    void *kinds = zap_list_new();
    for (int64_t i = 0; i < n; i++) {
        zap_list_push_ptr(names, zap_str_new(recs[i].name, (int64_t)strlen(recs[i].name)));
        zap_list_push_i64(kinds, (int64_t)recs[i].kind);
    }
    zap_fs_dirrecs_free(recs, n);
    ZapScan *s = (ZapScan *)zap_obj(&zap_desc_scan);
    s->names = names;
    s->kinds = kinds;
    return s;
}
int64_t zap_fs_scandir_count(void *h) { return zap_list_len(((ZapScan *)h)->names); }
void *zap_fs_scandir_name(void *h, int64_t i) { return zap_list_get_ptr(((ZapScan *)h)->names, i); }
int64_t zap_fs_scandir_is_dir(void *h, int64_t i) {
    return zap_list_get_i64(((ZapScan *)h)->kinds, i) == 1;
}
int64_t zap_fs_scandir_is_file(void *h, int64_t i) {
    return zap_list_get_i64(((ZapScan *)h)->kinds, i) == 2;
}

/* fs.walk(root) -> handle (Python `os.walk`, top-down): a parallel list per
 * visited directory of its path (str), its subdirectory names (list[str]) and
 * its file names (list[str]), which the lowering enumerates into a
 * `list[(str, list[str], list[str])]`. The whole tree is materialised eagerly.
 * An unreadable SUBdirectory is skipped silently (os.walk's default `onerror`);
 * an unreadable ROOT raises. Recursion follows the sorted subdir order. */
typedef struct {
    void *dirpaths; /* ZapList[str] */
    void *dirs;     /* ZapList[list[str]] */
    void *files;    /* ZapList[list[str]] */
} ZapWalk;
static const int32_t zap_off_walk[] = {
    (int32_t)offsetof(ZapWalk, dirpaths),
    (int32_t)offsetof(ZapWalk, dirs),
    (int32_t)offsetof(ZapWalk, files),
};
static const ZapTypeDesc zap_desc_walk = {(int64_t)sizeof(ZapWalk), ZAP_TD_PRECISE, 3,
                                          zap_off_walk};

static void zap_fs_walk_rec(const char *dirpath, void *dirpaths, void *dirs, void *files) {
    ZapDirRec *recs = NULL;
    int64_t n = 0;
    if (zap_fs_read_dir_sorted(dirpath, "fs.walk", &recs, &n) != 0) {
        return; /* unreadable subdirectory: skip (os.walk default) */
    }
    void *dnames = zap_list_new();
    void *fnames = zap_list_new();
    for (int64_t i = 0; i < n; i++) {
        void *nm = zap_str_new(recs[i].name, (int64_t)strlen(recs[i].name));
        if (recs[i].kind == 1) {
            zap_list_push_ptr(dnames, nm);
        } else {
            zap_list_push_ptr(fnames, nm);
        }
    }
    /* Record THIS directory first (top-down order). */
    zap_list_push_ptr(dirpaths, zap_str_new(dirpath, (int64_t)strlen(dirpath)));
    zap_list_push_ptr(dirs, dnames);
    zap_list_push_ptr(files, fnames);
    /* Then recurse into each subdirectory, in sorted order. */
    for (int64_t i = 0; i < n; i++) {
        if (recs[i].kind != 1) {
            continue;
        }
        size_t need = strlen(dirpath) + 1 + strlen(recs[i].name) + 1;
        char *child = (char *)malloc(need);
        if (child == NULL) {
            zap_fs_dirrecs_free(recs, n);
            zap_sys_fail("fs.walk", "out of memory");
        }
        snprintf(child, need, "%s/%s", dirpath, recs[i].name);
        zap_fs_walk_rec(child, dirpaths, dirs, files);
        free(child);
    }
    zap_fs_dirrecs_free(recs, n);
}
void *zap_fs_walk(void *root_) {
    char *root = zap_sys_cstr((ZapStr *)root_);
    /* The root must be a readable directory — surface a typo/permission error
     * at the call rather than silently yielding nothing. */
    DIR *probe = opendir(root);
    if (probe == NULL) {
        zap_sys_fail("fs.walk", strerror(errno));
    }
    closedir(probe);
    void *dirpaths = zap_list_new();
    void *dirs = zap_list_new();
    void *files = zap_list_new();
    zap_fs_walk_rec(root, dirpaths, dirs, files);
    ZapWalk *w = (ZapWalk *)zap_obj(&zap_desc_walk);
    w->dirpaths = dirpaths;
    w->dirs = dirs;
    w->files = files;
    return w;
}
int64_t zap_fs_walk_count(void *h) { return zap_list_len(((ZapWalk *)h)->dirpaths); }
void *zap_fs_walk_dirpath(void *h, int64_t i) {
    return zap_list_get_ptr(((ZapWalk *)h)->dirpaths, i);
}
void *zap_fs_walk_dirs(void *h, int64_t i) { return zap_list_get_ptr(((ZapWalk *)h)->dirs, i); }
void *zap_fs_walk_files(void *h, int64_t i) { return zap_list_get_ptr(((ZapWalk *)h)->files, i); }

/* fs.read_bytes(path) -> bytes: the whole file as raw bytes (physically a
 * `ZapStr` buffer, typed `bytes` by the lowerer). */
void *zap_fs_read_bytes(void *path_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_RDONLY | ZAP_O_BINARY);
    if (fd < 0) {
        zap_sys_fail("fs.read_bytes", strerror(errno));
    }
    void *s = zap_read_all_fd(fd);
    close(fd);
    return s;
}

/* fs.write_bytes(path, data): create/truncate and write all of `data` (bytes). */
void zap_fs_write_bytes(void *path_, void *data_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_WRONLY | O_CREAT | O_TRUNC | ZAP_O_BINARY,
                  0644);
    if (fd < 0) {
        zap_sys_fail("fs.write_bytes", strerror(errno));
    }
    (void)zap_io_write((int64_t)fd, data_);
    close(fd);
}

/* fs.append(path, data): create if absent, append `data` at the end. */
void zap_fs_append(void *path_, void *data_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_WRONLY | O_CREAT | O_APPEND | ZAP_O_BINARY,
                  0644);
    if (fd < 0) {
        zap_sys_fail("fs.append", strerror(errno));
    }
    (void)zap_io_write((int64_t)fd, data_);
    close(fd);
}

/* os.environ() -> dict[str, str]: every environment variable, split on the
 * first '='. */
void *zap_os_environ(void) {
    void *d = zap_dict_new();
    if (environ == NULL) {
        return d;
    }
    for (char **ep = environ; *ep != NULL; ep++) {
        const char *entry = *ep;
        const char *eq = strchr(entry, '=');
        if (eq == NULL) {
            continue;
        }
        void *k = zap_str_new(entry, (int64_t)(eq - entry));
        void *v = zap_str_new(eq + 1, (int64_t)strlen(eq + 1));
        zap_dict_set_str_ptr(d, k, v);
    }
    return d;
}

/* os.sep() -> str: Zap's one portable, Python-readable separator ('/'), on every
 * platform (every Windows API accepts it). */
void *zap_os_sep(void) { return zap_str_new("/", 1); }

/* os.path_isabs(path) -> bool. */
int64_t zap_os_path_isabs(void *path_) {
    return zap_path_is_abs((const ZapStr *)path_) ? 1 : 0;
}

/* os.path_expanduser(path): a leading "~" (alone or before a '/') expands to
 * $HOME; "~user" and a HOME-less environment are left unchanged. */
void *zap_os_path_expanduser(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    if (p->len == 0 || p->data[0] != '~') {
        return zap_str_new(p->data, p->len);
    }
    if (p->len > 1 && p->data[1] != '/') {
        return zap_str_new(p->data, p->len); /* "~user" — unsupported, verbatim */
    }
    const char *home = getenv("HOME");
    if (home == NULL) {
        return zap_str_new(p->data, p->len);
    }
    int64_t hlen = (int64_t)strlen(home);
    int64_t rest = p->len - 1; /* everything after '~' (includes the '/') */
    int64_t total = hlen + rest;
    char *buf = (char *)zap_buf(total > 0 ? total : 1);
    memcpy(buf, home, (size_t)hlen);
    if (rest > 0) {
        memcpy(buf + hlen, p->data + 1, (size_t)rest);
    }
    return zap_str_adopt(buf, total);
}

/* Normalize a path in place over `in`/`n` into `out` (collapse '.', resolve
 * '..' lexically, squeeze '//'), returning the output length. Leading '/' is
 * preserved; an empty non-absolute result becomes ".". Pure lexical (Python
 * `os.path.normpath`); it does NOT touch the filesystem. */
static int64_t zap_path_norm_bytes(const char *in, int64_t n, char *out) {
    int abs = (n > 0 && in[0] == '/');
    /* component offsets into `out`, tracked as a stack over the output buffer. */
    int64_t o = 0;
    if (abs) {
        out[o++] = '/';
    }
    int64_t seg_count = 0;
    int64_t i = 0;
    while (i < n) {
        while (i < n && in[i] == '/') {
            i++;
        }
        int64_t s = i;
        while (i < n && in[i] != '/') {
            i++;
        }
        int64_t len = i - s;
        if (len == 0) {
            continue;
        }
        if (len == 1 && in[s] == '.') {
            continue;
        }
        if (len == 2 && in[s] == '.' && in[s + 1] == '.') {
            /* pop one segment unless at root / a leading ".." run on a relative
             * path (which must be preserved). */
            if (seg_count > 0) {
                /* `o` points just past the last output segment's content (we
                 * only ever write a separator BEFORE content). Find that
                 * segment's start `j`, and if it is not itself a preserved
                 * "..", pop it AND the separator that preceded it. */
                int64_t j = o;
                while (j > (abs ? 1 : 0) && out[j - 1] != '/') {
                    j--;
                }
                int is_dotdot = (o - j == 2 && out[j] == '.' && out[j + 1] == '.');
                if (!is_dotdot) {
                    o = j;
                    if (o > (abs ? 1 : 0) && out[o - 1] == '/') {
                        o--; /* drop the separator before the popped segment */
                    }
                    seg_count--;
                    continue;
                }
            }
            if (abs) {
                continue; /* ".." at root is a no-op */
            }
            /* relative: keep the ".." */
        }
        if (o > (abs ? 1 : 0)) {
            out[o++] = '/';
        }
        memcpy(out + o, in + s, (size_t)len);
        o += len;
        seg_count++;
    }
    if (o == 0) {
        out[o++] = '.';
    }
    return o;
}

/* os.path_normpath(path). */
void *zap_os_path_normpath(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    char *out = (char *)zap_buf(p->len + 2);
    int64_t len = zap_path_norm_bytes(p->data, p->len, out);
    return zap_str_adopt(out, len);
}

/* os.path_abspath(path): normalize against the current directory when relative
 * (Python `os.path.abspath`). */
void *zap_os_path_abspath(void *path_) {
    const ZapStr *p = (const ZapStr *)path_;
    if (zap_path_is_abs(p)) {
        return zap_os_path_normpath(path_);
    }
    char cwd[4096];
    if (getcwd(cwd, sizeof cwd) == NULL) {
        zap_sys_fail("os.path_abspath", strerror(errno));
    }
    int64_t clen = (int64_t)strlen(cwd);
    int64_t joined = clen + 1 + p->len;
    char *buf = (char *)zap_buf(joined > 0 ? joined : 1);
    memcpy(buf, cwd, (size_t)clen);
    buf[clen] = '/';
    memcpy(buf + clen + 1, p->data, (size_t)p->len);
    char *out = (char *)zap_buf(joined + 2);
    int64_t len = zap_path_norm_bytes(buf, joined, out);
    return zap_str_adopt(out, len);
}

/* os.path_realpath(path): canonical absolute path with symlinks resolved
 * (libc realpath). Raises if the path cannot be resolved. */
void *zap_os_path_realpath(void *path_) {
#if defined(ZAP_PLATFORM_WINDOWS)
    char buf[4096];
    if (_fullpath(buf, zap_sys_cstr((ZapStr *)path_), sizeof buf) == NULL) {
        zap_sys_fail("os.path_realpath", "cannot resolve path");
    }
    return zap_str_new(buf, (int64_t)strlen(buf));
#else
    char buf[4096];
    if (realpath(zap_sys_cstr((ZapStr *)path_), buf) == NULL) {
        zap_sys_fail("os.path_realpath", strerror(errno));
    }
    return zap_str_new(buf, (int64_t)strlen(buf));
#endif
}

/* os.path_relpath(path, start): the relative path from `start` to `path`
 * (Python `os.path.relpath`), computed lexically on the normalized absolute
 * forms. Returns "." when they are equal. */
void *zap_os_path_relpath(void *path_, void *start_) {
    void *pa = zap_os_path_abspath(path_);
    void *sa = zap_os_path_abspath(start_);
    const ZapStr *P = (const ZapStr *)pa;
    const ZapStr *S = (const ZapStr *)sa;
    /* find the common directory prefix at a '/' boundary. */
    int64_t common = 0, i = 0;
    int64_t m = P->len < S->len ? P->len : S->len;
    int64_t last_sep = 0;
    while (i < m && P->data[i] == S->data[i]) {
        if (P->data[i] == '/') {
            last_sep = i;
        }
        i++;
    }
    if (i == P->len && i == S->len) {
        return zap_str_new(".", 1); /* identical */
    }
    /* a full-component match where one ends exactly at the other's '/'. */
    if ((i == P->len && S->data[i] == '/') || (i == S->len && P->data[i] == '/')) {
        last_sep = i;
    }
    common = last_sep;
    /* count the components remaining in `start` after the common prefix -> "..". */
    int64_t ups = 0;
    for (int64_t k = common; k < S->len; k++) {
        if (S->data[k] == '/' && k + 1 < S->len) {
            ups++;
        }
    }
    /* tail of `path` after the common prefix (skip a leading '/'). */
    int64_t tstart = common;
    while (tstart < P->len && P->data[tstart] == '/') {
        tstart++;
    }
    int64_t tlen = P->len - tstart;
    int64_t cap = ups * 3 + (tlen > 0 ? tlen : 1) + 2;
    char *buf = (char *)zap_buf(cap);
    int64_t o = 0;
    for (int64_t u = 0; u < ups; u++) {
        buf[o++] = '.';
        buf[o++] = '.';
        if (u + 1 < ups || tlen > 0) {
            buf[o++] = '/';
        }
    }
    if (tlen > 0) {
        memcpy(buf + o, P->data + tstart, (size_t)tlen);
        o += tlen;
    }
    if (o == 0) {
        buf[o++] = '.';
    }
    return zap_str_adopt(buf, o);
}

/* --- process: subprocess spawn + capture ------------------------------------ */

/* The result of process.run: exit code + captured stdout/stderr. A precise GC
 * object (two trailing pointer fields) so the two ZapStr captures stay live;
 * the lowerer reads the three fields back into a (int64, str, str) tuple. */
typedef struct {
    int64_t code;
    ZapStr *out;
    ZapStr *err;
} ZapProc;

static const int32_t zap_off_proc[] = {
    (int32_t)offsetof(ZapProc, out),
    (int32_t)offsetof(ZapProc, err),
};
static const ZapTypeDesc zap_desc_proc = {(int64_t)sizeof(ZapProc), ZAP_TD_PRECISE,
                                          2, zap_off_proc};

#if defined(ZAP_PLATFORM_WINDOWS)
/* Build a single Windows command line from an argv vector, applying the MSVCRT
 * `CommandLineToArgvW`/`parse_cmdline` quoting rules so each argument round-trips
 * through the child's argv exactly (quote args with spaces/tabs/quotes; escape
 * embedded quotes with backslashes; double the backslashes that precede a quote).
 * Returns a malloc'd NUL-terminated string the caller frees, or NULL on OOM. */
static char *zap_win_build_cmdline(char **argv, int64_t n) {
    size_t cap = 16;
    for (int64_t i = 0; i < n; i++) {
        cap += strlen(argv[i]) * 2 + 4; /* worst case: every char escaped + quotes */
    }
    char *out = (char *)malloc(cap);
    if (!out) {
        return NULL;
    }
    size_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        if (i > 0) {
            out[o++] = ' ';
        }
        const char *a = argv[i];
        int needq = (*a == '\0');
        for (const char *p = a; *p; p++) {
            if (*p == ' ' || *p == '\t' || *p == '"') {
                needq = 1;
                break;
            }
        }
        if (!needq) {
            for (const char *p = a; *p; p++) {
                out[o++] = *p;
            }
            continue;
        }
        out[o++] = '"';
        for (const char *p = a;; p++) {
            size_t nbs = 0;
            while (*p == '\\') {
                nbs++;
                p++;
            }
            if (*p == '\0') {
                for (size_t k = 0; k < nbs * 2; k++) {
                    out[o++] = '\\'; /* all backslashes before the closing quote */
                }
                break;
            } else if (*p == '"') {
                for (size_t k = 0; k < nbs * 2 + 1; k++) {
                    out[o++] = '\\'; /* escape the embedded quote */
                }
                out[o++] = '"';
            } else {
                for (size_t k = 0; k < nbs; k++) {
                    out[o++] = '\\';
                }
                out[o++] = *p;
            }
        }
        out[o++] = '"';
    }
    out[o] = '\0';
    return out;
}
#endif /* ZAP_PLATFORM_WINDOWS */

/* `cwd=` support (#976): posix_spawn gains a working directory via the
 * `posix_spawn_file_actions_addchdir_np` extension (glibc >= 2.29, macOS >=
 * 10.15 — both CI targets). Where it is unavailable a non-empty `cwd` is a
 * clean runtime error rather than a silent miss. Windows takes the directory
 * through `CreateProcess`'s `lpCurrentDirectory` and needs no feature gate. */
#if defined(__GLIBC__) && (__GLIBC__ > 2 || (__GLIBC__ == 2 && __GLIBC_MINOR__ >= 29))
#define ZAP_HAVE_SPAWN_CHDIR 1
#elif defined(__APPLE__)
#define ZAP_HAVE_SPAWN_CHDIR 1
#endif

#if defined(ZAP_PLATFORM_POSIX)
/* Does the NUL-terminated `environ` entry `e` ("KEY=VALUE") have key == `k`?
 * Keys compare up to the first '='; `k` is a (non-NUL-terminated) ZapStr. */
static int zap_proc_env_key_eq(const char *e, const ZapStr *k) {
    int64_t i = 0;
    for (; i < k->len; i++) {
        if (e[i] == '\0' || e[i] != k->data[i]) {
            return 0;
        }
    }
    return e[i] == '=';
}

/* malloc'd "KEY=VALUE\0" from two ZapStr (the caller frees), or NULL on OOM. */
static char *zap_proc_env_kv(const ZapStr *k, const ZapStr *v) {
    size_t n = (size_t)k->len + 1 + (size_t)v->len + 1;
    char *s = (char *)malloc(n);
    if (!s) {
        return NULL;
    }
    if (k->len > 0) {
        memcpy(s, k->data, (size_t)k->len);
    }
    s[k->len] = '=';
    if (v->len > 0) {
        memcpy(s + k->len + 1, v->data, (size_t)v->len);
    }
    s[k->len + 1 + v->len] = '\0';
    return s;
}

/* Build an augmented child environment (#976): the parent `environ` with the
 * `env` dict's entries overriding same-named variables and adding new ones
 * (Python `subprocess`'s semantics are full-replace; Zap's `env=` is
 * documented as override/augment, D-0237). Returns a malloc'd NULL-terminated
 * `char**` (each string malloc'd) that the caller frees with
 * `zap_proc_free_envp`, or NULL when `env` is empty/NULL (then the caller uses
 * `environ` directly). NULL is also returned on OOM — indistinguishable from
 * "no overrides", which is safe: the child merely inherits the unmodified
 * environment. */
static char **zap_proc_build_envp(const ZapDict *env) {
    if (!env || zap_dict_len((void *)env) == 0) {
        return NULL;
    }
    int64_t pcount = 0;
    while (environ[pcount]) {
        pcount++;
    }
    int64_t cap = pcount + zap_dict_len((void *)env) + 1;
    char **envp = (char **)malloc(sizeof(char *) * (size_t)cap);
    if (!envp) {
        return NULL;
    }
    int64_t o = 0;
    /* Copy each parent entry unless an override shadows its key. */
    for (int64_t i = 0; i < pcount; i++) {
        int shadowed = 0;
        for (int64_t e = 0; e < env->cap; e++) {
            if (zap_d_tags(env)[e] < 2) {
                continue; /* empty / tombstone slot (#840) */
            }
            const ZapStr *k = (const ZapStr *)(intptr_t)zap_d_keys(env)[e];
            if (zap_proc_env_key_eq(environ[i], k)) {
                shadowed = 1;
                break;
            }
        }
        if (!shadowed) {
            char *dup = strdup(environ[i]);
            if (!dup) {
                goto oom;
            }
            envp[o++] = dup;
        }
    }
    /* Append the overrides (KEY=VALUE). */
    for (int64_t e = 0; e < env->cap; e++) {
        if (zap_d_tags(env)[e] < 2) {
            continue;
        }
        const ZapStr *k = (const ZapStr *)(intptr_t)zap_d_keys(env)[e];
        const ZapStr *v = (const ZapStr *)(intptr_t)zap_d_vals(env)[e];
        char *kv = zap_proc_env_kv(k, v);
        if (!kv) {
            goto oom;
        }
        envp[o++] = kv;
    }
    envp[o] = NULL;
    return envp;
oom:
    for (int64_t i = 0; i < o; i++) {
        free(envp[i]);
    }
    free(envp);
    return NULL;
}

static void zap_proc_free_envp(char **envp) {
    if (!envp) {
        return;
    }
    for (int64_t i = 0; envp[i]; i++) {
        free(envp[i]);
    }
    free(envp);
}

/* Wait for `pid`, decoding the exit status (128+signo for a signal death). With
 * `timeout_s > 0` the wait is bounded by a monotonic deadline: on expiry the
 * child is SIGKILLed and reaped, and `*timed_out` is set (the caller raises
 * TimeoutError). Capture goes to temp files (never pipes), so the child can
 * never block on a full pipe while we poll — a plain WNOHANG poll loop suffices
 * and there is no deadlock. Bracketed in enter/leave_blocking so a concurrent
 * stop-the-world GC need not wait on this thread. */
static int64_t zap_proc_wait_timed(pid_t pid, double timeout_s, int *timed_out) {
    *timed_out = 0;
    int status = 0;
    if (timeout_s <= 0.0) {
        pid_t w;
        zap_enter_blocking();
        do {
            w = waitpid(pid, &status, 0);
        } while (w < 0 && errno == EINTR);
        zap_leave_blocking();
        if (w < 0) {
            return -1;
        }
    } else {
        int64_t deadline = np_now_ns() + (int64_t)(timeout_s * 1e9);
        zap_enter_blocking();
        for (;;) {
            pid_t w = waitpid(pid, &status, WNOHANG);
            if (w == pid) {
                break;
            }
            if (w < 0) {
                if (errno == EINTR) {
                    continue;
                }
                zap_leave_blocking();
                return -1;
            }
            /* w == 0: still running. */
            int64_t remaining = deadline - np_now_ns();
            if (remaining <= 0) {
                kill(pid, SIGKILL);
                do {
                    w = waitpid(pid, &status, 0);
                } while (w < 0 && errno == EINTR);
                zap_leave_blocking();
                *timed_out = 1;
                return -1;
            }
            int64_t ms = remaining / 1000000LL;
            if (ms > 20) {
                ms = 20; /* cap the poll granularity at 20ms */
            }
            struct timespec ts;
            ts.tv_sec = 0;
            ts.tv_nsec = (ms > 0 ? ms : 1) * 1000000L;
            nanosleep(&ts, NULL);
        }
        zap_leave_blocking();
    }
    if (WIFEXITED(status)) {
        return (int64_t)WEXITSTATUS(status);
    }
    if (WIFSIGNALED(status)) {
        return 128 + (int64_t)WTERMSIG(status);
    }
    return -1;
}
#endif /* ZAP_PLATFORM_POSIX */

#if defined(ZAP_PLATFORM_WINDOWS)
/* Build a Windows environment block ("K=V\0K=V\0...\0") from the parent block
 * augmented with the `env` dict's overrides/additions (#976). Returns a malloc'd
 * block the caller frees, or NULL when `env` is empty (inherit the parent block)
 * or on OOM (also inherit — safe). */
static char *zap_win_build_env_block(const ZapDict *env) {
    if (!env || zap_dict_len((void *)env) == 0) {
        return NULL;
    }
    char *parent = GetEnvironmentStringsA();
    size_t cap = 1;
    for (const char *p = parent; p && *p;) {
        size_t l = strlen(p);
        cap += l + 1;
        p += l + 1;
    }
    for (int64_t e = 0; e < env->cap; e++) {
        if (zap_d_tags(env)[e] < 2) {
            continue;
        }
        const ZapStr *k = (const ZapStr *)(intptr_t)zap_d_keys(env)[e];
        const ZapStr *v = (const ZapStr *)(intptr_t)zap_d_vals(env)[e];
        cap += (size_t)k->len + 1 + (size_t)v->len + 1;
    }
    cap += 1; /* final terminator */
    char *block = (char *)malloc(cap);
    if (!block) {
        if (parent) {
            FreeEnvironmentStringsA(parent);
        }
        return NULL;
    }
    size_t o = 0;
    for (const char *p = parent; p && *p;) {
        size_t l = strlen(p);
        const char *eq = (const char *)memchr(p, '=', l);
        int shadowed = 0;
        if (eq) {
            int64_t klen = (int64_t)(eq - p);
            for (int64_t e = 0; e < env->cap; e++) {
                if (zap_d_tags(env)[e] < 2) {
                    continue;
                }
                const ZapStr *k = (const ZapStr *)(intptr_t)zap_d_keys(env)[e];
                if (k->len == klen && memcmp(p, k->data, (size_t)klen) == 0) {
                    shadowed = 1;
                    break;
                }
            }
        }
        if (!shadowed) {
            memcpy(block + o, p, l + 1);
            o += l + 1;
        }
        p += l + 1;
    }
    if (parent) {
        FreeEnvironmentStringsA(parent);
    }
    for (int64_t e = 0; e < env->cap; e++) {
        if (zap_d_tags(env)[e] < 2) {
            continue;
        }
        const ZapStr *k = (const ZapStr *)(intptr_t)zap_d_keys(env)[e];
        const ZapStr *v = (const ZapStr *)(intptr_t)zap_d_vals(env)[e];
        if (k->len > 0) {
            memcpy(block + o, k->data, (size_t)k->len);
            o += (size_t)k->len;
        }
        block[o++] = '=';
        if (v->len > 0) {
            memcpy(block + o, v->data, (size_t)v->len);
            o += (size_t)v->len;
        }
        block[o++] = '\0';
    }
    block[o++] = '\0';
    return block;
}
#endif /* ZAP_PLATFORM_WINDOWS */

/* Write `data` entirely to fd `fd` (a temp file; no short-write parking needed
 * beyond the EINTR retry). Returns 0, or -1 with errno set. */
static int zap_proc_write_all_fd(int fd, const ZapStr *data) {
    const char *p = data->data;
    int64_t left = data->len;
    while (left > 0) {
        ssize_t w = write(fd, p, (size_t)left);
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        p += w;
        left -= (int64_t)w;
    }
    return 0;
}

/* Shared blocking spawn + capture for run / check_run / run_shell (#976,
 * D-0237). `cmd_arg` is a `list[str]` argv (is_shell == 0) or a `str` shell
 * command (is_shell == 1, run through `/bin/sh -c` / `cmd.exe /C`). `env`
 * overrides/augments the environment; `cwd` (non-empty) is the working
 * directory; `timeout_s > 0` SIGKILLs on expiry (-> TimeoutError); `input`
 * (non-empty) is fed to stdin via a temp file; `capture` toggles stdout/stderr
 * capture; `merge` folds stderr into stdout. `do_check` raises RuntimeError on a
 * non-zero exit. Returns a GC `ZapProc`. Raises on spawn failure. */
static void *zap_proc_run_impl(const char *op, void *cmd_arg, const ZapDict *env,
                               const ZapStr *cwd, double timeout_s,
                               const ZapStr *input, int64_t capture, int64_t merge,
                               int do_check, int is_shell) {
    int want_in = (input && input->len > 0);
    int want_cwd = (cwd && cwd->len > 0);

    /* --- build argv --- */
    char **cargv;
    int64_t n;
    const char *display; /* argv[0] / shell command, for messages */
    if (is_shell) {
        char *cmdz = zap_sys_cstr((ZapStr *)cmd_arg);
        cargv = (char **)malloc(sizeof(char *) * 4);
        if (!cargv) {
            zap_sys_fail(op, "out of memory");
        }
#if defined(ZAP_PLATFORM_WINDOWS)
        cargv[0] = "cmd.exe";
        cargv[1] = "/C";
#else
        cargv[0] = "/bin/sh";
        cargv[1] = "-c";
#endif
        cargv[2] = cmdz;
        cargv[3] = NULL;
        n = 3;
        display = cmdz;
    } else {
        n = zap_list_len(cmd_arg);
        if (n < 1) {
            zap_sys_fail(op, "empty argument list");
        }
        cargv = (char **)malloc(sizeof(char *) * (size_t)(n + 1));
        if (!cargv) {
            zap_sys_fail(op, "out of memory");
        }
        for (int64_t i = 0; i < n; i++) {
            cargv[i] = zap_sys_cstr((ZapStr *)zap_list_get_ptr(cmd_arg, i));
        }
        cargv[n] = NULL;
        display = cargv[0];
    }

    const char *tmp = zap_tempdir();
    char itmpl[4096], otmpl[4096], etmpl[4096];
    int ifd = -1, ofd = -1, efd = -1;

    /* --- stdin temp file (only when input is supplied) --- */
    if (want_in) {
        snprintf(itmpl, sizeof itmpl, "%s/zapin_XXXXXX", tmp);
#if defined(ZAP_PLATFORM_WINDOWS)
        ifd = zap_win_mkstemp(itmpl);
#else
        ifd = mkstemp(itmpl);
#endif
        if (ifd < 0) {
            free(cargv);
            zap_sys_fail(op, strerror(errno));
        }
        if (zap_proc_write_all_fd(ifd, input) != 0) {
            int e = errno;
            close(ifd);
            unlink(itmpl);
            free(cargv);
            zap_sys_fail(op, strerror(e));
        }
        lseek(ifd, 0, SEEK_SET);
    }

    /* --- capture temp files --- */
    if (capture) {
        snprintf(otmpl, sizeof otmpl, "%s/zapout_XXXXXX", tmp);
#if defined(ZAP_PLATFORM_WINDOWS)
        ofd = zap_win_mkstemp(otmpl);
#else
        ofd = mkstemp(otmpl);
#endif
        if (ofd < 0) {
            int e = errno;
            if (ifd >= 0) {
                close(ifd);
                unlink(itmpl);
            }
            free(cargv);
            zap_sys_fail(op, strerror(e));
        }
        if (!merge) {
            snprintf(etmpl, sizeof etmpl, "%s/zaperr_XXXXXX", tmp);
#if defined(ZAP_PLATFORM_WINDOWS)
            efd = zap_win_mkstemp(etmpl);
#else
            efd = mkstemp(etmpl);
#endif
            if (efd < 0) {
                int e = errno;
                close(ofd);
                unlink(otmpl);
                if (ifd >= 0) {
                    close(ifd);
                    unlink(itmpl);
                }
                free(cargv);
                zap_sys_fail(op, strerror(e));
            }
        }
    }

    int64_t code;
    int timed_out = 0;
#if defined(ZAP_PLATFORM_WINDOWS)
    char *cmdline = zap_win_build_cmdline(cargv, n);
    free(cargv);
    char *envblock = zap_win_build_env_block(env);
    char *cwdz = want_cwd ? zap_sys_cstr((ZapStr *)cwd) : NULL;
    if (!cmdline) {
        if (envblock) {
            free(envblock);
        }
        if (ifd >= 0) {
            close(ifd);
            unlink(itmpl);
        }
        if (ofd >= 0) {
            close(ofd);
            unlink(otmpl);
        }
        if (efd >= 0) {
            close(efd);
            unlink(etmpl);
        }
        zap_sys_fail(op, "out of memory");
    }
    STARTUPINFOA si;
    memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    si.dwFlags = STARTF_USESTDHANDLES;
    HANDLE ih = want_in ? (HANDLE)_get_osfhandle(ifd) : GetStdHandle(STD_INPUT_HANDLE);
    HANDLE oh = capture ? (HANDLE)_get_osfhandle(ofd) : GetStdHandle(STD_OUTPUT_HANDLE);
    HANDLE eh = capture ? (merge ? oh : (HANDLE)_get_osfhandle(efd))
                        : GetStdHandle(STD_ERROR_HANDLE);
    if (want_in) {
        SetHandleInformation(ih, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
    }
    if (capture) {
        SetHandleInformation(oh, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        if (!merge) {
            SetHandleInformation(eh, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
    }
    si.hStdInput = ih;
    si.hStdOutput = oh;
    si.hStdError = eh;
    PROCESS_INFORMATION pi;
    memset(&pi, 0, sizeof pi);
    BOOL ok = CreateProcessA(NULL, cmdline, NULL, NULL, TRUE, 0, envblock, cwdz, &si, &pi);
    free(cmdline);
    if (envblock) {
        free(envblock);
    }
    if (!ok) {
        if (ifd >= 0) {
            close(ifd);
            unlink(itmpl);
        }
        if (ofd >= 0) {
            close(ofd);
            unlink(otmpl);
        }
        if (efd >= 0) {
            close(efd);
            unlink(etmpl);
        }
        zap_sys_fail(op, "CreateProcess failed");
    }
    if (ifd >= 0) {
        close(ifd); /* child holds its own inherited handle */
        ifd = -1;
    }
    DWORD wait_ms = (timeout_s > 0.0) ? (DWORD)(timeout_s * 1000.0) : INFINITE;
    zap_enter_blocking();
    DWORD wr = WaitForSingleObject(pi.hProcess, wait_ms);
    if (wr == WAIT_TIMEOUT) {
        TerminateProcess(pi.hProcess, 1);
        WaitForSingleObject(pi.hProcess, INFINITE);
        timed_out = 1;
    }
    zap_leave_blocking();
    DWORD exit_code = 1;
    GetExitCodeProcess(pi.hProcess, &exit_code);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    code = (int64_t)exit_code;
#else
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    if (want_in) {
        posix_spawn_file_actions_adddup2(&fa, ifd, 0);
    }
    if (capture) {
        posix_spawn_file_actions_adddup2(&fa, ofd, 1);
        posix_spawn_file_actions_adddup2(&fa, merge ? ofd : efd, 2);
    }
    if (want_cwd) {
#if defined(ZAP_HAVE_SPAWN_CHDIR)
        char *cwdz = zap_sys_cstr((ZapStr *)cwd);
        posix_spawn_file_actions_addchdir_np(&fa, cwdz);
#else
        posix_spawn_file_actions_destroy(&fa);
        if (ifd >= 0) {
            close(ifd);
            unlink(itmpl);
        }
        if (ofd >= 0) {
            close(ofd);
            unlink(otmpl);
        }
        if (efd >= 0) {
            close(efd);
            unlink(etmpl);
        }
        free(cargv);
        zap_sys_fail(op, "cwd= is not supported on this platform");
#endif
    }
    char **built = zap_proc_build_envp(env);
    char **envp = built ? built : environ;
    pid_t pid = 0;
    int rc = posix_spawnp(&pid, cargv[0], &fa, NULL, cargv, envp);
    posix_spawn_file_actions_destroy(&fa);
    zap_proc_free_envp(built);
    free(cargv);
    if (want_in) {
        close(ifd); /* the child has its own stdin from the temp file */
        ifd = -1;
    }
    if (rc != 0) {
        if (want_in) {
            unlink(itmpl);
        }
        if (ofd >= 0) {
            close(ofd);
            unlink(otmpl);
        }
        if (efd >= 0) {
            close(efd);
            unlink(etmpl);
        }
        zap_sys_fail(op, strerror(rc));
    }
    code = zap_proc_wait_timed(pid, timeout_s, &timed_out);
#endif

    if (want_in && ifd < 0) {
        unlink(itmpl);
    }
    if (timed_out) {
        if (ofd >= 0) {
            close(ofd);
            unlink(otmpl);
        }
        if (efd >= 0) {
            close(efd);
            unlink(etmpl);
        }
        char msg[256];
        int mn = snprintf(msg, sizeof msg, "%s: command timed out after %.3gs", op, timeout_s);
        if (mn < 0) {
            mn = 0;
        }
        if (mn >= (int)sizeof msg) {
            mn = (int)sizeof msg - 1;
        }
        zap_panic_typed(ZAP_EXC_TIMEOUT_ERROR, zap_str_new(msg, (int64_t)mn));
    }

    ZapProc *r = (ZapProc *)zap_obj(&zap_desc_proc);
    r->code = code;
    r->out = NULL;
    r->err = NULL;
    if (capture) {
        r->out = (ZapStr *)zap_read_all_fd(ofd);
        close(ofd);
        unlink(otmpl);
        if (merge) {
            r->err = (ZapStr *)zap_str_new("", 0);
        } else {
            r->err = (ZapStr *)zap_read_all_fd(efd);
            close(efd);
            unlink(etmpl);
        }
    } else {
        r->out = (ZapStr *)zap_str_new("", 0);
        r->err = (ZapStr *)zap_str_new("", 0);
    }

    if (do_check && code != 0) {
        /* RuntimeError (Zap has no dedicated CalledProcessError, D-0237): the
         * message carries argv[0]/command and the exit code, plus a short tail of
         * captured stderr (or merged stdout) for context. */
        const ZapStr *tail = (!merge && r->err && r->err->len > 0) ? r->err : r->out;
        char msg[512];
        int mn = snprintf(msg, sizeof msg, "%s: command '%s' exited with code %lld", op,
                          display ? display : "?", (long long)code);
        if (mn < 0) {
            mn = 0;
        }
        if (tail && tail->len > 0 && mn < (int)sizeof msg - 2) {
            int room = (int)sizeof msg - mn - 1;
            int take = (int)(tail->len < room ? tail->len : room);
            msg[mn++] = ':';
            msg[mn++] = ' ';
            room = (int)sizeof msg - mn;
            if (take > room) {
                take = room;
            }
            memcpy(msg + mn, tail->data, (size_t)take);
            mn += take;
        }
        if (mn >= (int)sizeof msg) {
            mn = (int)sizeof msg - 1;
        }
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR, zap_str_new(msg, (int64_t)mn));
    }
    return r;
}

/* process.run(cmd, env, cwd, timeout_s, input, capture_output, merge_stderr)
 * -> ZapProc* (#97, #976). The exit code is the child's status, or 128+signo if
 * it was killed by a signal (POSIX). Raises on spawn failure / timeout. */
void *zap_process_run(void *argv_list, void *env, void *cwd, double timeout_s,
                      void *input, int64_t capture, int64_t merge) {
    return zap_proc_run_impl("process.run", argv_list, (const ZapDict *)env,
                             (const ZapStr *)cwd, timeout_s, (const ZapStr *)input,
                             capture, merge, 0, 0);
}

/* process.check_run(...): like run, but raises RuntimeError on a non-zero exit
 * (Python `subprocess.run(check=True)`). Returns the `(0, out, err)` tuple. */
void *zap_process_check_run(void *argv_list, void *env, void *cwd, double timeout_s,
                            void *input, int64_t capture, int64_t merge) {
    return zap_proc_run_impl("process.check_run", argv_list, (const ZapDict *)env,
                             (const ZapStr *)cwd, timeout_s, (const ZapStr *)input,
                             capture, merge, 1, 0);
}

/* process.run_shell(cmd, ...): run `cmd` through the system shell (`/bin/sh -c`
 * on POSIX, `cmd.exe /C` on Windows). Explicit opt-in shell form (#976). */
void *zap_process_run_shell(void *cmd, void *env, void *cwd, double timeout_s,
                            void *input, int64_t capture, int64_t merge) {
    return zap_proc_run_impl("process.run_shell", cmd, (const ZapDict *)env,
                             (const ZapStr *)cwd, timeout_s, (const ZapStr *)input,
                             capture, merge, 0, 1);
}

/* Field getters the lowerer calls to unpack the result into a tuple. */
int64_t zap_process_result_code(void *r) { return ((ZapProc *)r)->code; }
void *zap_process_result_stdout(void *r) { return ((ZapProc *)r)->out; }
void *zap_process_result_stderr(void *r) { return ((ZapProc *)r)->err; }

/* --- process: streaming Popen-style handle (#976, D-0237) -------------------
 *
 * `process.Popen(...)` (the prelude struct) spawns a child with pipes for the
 * selected streams and hands back an opaque `ZapPopen` handle. Non-blocking
 * pipe reads/writes PARK the calling coroutine on the netpoller (epoll), freeing
 * the worker thread rather than blocking it — so interleaved stdout/stderr
 * streaming of output larger than a pipe buffer never deadlocks.
 *
 * The handle is a NON-GC malloc block addressed by an `int64` handle (the
 * `http.Client` pool pattern, #990): a stable address the precise-Immix
 * collector never moves, so the C code may hold it — and mutate its state
 * (`reaped`/`returncode`, closing a stream's fd) — ACROSS a park safely. (A GC
 * object would move under evacuation, invalidating the C pointer held across the
 * park.) Scratch read buffers are likewise malloc, copied into a GC `bytes` only
 * at the very end, after the last park. The block's fds/netpoller descriptors
 * are released on stream EOF, `close_stdin`, and `wait`; the small control block
 * itself is intentionally leaked on drop (like the keep-alive pool without
 * `shutdown`) — spawn a bounded number of children and join each with `wait`.
 *
 * Windows: streaming over anonymous pipes needs named-pipe + overlapped IOCP
 * integration (I#196 basis) — a substantial, separately-landable follow-up that
 * CI (Linux-only) cannot exercise. `zap_popen_spawn` therefore raises a clear
 * "not yet supported on Windows" error there; the blocking run/check_run/
 * run_shell forms remain fully cross-platform. */

#define ZAP_POPEN_RUNNING (-256) /* poll() sentinel: child has not exited yet */

typedef struct {
    int64_t pid;
    int in_fd;  /* stdin write end, or -1 (not opened / closed) */
    int out_fd; /* stdout read end, or -1 */
    int err_fd; /* stderr read end, or -1 */
    void *in_pd;
    void *out_pd;
    void *err_pd;
    int64_t returncode;
    int reaped; /* 0 = running, 1 = exited (returncode valid) */
} ZapPopen;

/* The int64 handle is the malloc'd block's address; cast back here. */
static ZapPopen *zap_popen_of(int64_t h) { return (ZapPopen *)(intptr_t)h; }

#if defined(ZAP_PLATFORM_POSIX)
/* Make `fd` non-blocking and register it with the netpoller. `*pd_out` is the
 * descriptor (NULL when the netpoller is unavailable -> blocking-poll fallback). */
static void zap_popen_arm_fd(int fd, void **pd_out) {
    int fl = fcntl(fd, F_GETFL, 0);
    if (fl >= 0) {
        fcntl(fd, F_SETFL, fl | O_NONBLOCK);
    }
    *pd_out = zap_netpoll_open((int64_t)fd);
}

/* Park until `fd` is ready for `mode` (ZAP_POLL_READ / _WRITE). Returns 0 ready,
 * -1 closed, -3 cancelled. With the netpoller this frees the worker thread; the
 * pd==NULL fallback blocks on poll() bracketed by enter/leave_blocking. */
static int zap_popen_park(int fd, void *pd, int mode) {
    if (pd) {
        int64_t r = zap_netpoll_wait(pd, mode, 0);
        if (r == ZAP_POLL_CANCELLED) {
            return -3;
        }
        if (r == ZAP_POLL_CLOSED) {
            return -1;
        }
        return 0;
    }
    for (;;) {
        struct pollfd pfd;
        pfd.fd = fd;
        pfd.events = (short)((mode == ZAP_POLL_READ) ? POLLIN : POLLOUT);
        pfd.revents = 0;
        zap_enter_blocking();
        int n = poll(&pfd, 1, -1);
        int saved = errno;
        zap_leave_blocking();
        if (n < 0 && saved == EINTR) {
            continue;
        }
        return (n < 0) ? -1 : 0;
    }
}

/* Close a stream fd and deregister its netpoller descriptor (idempotent). */
static void zap_popen_close_stream(int *fd, void **pd) {
    if (*pd) {
        zap_netpoll_close(*pd);
        *pd = NULL;
    }
    if (*fd >= 0) {
        close(*fd);
        *fd = -1;
    }
}
#endif /* ZAP_PLATFORM_POSIX */

/* process._popen_spawn(argv, env, cwd, flags) -> int64 handle. flags bits:
 * 1 merge_stderr, 2 want_stdin, 4 want_stdout, 8 want_stderr. */
int64_t zap_popen_spawn(void *argv_list, void *env, void *cwd, int64_t flags) {
#if defined(ZAP_PLATFORM_WINDOWS)
    (void)argv_list;
    (void)env;
    (void)cwd;
    (void)flags;
    zap_sys_fail("process.Popen",
                 "streaming subprocess is not yet supported on Windows (#976)");
    return 0;
#else
    int merge = (flags & 1) != 0;
    int want_in = (flags & 2) != 0;
    int want_out = (flags & 4) != 0;
    int want_err = (flags & 8) != 0 && !merge;
    const ZapStr *cwds = (const ZapStr *)cwd;
    int want_cwd = (cwds && cwds->len > 0);

    int64_t n = zap_list_len(argv_list);
    if (n < 1) {
        zap_sys_fail("process.Popen", "empty argument list");
    }
    char **cargv = (char **)malloc(sizeof(char *) * (size_t)(n + 1));
    if (!cargv) {
        zap_sys_fail("process.Popen", "out of memory");
    }
    for (int64_t i = 0; i < n; i++) {
        cargv[i] = zap_sys_cstr((ZapStr *)zap_list_get_ptr(argv_list, i));
    }
    cargv[n] = NULL;

    /* Pipes: [0] read end, [1] write end. Parent keeps the far end per stream. */
    int ipipe[2] = {-1, -1}, opipe[2] = {-1, -1}, epipe[2] = {-1, -1};
    if ((want_in && pipe(ipipe) != 0) || (want_out && pipe(opipe) != 0) ||
        (want_err && pipe(epipe) != 0)) {
        int e = errno;
        for (int k = 0; k < 2; k++) {
            if (ipipe[k] >= 0) close(ipipe[k]);
            if (opipe[k] >= 0) close(opipe[k]);
            if (epipe[k] >= 0) close(epipe[k]);
        }
        free(cargv);
        zap_sys_fail("process.Popen", strerror(e));
    }

    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    if (want_in) {
        posix_spawn_file_actions_adddup2(&fa, ipipe[0], 0);
        posix_spawn_file_actions_addclose(&fa, ipipe[1]); /* child drops the write end */
    }
    if (want_out) {
        posix_spawn_file_actions_adddup2(&fa, opipe[1], 1);
        posix_spawn_file_actions_addclose(&fa, opipe[0]);
    }
    if (merge && want_out) {
        posix_spawn_file_actions_adddup2(&fa, opipe[1], 2); /* 2>&1 */
    } else if (want_err) {
        posix_spawn_file_actions_adddup2(&fa, epipe[1], 2);
        posix_spawn_file_actions_addclose(&fa, epipe[0]);
    }
    if (want_cwd) {
#if defined(ZAP_HAVE_SPAWN_CHDIR)
        posix_spawn_file_actions_addchdir_np(&fa, zap_sys_cstr((ZapStr *)cwd));
#else
        posix_spawn_file_actions_destroy(&fa);
        for (int k = 0; k < 2; k++) {
            if (ipipe[k] >= 0) close(ipipe[k]);
            if (opipe[k] >= 0) close(opipe[k]);
            if (epipe[k] >= 0) close(epipe[k]);
        }
        free(cargv);
        zap_sys_fail("process.Popen", "cwd= is not supported on this platform");
#endif
    }

    char **built = zap_proc_build_envp((const ZapDict *)env);
    char **envp = built ? built : environ;
    pid_t pid = 0;
    int rc = posix_spawnp(&pid, cargv[0], &fa, NULL, cargv, envp);
    posix_spawn_file_actions_destroy(&fa);
    zap_proc_free_envp(built);
    free(cargv);

    /* Close the child's ends we kept open in the parent. */
    if (want_in && ipipe[0] >= 0) close(ipipe[0]);
    if (want_out && opipe[1] >= 0) close(opipe[1]);
    if (want_err && epipe[1] >= 0) close(epipe[1]);

    if (rc != 0) {
        if (ipipe[1] >= 0) close(ipipe[1]);
        if (opipe[0] >= 0) close(opipe[0]);
        if (epipe[0] >= 0) close(epipe[0]);
        zap_sys_fail("process.Popen", strerror(rc));
    }

    ZapPopen *h = (ZapPopen *)malloc(sizeof(ZapPopen));
    if (!h) {
        if (ipipe[1] >= 0) close(ipipe[1]);
        if (opipe[0] >= 0) close(opipe[0]);
        if (epipe[0] >= 0) close(epipe[0]);
        if (rc == 0) {
            kill(pid, SIGKILL);
            int st;
            waitpid(pid, &st, 0);
        }
        zap_sys_fail("process.Popen", "out of memory");
    }
    h->pid = (int64_t)pid;
    h->in_fd = want_in ? ipipe[1] : -1;
    h->out_fd = want_out ? opipe[0] : -1;
    h->err_fd = want_err ? epipe[0] : -1;
    h->in_pd = NULL;
    h->out_pd = NULL;
    h->err_pd = NULL;
    h->returncode = 0;
    h->reaped = 0;
    if (h->in_fd >= 0) zap_popen_arm_fd(h->in_fd, &h->in_pd);
    if (h->out_fd >= 0) zap_popen_arm_fd(h->out_fd, &h->out_pd);
    if (h->err_fd >= 0) zap_popen_arm_fd(h->err_fd, &h->err_pd);
    return (int64_t)(intptr_t)h;
#endif
}

/* process._popen_write(h, data) -> bytes written (== len(data)). Parks while the
 * pipe is full; raises on a broken pipe or a closed stdin. */
int64_t zap_popen_write(int64_t h_, void *data_) {
#if defined(ZAP_PLATFORM_WINDOWS)
    (void)h_;
    (void)data_;
    zap_sys_fail("process.Popen.write", "not supported on Windows");
    return 0;
#else
    ZapPopen *h = zap_popen_of(h_);
    const ZapStr *data = (const ZapStr *)data_;
    if (h->in_fd < 0) {
        zap_sys_fail("process.Popen.write", "stdin is not open");
    }
    int64_t total = data->len;
    if (total <= 0) {
        return 0;
    }
    /* Copy into malloc scratch: the loop parks, and a moving GC could relocate
     * the source ZapStr out from under a pointer held across the park. */
    char *scratch = (char *)malloc((size_t)total);
    if (!scratch) {
        zap_sys_fail("process.Popen.write", "out of memory");
    }
    memcpy(scratch, data->data, (size_t)total);
    int64_t left = total;
    const char *p = scratch;
    while (left > 0) {
        ssize_t w = write(h->in_fd, p, (size_t)left);
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                int pr = zap_popen_park(h->in_fd, h->in_pd, ZAP_POLL_WRITE);
                if (pr == -3) {
                    free(scratch);
                    zap_raise(cancelled_exc());
                }
                if (pr < 0) {
                    free(scratch);
                    zap_sys_fail("process.Popen.write", "stdin closed");
                }
                continue;
            }
            free(scratch);
            zap_sys_fail("process.Popen.write", strerror(errno));
        }
        p += w;
        left -= (int64_t)w;
    }
    free(scratch);
    return total;
#endif
}

/* process._popen_close_stdin(h): close the child's stdin (signals EOF). */
void zap_popen_close_stdin(int64_t h_) {
#if defined(ZAP_PLATFORM_POSIX)
    ZapPopen *h = zap_popen_of(h_);
    zap_popen_close_stream(&h->in_fd, &h->in_pd);
#else
    (void)h_;
#endif
}

#if defined(ZAP_PLATFORM_POSIX)
/* Read up to `maxn` bytes from stream `which` (1=stdout, 2=stderr) into `buf`,
 * parking until data arrives. Returns the byte count (0 at EOF), closing the
 * stream on EOF. Raises on error / cancellation. */
static int64_t zap_popen_read_into(ZapPopen *h, int64_t which, char *buf, int64_t maxn) {
    int *fd = (which == 2) ? &h->err_fd : &h->out_fd;
    void **pd = (which == 2) ? &h->err_pd : &h->out_pd;
    if (*fd < 0) {
        return 0; /* stream not open / already at EOF */
    }
    for (;;) {
        ssize_t r = read(*fd, buf, (size_t)maxn);
        if (r > 0) {
            return (int64_t)r;
        }
        if (r == 0) {
            zap_popen_close_stream(fd, pd);
            return 0;
        }
        if (errno == EINTR) {
            continue;
        }
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            int pr = zap_popen_park(*fd, *pd, ZAP_POLL_READ);
            if (pr == -3) {
                zap_raise(cancelled_exc());
            }
            if (pr < 0) {
                zap_popen_close_stream(fd, pd);
                return 0;
            }
            continue;
        }
        zap_sys_fail("process.Popen.read", strerror(errno));
    }
}
#endif /* ZAP_PLATFORM_POSIX */

/* process._popen_read(h, which, maxn) -> bytes (b"" at EOF). The scratch buffer
 * is malloc (it lives across the park, where a moving GC would relocate a GC
 * buffer); the result is copied into a GC `bytes` at the end. */
void *zap_popen_read(int64_t h_, int64_t which, int64_t maxn) {
#if defined(ZAP_PLATFORM_WINDOWS)
    (void)h_;
    (void)which;
    (void)maxn;
    zap_sys_fail("process.Popen.read", "not supported on Windows");
    return NULL;
#else
    ZapPopen *h = zap_popen_of(h_);
    if (maxn <= 0) {
        maxn = 65536;
    }
    char *buf = (char *)malloc((size_t)maxn);
    if (!buf) {
        zap_sys_fail("process.Popen.read", "out of memory");
    }
    int64_t got = zap_popen_read_into(h, which, buf, maxn);
    void *out = zap_str_new(buf, got);
    free(buf);
    return out;
#endif
}

/* process._popen_read_all(h, which) -> bytes read to end-of-output. */
void *zap_popen_read_all(int64_t h_, int64_t which) {
#if defined(ZAP_PLATFORM_WINDOWS)
    (void)h_;
    (void)which;
    zap_sys_fail("process.Popen.read_all", "not supported on Windows");
    return NULL;
#else
    ZapPopen *h = zap_popen_of(h_);
    size_t cap = 4096, len = 0;
    char *buf = (char *)malloc(cap);
    if (!buf) {
        zap_sys_fail("process.Popen.read_all", "out of memory");
    }
    for (;;) {
        if (len == cap) {
            size_t ncap = cap * 2;
            char *nb = (char *)realloc(buf, ncap);
            if (!nb) {
                free(buf);
                zap_sys_fail("process.Popen.read_all", "out of memory");
            }
            buf = nb;
            cap = ncap;
        }
        int64_t r = zap_popen_read_into(h, which, buf + len, (int64_t)(cap - len));
        if (r == 0) {
            break;
        }
        len += (size_t)r;
    }
    void *out = zap_str_new(buf, (int64_t)len);
    free(buf);
    return out;
#endif
}

#if defined(ZAP_PLATFORM_POSIX)
/* Reap the child without blocking; on exit record the decoded returncode.
 * Returns 1 if (now) reaped, 0 if still running. */
static int zap_popen_try_reap(ZapPopen *h) {
    if (h->reaped) {
        return 1;
    }
    int status = 0;
    pid_t w;
    do {
        w = waitpid((pid_t)h->pid, &status, WNOHANG);
    } while (w < 0 && errno == EINTR);
    if (w == (pid_t)h->pid) {
        if (WIFEXITED(status)) {
            h->returncode = (int64_t)WEXITSTATUS(status);
        } else if (WIFSIGNALED(status)) {
            h->returncode = 128 + (int64_t)WTERMSIG(status);
        } else {
            h->returncode = -1;
        }
        h->reaped = 1;
        return 1;
    }
    return 0;
}
#endif /* ZAP_PLATFORM_POSIX */

/* process._popen_poll(h) -> returncode, or ZAP_POPEN_RUNNING while still alive. */
int64_t zap_popen_poll(int64_t h_) {
#if defined(ZAP_PLATFORM_POSIX)
    ZapPopen *h = zap_popen_of(h_);
    return zap_popen_try_reap(h) ? h->returncode : ZAP_POPEN_RUNNING;
#else
    (void)h_;
    return ZAP_POPEN_RUNNING;
#endif
}

/* process._popen_wait(h) -> returncode (blocks until exit). Releases the stream
 * fds/netpoller descriptors on return (read all output BEFORE waiting). */
int64_t zap_popen_wait(int64_t h_) {
#if defined(ZAP_PLATFORM_POSIX)
    ZapPopen *h = zap_popen_of(h_);
    if (!h->reaped) {
        int status = 0;
        pid_t w;
        zap_enter_blocking();
        do {
            w = waitpid((pid_t)h->pid, &status, 0);
        } while (w < 0 && errno == EINTR);
        zap_leave_blocking();
        if (w < 0) {
            h->returncode = -1;
        } else if (WIFEXITED(status)) {
            h->returncode = (int64_t)WEXITSTATUS(status);
        } else if (WIFSIGNALED(status)) {
            h->returncode = 128 + (int64_t)WTERMSIG(status);
        } else {
            h->returncode = -1;
        }
        h->reaped = 1;
    }
    zap_popen_close_stream(&h->in_fd, &h->in_pd);
    zap_popen_close_stream(&h->out_fd, &h->out_pd);
    zap_popen_close_stream(&h->err_fd, &h->err_pd);
    return h->returncode;
#else
    (void)h_;
    zap_sys_fail("process.Popen.wait", "not supported on Windows");
    return -1;
#endif
}

/* process._popen_returncode(h) -> returncode; raises RuntimeError if running. */
int64_t zap_popen_returncode(int64_t h_) {
#if defined(ZAP_PLATFORM_POSIX)
    ZapPopen *h = zap_popen_of(h_);
    if (!zap_popen_try_reap(h)) {
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                        zap_str_new("process.Popen.returncode: process still running", 48));
    }
    return h->returncode;
#else
    (void)h_;
    zap_sys_fail("process.Popen.returncode", "not supported on Windows");
    return -1;
#endif
}

/* process._popen_pid(h) -> child process id. */
int64_t zap_popen_pid(int64_t h_) { return zap_popen_of(h_)->pid; }

/* process._popen_signal(h, sig): terminate(15) / kill(9). No-op once reaped. */
void zap_popen_signal(int64_t h_, int64_t sig) {
#if defined(ZAP_PLATFORM_POSIX)
    ZapPopen *h = zap_popen_of(h_);
    if (!h->reaped) {
        kill((pid_t)h->pid, (int)sig);
    }
#else
    (void)h_;
    (void)sig;
    zap_sys_fail("process.Popen.signal", "not supported on Windows");
#endif
}

/* --- Line-oriented input (#969, D-0212) ------------------------------------
 *
 * `input([prompt])` plus a buffered reader over an fd. The reader is an opaque
 * handle built with the crypto-streaming pattern (`zap_crypto_new`): the handle
 * is a `bytes` value (ZapStr) whose buffer is a POINTER-FREE `zap_io_reader`
 * struct, so the precise/moving collector traces and frees it with no special
 * finalizer. Line/chunk accumulators use plain `malloc`/`realloc` and are copied
 * into a fresh ZapStr before return, so no raw pointer is held across an
 * allocation that the GC could observe. */

/* input([prompt]) -> str: write the optional prompt to stdout (no trailing
 * newline), then read one line from stdin (fd 0). The returned string excludes
 * the trailing '\n' and a preceding '\r' (CRLF). At end-of-input with no bytes
 * read it raises EOFError (#1036, D-0215), matching CPython's `input()`. Reads a
 * byte at a time so it never consumes past
 * the newline, which keeps the fd positioned for a following reader. */
void *zap_input(void *prompt_) {
    if (prompt_) {
        ZapStr *p = (ZapStr *)prompt_;
        size_t off = 0;
        while (off < (size_t)p->len) {
            ssize_t w = write(1, p->data + off, (size_t)p->len - off);
            if (w < 0) {
                if (errno == EINTR) {
                    continue;
                }
                zap_sys_fail("input", strerror(errno));
            }
            off += (size_t)w;
        }
    }
    size_t cap = 128, len = 0;
    char *buf = (char *)malloc(cap);
    if (!buf) {
        zap_sys_fail("input", "out of memory");
    }
    int saw_any = 0;
    for (;;) {
        char c;
        ssize_t got;
        do {
            got = read(0, &c, 1);
        } while (got < 0 && errno == EINTR);
        if (got < 0) {
            free(buf);
            zap_sys_fail("input", strerror(errno));
        }
        if (got == 0) {
            break; /* EOF */
        }
        saw_any = 1;
        if (c == '\n') {
            break;
        }
        if (len + 1 > cap) {
            cap *= 2;
            char *nb = (char *)realloc(buf, cap);
            if (!nb) {
                free(buf);
                zap_sys_fail("input", "out of memory");
            }
            buf = nb;
        }
        buf[len++] = c;
    }
    if (!saw_any) {
        free(buf);
        const char *msg = "EOF when reading a line";
        zap_panic_typed(ZAP_EXC_EOF_ERROR,
                        zap_str_new(msg, (int64_t)strlen(msg)));
        return NULL; /* unreachable */
    }
    if (len > 0 && buf[len - 1] == '\r') {
        len--; /* strip a CRLF carriage return */
    }
    void *s = zap_str_new(buf, (int64_t)len);
    free(buf);
    return s;
}

/* Buffered reader over an fd. All fields are pointer-free, so the handle's
 * ZapStr buffer is traced safely by the precise collector. */
#define ZAP_IO_RBUF 8192
typedef struct {
    int64_t fd;
    int32_t fill; /* bytes currently valid in buf */
    int32_t pos;  /* index of next unread byte in buf */
    int32_t eof;  /* 1 once read() has reported end-of-input */
    int32_t _pad;
    char buf[ZAP_IO_RBUF];
} zap_io_reader;

/* io.reader(fd) -> handle: wrap an open fd in an 8 KiB-buffered reader. */
void *zap_io_reader_new(int64_t fd) {
    ZapStr *h = zap_str_alloc((int64_t)sizeof(zap_io_reader));
    zap_io_reader *r = (zap_io_reader *)(void *)h->data;
    r->fd = fd;
    r->fill = 0;
    r->pos = 0;
    r->eof = 0;
    r->_pad = 0;
    return h;
}

/* Ensure buffered bytes are available; returns the count unread in buf (0 at
 * EOF). Never allocates, so the caller's `r` pointer stays valid across it. */
static int32_t zap_io_reader_fill(zap_io_reader *r) {
    if (r->pos < r->fill) {
        return r->fill - r->pos;
    }
    if (r->eof) {
        return 0;
    }
    ssize_t got;
    do {
        got = read((int)r->fd, r->buf, ZAP_IO_RBUF);
    } while (got < 0 && errno == EINTR);
    if (got < 0) {
        zap_sys_fail("io.reader", strerror(errno));
    }
    r->pos = 0;
    r->fill = (int32_t)got;
    if (got == 0) {
        r->eof = 1;
        return 0;
    }
    return r->fill;
}

/* io.readline(reader) -> str: the next line INCLUDING its trailing '\n'; "" at
 * EOF. Keeping the newline lets a `while line != ""` loop distinguish a blank
 * line from end-of-input. */
void *zap_io_readline(void *h_) {
    zap_io_reader *r = (zap_io_reader *)(void *)((ZapStr *)h_)->data;
    size_t cap = 128, len = 0;
    char *out = (char *)malloc(cap);
    if (!out) {
        zap_sys_fail("io.readline", "out of memory");
    }
    for (;;) {
        if (zap_io_reader_fill(r) == 0) {
            break; /* EOF */
        }
        char *start = r->buf + r->pos;
        int32_t avail = r->fill - r->pos;
        char *nl = (char *)memchr(start, '\n', (size_t)avail);
        int32_t take = nl ? (int32_t)(nl - start) + 1 : avail;
        if (len + (size_t)take > cap) {
            while (len + (size_t)take > cap) {
                cap *= 2;
            }
            char *nb = (char *)realloc(out, cap);
            if (!nb) {
                free(out);
                zap_sys_fail("io.readline", "out of memory");
            }
            out = nb;
        }
        memcpy(out + len, start, (size_t)take);
        len += (size_t)take;
        r->pos += take;
        if (nl) {
            break; /* found the line terminator */
        }
    }
    void *s = zap_str_new(out, (int64_t)len);
    free(out);
    return s;
}

/* io.readall(reader) -> str: the remainder of the stream as one string. */
void *zap_io_readall(void *h_) {
    zap_io_reader *r = (zap_io_reader *)(void *)((ZapStr *)h_)->data;
    size_t cap = ZAP_IO_RBUF, len = 0;
    char *out = (char *)malloc(cap);
    if (!out) {
        zap_sys_fail("io.readall", "out of memory");
    }
    for (;;) {
        if (zap_io_reader_fill(r) == 0) {
            break; /* EOF */
        }
        int32_t avail = r->fill - r->pos;
        if (len + (size_t)avail > cap) {
            while (len + (size_t)avail > cap) {
                cap *= 2;
            }
            char *nb = (char *)realloc(out, cap);
            if (!nb) {
                free(out);
                zap_sys_fail("io.readall", "out of memory");
            }
            out = nb;
        }
        memcpy(out + len, r->buf + r->pos, (size_t)avail);
        len += (size_t)avail;
        r->pos = r->fill;
    }
    void *s = zap_str_new(out, (int64_t)len);
    free(out);
    return s;
}

/* io.readn(reader, n) -> str: up to `n` bytes (fewer at EOF, "" once drained). */
void *zap_io_readn(void *h_, int64_t n) {
    if (n <= 0) {
        return zap_str_new("", 0);
    }
    zap_io_reader *r = (zap_io_reader *)(void *)((ZapStr *)h_)->data;
    char *out = (char *)malloc((size_t)n);
    if (!out) {
        zap_sys_fail("io.readn", "out of memory");
    }
    size_t len = 0;
    while (len < (size_t)n) {
        if (zap_io_reader_fill(r) == 0) {
            break; /* EOF */
        }
        size_t avail = (size_t)(r->fill - r->pos);
        size_t take = avail < (size_t)n - len ? avail : (size_t)n - len;
        memcpy(out + len, r->buf + r->pos, take);
        len += take;
        r->pos += (int32_t)take;
    }
    void *s = zap_str_new(out, (int64_t)len);
    free(out);
    return s;
}

/* Buffered writer over an fd — the write twin of zap_io_reader (#1071). All
 * fields are pointer-free, so the handle's ZapStr buffer is traced safely by the
 * precise collector (the crypto-streaming/D-0133 pattern). None of the writer
 * entry points allocates a Zap object, so the raw `w`/`s` pointers into GC
 * buffers stay valid across the blocking read/write syscalls (same discipline
 * as zap_io_reader_fill). */
#define ZAP_IO_WBUF 8192
typedef struct {
    int64_t fd;
    int32_t len;    /* bytes currently buffered in buf (0..ZAP_IO_WBUF) */
    int32_t closed; /* 1 once close() has run; write/flush then raise */
    char buf[ZAP_IO_WBUF];
} zap_io_writer;

/* io.writer(fd) -> handle: wrap an open, writable fd in an 8 KiB-buffered
 * writer. Only allocates the handle; never does I/O, so it cannot raise. */
void *zap_io_writer_new(int64_t fd) {
    ZapStr *h = zap_str_alloc((int64_t)sizeof(zap_io_writer));
    zap_io_writer *w = (zap_io_writer *)(void *)h->data;
    w->fd = fd;
    w->len = 0;
    w->closed = 0;
    return h;
}

/* Write exactly `n` bytes from `p` to the fd, looping over short writes and
 * retrying on EINTR. Raises (RuntimeError) on a real error. Never allocates. */
static void zap_io_write_full(zap_io_writer *w, const char *p, size_t n) {
    size_t off = 0;
    while (off < n) {
        ssize_t got = write((int)w->fd, p + off, n - off);
        if (got < 0) {
            if (errno == EINTR) {
                continue;
            }
            zap_sys_fail("io.writer", strerror(errno));
        }
        off += (size_t)got;
    }
}

/* Drain any buffered bytes to the fd (no-op when empty). */
static void zap_io_writer_drain(zap_io_writer *w) {
    if (w->len > 0) {
        zap_io_write_full(w, w->buf, (size_t)w->len);
        w->len = 0;
    }
}

/* Raise ValueError on any write/flush after close (matches Python's
 * "I/O operation on closed file"). */
static void zap_io_writer_closed_fail(void) {
    const char *m = "I/O operation on closed writer";
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}

/* io._writer_write(handle, data) -> int64: write `data`, returning the number
 * of bytes written (always len(data) once it returns). Buffered: bytes may sit
 * in the writer until a flush/close. A write at least as large as the buffer is
 * sent straight through (after draining, to keep output ordered) to avoid a
 * pointless copy. Raises ValueError on a closed writer, RuntimeError on I/O. */
int64_t zap_io_writer_write(void *h_, void *data_) {
    zap_io_writer *w = (zap_io_writer *)(void *)((ZapStr *)h_)->data;
    if (w->closed) {
        zap_io_writer_closed_fail();
    }
    ZapStr *s = (ZapStr *)data_;
    size_t n = (size_t)s->len;
    if (n == 0) {
        return 0;
    }
    if (n >= (size_t)ZAP_IO_WBUF) {
        zap_io_writer_drain(w);
        zap_io_write_full(w, s->data, n);
        return (int64_t)n;
    }
    if ((size_t)w->len + n > (size_t)ZAP_IO_WBUF) {
        zap_io_writer_drain(w);
    }
    memcpy(w->buf + w->len, s->data, n);
    w->len += (int32_t)n;
    return (int64_t)n;
}

/* io._writer_flush(handle): drain the buffer to the fd. Raises ValueError on a
 * closed writer, RuntimeError on I/O. */
void zap_io_writer_flush(void *h_) {
    zap_io_writer *w = (zap_io_writer *)(void *)((ZapStr *)h_)->data;
    if (w->closed) {
        zap_io_writer_closed_fail();
    }
    zap_io_writer_drain(w);
}

/* io._writer_close(handle): flush any buffered bytes, then close the underlying
 * fd. Idempotent — a second close is a no-op, so the `with` __exit__ may always
 * call it even after an explicit close in the body. Raises RuntimeError if the
 * final flush or the close fails. Unlike the reader (which never touches the
 * fd), the writer owns the fd's lifetime once closed, matching Python's
 * file.close() and the resource release the `with` form and fs.temp_file need. */
void zap_io_writer_close(void *h_) {
    zap_io_writer *w = (zap_io_writer *)(void *)((ZapStr *)h_)->data;
    if (w->closed) {
        return;
    }
    zap_io_writer_drain(w); /* may raise; leave closed=0 so a retry can flush */
    w->closed = 1;
    if (close((int)w->fd) != 0) {
        zap_sys_fail("io.writer", strerror(errno));
    }
}

/* fs.lines(path) -> list[str]: read the whole file and split into lines with the
 * terminators stripped (`str.splitlines` semantics). A convenience for files
 * that fit in memory; stream larger inputs with io.reader + io.readline. */
void *zap_fs_lines(void *path_) {
    void *content = zap_fs_read_file(path_); /* raises on a bad path */
    return zap_str_splitlines(content);
}

/* --- path matching: fnmatch / glob (#1033, D-0216) ------------------------- */

/* Shell-glob matcher over two byte strings. `*` matches any run (including '/',
 * which is why `glob` applies it per path segment), `?` any single byte,
 * `[seq]` / `[!seq]` a byte class with `a-z` ranges. Recursive with `*`
 * backtracking; patterns are short, so the O(n*m) worst case is fine. */
static int zap_fnmatch_core(const char *s, int64_t ns, const char *p, int64_t np) {
    if (np == 0) {
        return ns == 0;
    }
    if (p[0] == '*') {
        int64_t k = 0;
        while (k < np && p[k] == '*') {
            k++; /* collapse consecutive stars */
        }
        if (k == np) {
            return 1; /* trailing star(s) match the rest */
        }
        for (int64_t i = 0; i <= ns; i++) {
            if (zap_fnmatch_core(s + i, ns - i, p + k, np - k)) {
                return 1;
            }
        }
        return 0;
    }
    if (ns == 0) {
        return 0;
    }
    if (p[0] == '?') {
        return zap_fnmatch_core(s + 1, ns - 1, p + 1, np - 1);
    }
    if (p[0] == '[') {
        int64_t j = 1;
        int neg = 0;
        if (j < np && (p[j] == '!' || p[j] == '^')) {
            neg = 1;
            j++;
        }
        int matched = 0;
        int64_t first = j;
        while (j < np && (p[j] != ']' || j == first)) {
            if (j + 2 < np && p[j + 1] == '-' && p[j + 2] != ']') {
                unsigned char c = (unsigned char)s[0];
                if (c >= (unsigned char)p[j] && c <= (unsigned char)p[j + 2]) {
                    matched = 1;
                }
                j += 3;
            } else {
                if (s[0] == p[j]) {
                    matched = 1;
                }
                j++;
            }
        }
        if (j >= np) {
            /* no closing ']': treat '[' as a literal */
            return s[0] == '[' ? zap_fnmatch_core(s + 1, ns - 1, p + 1, np - 1) : 0;
        }
        if (matched != neg) {
            return zap_fnmatch_core(s + 1, ns - 1, p + j + 1, np - (j + 1));
        }
        return 0;
    }
    return s[0] == p[0] ? zap_fnmatch_core(s + 1, ns - 1, p + 1, np - 1) : 0;
}

/* fs.fnmatch(name, pattern) -> bool: whole-string shell-glob match. */
int64_t zap_fs_fnmatch(void *name_, void *pat_) {
    const ZapStr *n = (const ZapStr *)name_;
    const ZapStr *p = (const ZapStr *)pat_;
    return zap_fnmatch_core(n->data, n->len, p->data, p->len) ? 1 : 0;
}

/* Growable array of NUL-terminated result paths (relative to the glob root). */
typedef struct {
    char **items;
    size_t len, cap;
} zap_glob_acc;

static char *zap_glob_dup(const char *s, size_t n) {
    char *out = (char *)malloc(n + 1);
    if (!out) {
        const char *m = "out of memory";
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    if (n > 0) {
        memcpy(out, s, n);
    }
    out[n] = '\0';
    return out;
}

static void zap_glob_push(zap_glob_acc *a, const char *s) {
    if (a->len == a->cap) {
        a->cap = a->cap ? a->cap * 2 : 16;
        a->items = (char **)realloc(a->items, a->cap * sizeof(char *));
        if (!a->items) {
            const char *m = "out of memory";
            zap_panic_typed(ZAP_EXC_RUNTIME_ERROR, zap_str_new(m, (int64_t)strlen(m)));
        }
    }
    a->items[a->len++] = zap_glob_dup(s, strlen(s));
}

/* Join `a`/`b` with a '/', or just `b` when `a` is empty. Caller frees. */
static char *zap_glob_join(const char *a, const char *b) {
    size_t la = strlen(a), lb = strlen(b);
    if (la == 0) {
        return zap_glob_dup(b, lb);
    }
    char *out = zap_glob_dup(a, la + 1 + lb);
    out[la] = '/';
    memcpy(out + la + 1, b, lb);
    out[la + 1 + lb] = '\0';
    return out;
}

static int zap_glob_is_dir(const char *p) {
    struct stat st;
    return stat(p, &st) == 0 && S_ISDIR(st.st_mode);
}

/* True if `p` is a symlink. `**` does not descend through symlinks (matching
 * CPython's `glob`), which also makes a symlink cycle safe. */
static int zap_glob_is_symlink(const char *p) {
#if defined(ZAP_PLATFORM_WINDOWS)
    (void)p;
    return 0; /* MSVC CRT has no lstat; Windows symlinks are rare here */
#else
    struct stat st;
    return lstat(p, &st) == 0 && S_ISLNK(st.st_mode);
#endif
}

/* Match `segs[i..]` against the tree rooted at `dir`; `prefix` is the matched
 * path so far relative to the glob root. `**` matches zero or more directory
 * levels. A leading-dot entry is skipped unless its segment also starts with
 * '.', matching CPython's `glob` dotfile rule. */
static void zap_glob_rec(const char *dir, const char *prefix, char **segs, int nseg,
                         int i, zap_glob_acc *acc) {
    if (i == nseg) {
        if (prefix[0] != '\0') {
            zap_glob_push(acc, prefix);
        }
        return;
    }
    const char *seg = segs[i];
    int is_dstar = strcmp(seg, "**") == 0;
    int last = i == nseg - 1;
    if (is_dstar) {
        /* zero directories: advance past `**` in the same directory */
        zap_glob_rec(dir, prefix, segs, nseg, i + 1, acc);
        /* one or more: descend into each subdirectory, keeping `**` */
        DIR *d = opendir(dir);
        if (!d) {
            return;
        }
        struct dirent *e;
        while ((e = readdir(d)) != NULL) {
            if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
                continue;
            }
            if (e->d_name[0] == '.') {
                continue; /* `**` never descends into dotted directories */
            }
            char *full = zap_glob_join(dir, e->d_name);
            if (zap_glob_is_dir(full) && !zap_glob_is_symlink(full)) {
                char *np = zap_glob_join(prefix, e->d_name);
                zap_glob_rec(full, np, segs, nseg, i, acc);
                free(np);
            }
            free(full);
        }
        closedir(d);
        return;
    }
    DIR *d = opendir(dir);
    if (!d) {
        return; /* unreadable / missing directory: no matches, not an error */
    }
    int seg_dot = seg[0] == '.';
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
            continue;
        }
        if (e->d_name[0] == '.' && !seg_dot) {
            continue; /* dotfile rule */
        }
        if (zap_fnmatch_core(e->d_name, (int64_t)strlen(e->d_name), seg,
                             (int64_t)strlen(seg))) {
            char *np = zap_glob_join(prefix, e->d_name);
            if (last) {
                zap_glob_push(acc, np);
            } else {
                char *full = zap_glob_join(dir, e->d_name);
                if (zap_glob_is_dir(full)) {
                    zap_glob_rec(full, np, segs, nseg, i + 1, acc);
                }
                free(full);
            }
            free(np);
        }
    }
    closedir(d);
}

static int zap_glob_cmp(const void *a, const void *b) {
    return strcmp(*(const char *const *)a, *(const char *const *)b);
}

/* fs.glob(root, pattern) -> list[str]: paths under `root` matching the '/'-split
 * glob `pattern`, returned RELATIVE to `root` and sorted. `*`/`?`/`[seq]` match
 * within a path segment; `**` matches across directory levels. Dotfiles are
 * skipped unless the segment starts with '.' (CPython `glob` semantics). */
void *zap_fs_glob(void *root_, void *pat_) {
    char *root = zap_sys_cstr((ZapStr *)root_);
    const ZapStr *pat = (const ZapStr *)pat_;
    /* Split the pattern into non-empty '/'-separated segments. */
    char *pbuf = zap_glob_dup(pat->data, pat->len);
    char **segs = (char **)malloc((pat->len + 1) * sizeof(char *) + sizeof(char *));
    if (!segs) {
        const char *m = "out of memory";
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    int nseg = 0;
    char *cur = pbuf;
    for (int64_t k = 0; k <= pat->len; k++) {
        if (pbuf[k] == '/' || pbuf[k] == '\0') {
            pbuf[k] = '\0';
            if (*cur != '\0') {
                segs[nseg++] = cur;
            }
            cur = pbuf + k + 1;
        }
    }
    zap_glob_acc acc = {NULL, 0, 0};
    zap_glob_rec(root, "", segs, nseg, 0, &acc);
    if (acc.len > 1) {
        qsort(acc.items, acc.len, sizeof(char *), zap_glob_cmp);
    }
    void *out = zap_list_new();
    for (size_t j = 0; j < acc.len; j++) {
        zap_list_push_ptr(out, zap_str_new(acc.items[j], (int64_t)strlen(acc.items[j])));
        free(acc.items[j]);
    }
    free(acc.items);
    free(segs);
    free(pbuf);
    return out;
}

/* --- sys standard streams (#1035, D-0217) ---------------------------------
 *
 * Stream objects over the standard fds, so a program can pass "the input" or
 * "the output" around as a value instead of a bare fd. `stdin()` reuses the
 * #969 buffered reader over fd 0; `stdout()`/`stderr()` return an opaque writer
 * (a `bytes` handle whose buffer is just the pointer-free fd). Fresh per call —
 * bind the result once (`auto out = sys.stdout()`) rather than re-fetching it in
 * a loop (a fresh `stdin()` reader starts with an empty buffer). */

/* Opaque writer handle: a bytes value whose buffer is the int64 fd. */
static void *zap_sys_writer_new(int64_t fd) {
    ZapStr *h = zap_str_alloc((int64_t)sizeof(int64_t));
    *(int64_t *)(void *)h->data = fd;
    return h;
}

void *zap_sys_stdin(void) {
    return zap_io_reader_new(0);
}
void *zap_sys_stdout(void) {
    return zap_sys_writer_new(1);
}
void *zap_sys_stderr(void) {
    return zap_sys_writer_new(2);
}

/* sys.write(stream, data) -> int64: write `data` to a writer stream, returning
 * the number of bytes written. Raises on an I/O error (via zap_io_write). */
int64_t zap_sys_write(void *w_, void *data_) {
    int64_t fd = *(int64_t *)(void *)((ZapStr *)w_)->data;
    return zap_io_write(fd, data_);
}


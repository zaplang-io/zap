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
#include <spawn.h>    /* posix_spawn: process.run (POSIX)                        */
#include <sys/wait.h> /* waitpid                                                 */
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

/* process.run(argv: list[str]) -> ZapProc*: spawn argv[0] (PATH-searched) with
 * the given arguments, capturing stdout/stderr into temp files (so there is no
 * pipe-buffer deadlock), and wait. The exit code is the child's status, or
 * 128+signo if it was killed by a signal (POSIX). Raises on spawn failure. */
void *zap_process_run(void *argv_list) {
    int64_t n = zap_list_len(argv_list);
    if (n < 1) {
        zap_sys_fail("process.run", "empty argument list");
    }
    char **cargv = (char **)malloc(sizeof(char *) * (size_t)(n + 1));
    if (cargv == NULL) {
        zap_sys_fail("process.run", "out of memory");
    }
    for (int64_t i = 0; i < n; i++) {
        cargv[i] = zap_sys_cstr((ZapStr *)zap_list_get_ptr(argv_list, i));
    }
    cargv[n] = NULL;

    const char *tmp = zap_tempdir();
    char otmpl[4096], etmpl[4096];
    snprintf(otmpl, sizeof otmpl, "%s/zapout_XXXXXX", tmp);
    snprintf(etmpl, sizeof etmpl, "%s/zaperr_XXXXXX", tmp);
#if defined(ZAP_PLATFORM_WINDOWS)
    int ofd = zap_win_mkstemp(otmpl);
#else
    int ofd = mkstemp(otmpl);
#endif
    if (ofd < 0) {
        free(cargv);
        zap_sys_fail("process.run", strerror(errno));
    }
#if defined(ZAP_PLATFORM_WINDOWS)
    int efd = zap_win_mkstemp(etmpl);
#else
    int efd = mkstemp(etmpl);
#endif
    if (efd < 0) {
        int e = errno;
        close(ofd);
        unlink(otmpl);
        free(cargv);
        zap_sys_fail("process.run", strerror(e));
    }

    int64_t code;
#if defined(ZAP_PLATFORM_WINDOWS)
    /* CreateProcessW with stdout/stderr redirected to the temp files. The child
     * inherits the two file handles (marked inheritable); the command line carries
     * argv with MSVCRT quoting, and a NULL application name lets CreateProcess do
     * the PATH search posix_spawnp would (Python-faithful). There is no signal
     * exit status on Windows, so `code` is just the process exit code. */
    char *cmdline = zap_win_build_cmdline(cargv, n);
    free(cargv);
    if (!cmdline) {
        close(ofd);
        close(efd);
        unlink(otmpl);
        unlink(etmpl);
        zap_sys_fail("process.run", "out of memory");
    }
    HANDLE oh = (HANDLE)_get_osfhandle(ofd);
    HANDLE eh = (HANDLE)_get_osfhandle(efd);
    SetHandleInformation(oh, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
    SetHandleInformation(eh, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
    STARTUPINFOA si;
    memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    si.hStdOutput = oh;
    si.hStdError = eh;
    PROCESS_INFORMATION pi;
    memset(&pi, 0, sizeof pi);
    BOOL ok = CreateProcessA(NULL, cmdline, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi);
    free(cmdline);
    if (!ok) {
        close(ofd);
        close(efd);
        unlink(otmpl);
        unlink(etmpl);
        zap_sys_fail("process.run", "CreateProcess failed");
    }
    zap_enter_blocking(); /* GC-safe: no un-flushed Zap temporaries across wait */
    WaitForSingleObject(pi.hProcess, INFINITE);
    zap_leave_blocking();
    DWORD exit_code = 1;
    GetExitCodeProcess(pi.hProcess, &exit_code);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    code = (int64_t)exit_code;
#else
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_adddup2(&fa, ofd, 1);
    posix_spawn_file_actions_adddup2(&fa, efd, 2);
    pid_t pid = 0;
    int rc = posix_spawnp(&pid, cargv[0], &fa, NULL, cargv, environ);
    posix_spawn_file_actions_destroy(&fa);
    free(cargv);
    if (rc != 0) {
        close(ofd);
        close(efd);
        unlink(otmpl);
        unlink(etmpl);
        zap_sys_fail("process.run", strerror(rc));
    }

    int status = 0;
    pid_t w;
    zap_enter_blocking(); /* GC-safe: no un-flushed Zap temporaries across wait */
    do {
        w = waitpid(pid, &status, 0);
    } while (w < 0 && errno == EINTR);
    zap_leave_blocking();

    if (w < 0) {
        code = -1;
    } else if (WIFEXITED(status)) {
        code = (int64_t)WEXITSTATUS(status);
    } else if (WIFSIGNALED(status)) {
        code = 128 + (int64_t)WTERMSIG(status);
    } else {
        code = -1;
    }
#endif

    ZapProc *r = (ZapProc *)zap_obj(&zap_desc_proc);
    r->code = code;
    r->out = NULL;
    r->err = NULL;
    r->out = (ZapStr *)zap_read_all_fd(ofd);
    r->err = (ZapStr *)zap_read_all_fd(efd);
    close(ofd);
    close(efd);
    unlink(otmpl);
    unlink(etmpl);
    return r;
}

/* Field getters the lowerer calls to unpack the result into a tuple. */
int64_t zap_process_result_code(void *r) { return ((ZapProc *)r)->code; }
void *zap_process_result_stdout(void *r) { return ((ZapProc *)r)->out; }
void *zap_process_result_stderr(void *r) { return ((ZapProc *)r)->err; }

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


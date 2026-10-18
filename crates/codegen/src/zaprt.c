/* Minimal Zap runtime (Phase 1).
 *
 * String entry points per design/RUNTIME-ABI.md, and a two-word string object
 * { len, data }. Heap allocation routes through the conservative Boehm
 * collector (ticket #20, D-0009): `GC_MALLOC` for objects that may hold
 * pointers (scanned) and `GC_MALLOC_ATOMIC` for pointer-free byte buffers. The
 * collector is initialized once at startup via a constructor; it scans the
 * stack, registers and globals, so no stack maps or safepoints are needed. The
 * type-descriptor header from the ABI is added when reflection / precise GC
 * land. */
#include <gc.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    int64_t len;
    const char *data;
} ZapStr;

/* Initialize the Boehm collector exactly once, before any Zap code runs. */
__attribute__((constructor)) static void zap_gc_init(void) {
    GC_INIT();
}

/* Allocate `size` zeroed, pointer-scanned bytes via the GC. */
void *zap_alloc(int64_t size) {
    return GC_MALLOC((size_t)size);
}

void *zap_str_new(const char *data, int64_t len) {
    ZapStr *s = (ZapStr *)GC_MALLOC(sizeof(ZapStr));
    char *buf = (char *)GC_MALLOC_ATOMIC((size_t)len);
    if (len > 0) {
        memcpy(buf, data, (size_t)len);
    }
    s->len = len;
    s->data = buf;
    return s;
}

void *zap_str_concat(void *a, void *b) {
    ZapStr *x = (ZapStr *)a;
    ZapStr *y = (ZapStr *)b;
    int64_t n = x->len + y->len;
    char *buf = (char *)GC_MALLOC_ATOMIC((size_t)n);
    memcpy(buf, x->data, (size_t)x->len);
    memcpy(buf + x->len, y->data, (size_t)y->len);
    ZapStr *s = (ZapStr *)GC_MALLOC(sizeof(ZapStr));
    s->len = n;
    s->data = buf;
    return s;
}

void zap_print_str(void *s) {
    ZapStr *z = (ZapStr *)s;
    fwrite(z->data, 1, (size_t)z->len, stdout);
}

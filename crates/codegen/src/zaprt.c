/* Minimal Zap runtime (Phase 1).
 *
 * String entry points per design/RUNTIME-ABI.md. This first cut uses malloc
 * (no GC yet, ticket #10) and a two-word string object { len, data }. The
 * type-descriptor header from the ABI is added when GC/reflection land. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int64_t len;
    const char *data;
} ZapStr;

/* Allocate `size` zeroed bytes. Routed to the GC later (ticket #10). */
void *zap_alloc(int64_t size) {
    return calloc(1, (size_t)size);
}

void *zap_str_new(const char *data, int64_t len) {
    ZapStr *s = (ZapStr *)malloc(sizeof(ZapStr));
    char *buf = (char *)malloc((size_t)len);
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
    char *buf = (char *)malloc((size_t)n);
    memcpy(buf, x->data, (size_t)x->len);
    memcpy(buf + x->len, y->data, (size_t)y->len);
    ZapStr *s = (ZapStr *)malloc(sizeof(ZapStr));
    s->len = n;
    s->data = buf;
    return s;
}

void zap_print_str(void *s) {
    ZapStr *z = (ZapStr *)s;
    fwrite(z->data, 1, (size_t)z->len, stdout);
}

/* zap runtime fragment: bytearray.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (the `ZapStr` / `ZapList` layouts, `zap_obj` / `zap_buf`,
 * `zap_str_new` / `zap_str_adopt`, `zap_panic_typed`, and the slice helpers) and the
 * `zap_desc_bytearray` descriptor + `ZAP_TD_BYTEARRAY` trace case added alongside it.
 * Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * bytearray (ticket #1385, decisions/lang/1385-bytearray.md). The mutable,
 * growable sibling of the immutable `bytes`. Representation R1: a dedicated
 * growable reference object
 *
 *     struct ZapByteArray { int64_t len; int64_t cap; char *data; }
 *
 * whose `data` is a SEPARATE, atomic (pointer-free) leaf buffer — ZapList's
 * amortized-doubling growth model with ZapStr's atomic-buffer discipline. The
 * header maps to `Ty::Ptr` in codegen exactly as `bytes`/`list` do (no new IR
 * scalar); the one new runtime descriptor `zap_desc_bytearray` + the
 * `ZAP_TD_BYTEARRAY` trace case forward only the `data` slot (the buffer is
 * atomic, never pointer-scanned). The header identity is stable across appends
 * (only `data` is reallocated), so every reference observes a mutation.
 *
 * `bytearray(b)` COPIES — the fresh buffer never aliases the source `bytes`, so
 * mutating the result never writes the source (pinned by a golden test).
 * =========================================================================== */

typedef struct {
    int64_t len;  /* number of live bytes                                     */
    int64_t cap;  /* capacity of `data` in bytes                              */
    char *data;   /* `cap` bytes, atomic (pointer-free) leaf; may be NULL     */
} ZapByteArray;

/* Grow `a` so it can hold at least `need` bytes, ZapList-style: the new capacity
 * is `cap < 8 ? 8 : cap*2`, bumped further until it covers `need`. A FRESH atomic
 * leaf buffer is allocated (never C `realloc` — mirrors `zap_list_grow`), the
 * live bytes are copied over, and the `data` slot is swapped. `a->data` is read
 * back from the (pinned) header AFTER the allocation, so a moving collection
 * during the alloc that relocated the old buffer has already rewritten the slot.
 */
static void zap_bytearray_grow(ZapByteArray *a, int64_t need) {
    int64_t cap = a->cap < 8 ? 8 : a->cap * 2;
    while (cap < need) {
        cap *= 2;
    }
    char *data = (char *)zap_buf(cap);
    if (a->len > 0) {
        memcpy(data, a->data, (size_t)a->len);
    }
    a->data = data;
    a->cap = cap;
}

/* Reserve space for `n` bytes, growing if the current capacity is short. */
static void zap_bytearray_reserve(ZapByteArray *a, int64_t n) {
    if (n > a->cap) {
        zap_bytearray_grow(a, n);
    }
}

/* bytearray() — a fresh empty buffer (len 0, cap 0, data NULL). */
void *zap_bytearray_new(void) {
    ZapByteArray *a = (ZapByteArray *)zap_obj(&zap_desc_bytearray);
    a->len = 0;
    a->cap = 0;
    a->data = NULL;
    return a;
}

/* bytearray(int n) — `n` zero-filled bytes (n < 0 -> catchable ValueError, like
 * CPython; `bytes(n)` faults the same way, #1386). */
void *zap_bytearray_with_len(int64_t n) {
    if (n < 0) {
        const char *m = "bytearray(): negative count";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    ZapByteArray *a = (ZapByteArray *)zap_bytearray_new();
    if (n > 0) {
        a->data = (char *)zap_buf(n);
        memset(a->data, 0, (size_t)n);
        a->cap = n;
        a->len = n;
    }
    return a;
}

/* Append `src[0..n)` to `a`, growing once if needed. `src` is read into a fresh
 * buffer by `grow` before the `data` slot is swapped, so a `src` that aliased
 * `a->data` would still be read correctly; in practice callers never alias. */
static void zap_bytearray_append_raw(ZapByteArray *a, const char *src, int64_t n) {
    if (n <= 0) {
        return;
    }
    zap_bytearray_reserve(a, a->len + n);
    memcpy(a->data + a->len, src, (size_t)n);
    a->len += n;
}

/* bytearray(bytes b) — a COPY of `b`'s bytes into a fresh atomic buffer. The
 * result never aliases the source `ZapStr`, so later mutation leaves `b`
 * untouched (the no-aliasing guarantee). */
void *zap_bytearray_from_bytes(void *bp) {
    const ZapStr *b = (const ZapStr *)bp;
    ZapByteArray *a = (ZapByteArray *)zap_bytearray_new();
    zap_bytearray_append_raw(a, b->data, b->len);
    return a;
}

/* bytearray(str s) — the UTF-8 bytes of `s` (its `ZapStr` payload), copied into a
 * fresh buffer. `str` and `bytes` share the `ZapStr` layout, so this matches the
 * bare `bytes(str)` UTF-8 encode (#337), then owns a mutable copy. */
void *zap_bytearray_from_str(void *sp) { return zap_bytearray_from_bytes(sp); }

/* bytearray(bytearray other) — a COPY (distinct header + distinct buffer). */
void *zap_bytearray_from_ba(void *op) {
    const ZapByteArray *o = (const ZapByteArray *)op;
    ZapByteArray *a = (ZapByteArray *)zap_bytearray_new();
    zap_bytearray_append_raw(a, o->data, o->len);
    return a;
}

/* bytearray(list[int]) — one byte per element, each validated to lie in 0..=255
 * (out of range -> catchable ValueError, like CPython). */
void *zap_bytearray_from_list(void *lp) {
    ZapList *l = (ZapList *)lp;
    int64_t n = l->len;
    ZapByteArray *a = (ZapByteArray *)zap_bytearray_new();
    zap_bytearray_reserve(a, n);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = l->data[i];
        if (v < 0 || v > 255) {
            const char *m = "bytearray(): list element out of range(0..256)";
            zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
        }
        a->data[a->len++] = (char)(unsigned char)v;
    }
    return a;
}

int64_t zap_bytearray_len(void *ap) { return ((ZapByteArray *)ap)->len; }

/* Normalize a (possibly negative) index and bounds-check it against `len`.
 * Negative indices count from the end (`a[-1]`); out of range raises a catchable
 * IndexError (#383, matches list/bytes indexing). Returns the resolved
 * non-negative index. */
static int64_t zap_bytearray_norm(ZapByteArray *a, int64_t idx) {
    int64_t n = a->len;
    if (idx < 0) {
        idx += n;
    }
    if (idx < 0 || idx >= n) {
        const char *m = "bytearray index out of range";
        zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    return idx;
}

/* a[i] -> the byte at index `i` as an int64 in 0..=255 (unsigned, so 0x00/0xFF
 * read back as 0/255). */
int64_t zap_bytearray_get(void *ap, int64_t idx) {
    ZapByteArray *a = (ZapByteArray *)ap;
    int64_t i = zap_bytearray_norm(a, idx);
    return (int64_t)(unsigned char)a->data[i];
}

/* a[i] = v in place. Out-of-range index -> IndexError; a value outside 0..=255
 * -> catchable ValueError (CPython raises ValueError for both of these). */
void zap_bytearray_set(void *ap, int64_t idx, int64_t v) {
    ZapByteArray *a = (ZapByteArray *)ap;
    int64_t i = zap_bytearray_norm(a, idx);
    if (v < 0 || v > 255) {
        const char *m = "byte must be in range(0, 256)";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    a->data[i] = (char)(unsigned char)v;
}

/* a.append(v) — append one byte; `v` outside 0..=255 -> catchable ValueError. */
void zap_bytearray_append(void *ap, int64_t v) {
    ZapByteArray *a = (ZapByteArray *)ap;
    if (v < 0 || v > 255) {
        const char *m = "byte must be in range(0, 256)";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    zap_bytearray_reserve(a, a->len + 1);
    a->data[a->len++] = (char)(unsigned char)v;
}

/* a.extend(bytes) / a += bytes — append every byte of a `ZapStr`. */
void zap_bytearray_extend_bytes(void *ap, void *bp) {
    ZapByteArray *a = (ZapByteArray *)ap;
    const ZapStr *b = (const ZapStr *)bp;
    zap_bytearray_append_raw(a, b->data, b->len);
}

/* a.extend(bytearray) / a += bytearray — append every byte of another
 * bytearray. Safe even when `b` IS `a` (self-extend): the live bytes are read
 * through `a->data`, and `grow` copies into a fresh buffer before swapping, so
 * no read-after-free of the old buffer occurs. */
void zap_bytearray_extend_ba(void *ap, void *bp) {
    ZapByteArray *a = (ZapByteArray *)ap;
    ZapByteArray *b = (ZapByteArray *)bp;
    int64_t n = b->len;
    if (n <= 0) {
        return;
    }
    zap_bytearray_reserve(a, a->len + n);
    /* Re-read b->data AFTER the reserve (a self-extend reallocated it). */
    memcpy(a->data + a->len, b->data, (size_t)n);
    a->len += n;
}

/* a.extend(list[int]) / a += list[int] — append one byte per element, each
 * validated to lie in 0..=255 (out of range -> catchable ValueError). */
void zap_bytearray_extend_list(void *ap, void *lp) {
    ZapByteArray *a = (ZapByteArray *)ap;
    ZapList *l = (ZapList *)lp;
    int64_t n = l->len;
    zap_bytearray_reserve(a, a->len + n);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = l->data[i];
        if (v < 0 || v > 255) {
            const char *m = "bytearray.extend(): list element out of range(0..256)";
            zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
        }
        a->data[a->len++] = (char)(unsigned char)v;
    }
}

/* a + b (both bytearray) -> a NEW bytearray holding their concatenation; neither
 * operand is mutated. */
void *zap_bytearray_concat(void *ap, void *bp) {
    ZapByteArray *a = (ZapByteArray *)ap;
    ZapByteArray *b = (ZapByteArray *)bp;
    ZapByteArray *out = (ZapByteArray *)zap_bytearray_new();
    zap_bytearray_reserve(out, a->len + b->len);
    if (a->len > 0) {
        memcpy(out->data, a->data, (size_t)a->len);
    }
    if (b->len > 0) {
        memcpy(out->data + a->len, b->data, (size_t)b->len);
    }
    out->len = a->len + b->len;
    return out;
}

/* a + b where exactly one operand is `bytes` -> a NEW bytearray. `ap`/`bp` carry
 * the ZapByteArray and ZapStr in operand order (`a_is_ba` picks which is which),
 * so `ba + bytes` and `bytes + ba` both route here with the right ordering. */
void *zap_bytearray_concat_bytes(void *ap, void *bp, int64_t a_is_ba) {
    ZapByteArray *ba = (ZapByteArray *)(a_is_ba ? ap : bp);
    const ZapStr *bs = (const ZapStr *)(a_is_ba ? bp : ap);
    ZapByteArray *out = (ZapByteArray *)zap_bytearray_new();
    int64_t ln = a_is_ba ? ba->len : bs->len;
    int64_t rn = a_is_ba ? bs->len : ba->len;
    const char *ld = a_is_ba ? ba->data : bs->data;
    const char *rd = a_is_ba ? bs->data : ba->data;
    zap_bytearray_reserve(out, ln + rn);
    if (ln > 0) {
        memcpy(out->data, ld, (size_t)ln);
    }
    if (rn > 0) {
        memcpy(out->data + ln, rd, (size_t)rn);
    }
    out->len = ln + rn;
    return out;
}

/* bytes(bytearray) -> a fresh IMMUTABLE `bytes` (a copy over the ZapStr layout);
 * later mutation of the source bytearray never writes the returned bytes. */
void *zap_bytearray_to_bytes(void *ap) {
    ZapByteArray *a = (ZapByteArray *)ap;
    return zap_str_new(a->data, a->len);
}

/* a == b (both bytearray) -> 1 iff equal length and bytes, else 0. */
int64_t zap_bytearray_eq(void *ap, void *bp) {
    ZapByteArray *a = (ZapByteArray *)ap;
    ZapByteArray *b = (ZapByteArray *)bp;
    if (a->len != b->len) {
        return 0;
    }
    return a->len == 0 || memcmp(a->data, b->data, (size_t)a->len) == 0 ? 1 : 0;
}

/* a == b where `a` is a bytearray and `b` is `bytes` -> content equality. */
int64_t zap_bytearray_eq_bytes(void *ap, void *bp) {
    ZapByteArray *a = (ZapByteArray *)ap;
    const ZapStr *b = (const ZapStr *)bp;
    if (a->len != b->len) {
        return 0;
    }
    return a->len == 0 || memcmp(a->data, b->data, (size_t)a->len) == 0 ? 1 : 0;
}

/* a[lo:hi:step] -> a NEW bytearray, Python slice semantics by byte (mirrors
 * `zap_bytes_slice`, but materializes a mutable bytearray). A zero step raises a
 * catchable ValueError. */
void *zap_bytearray_slice(void *ap, int64_t lo, int64_t hi, int64_t step,
                          int64_t has_lo, int64_t has_hi, int64_t has_step) {
    ZapByteArray *a = (ZapByteArray *)ap;
    int64_t n = a->len;
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        const char *m = "slice step cannot be zero";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    int64_t start;
    int64_t stop;
    if (st > 0) {
        start = has_lo ? zap_slice_pos(lo, n) : 0;
        stop = has_hi ? zap_slice_pos(hi, n) : n;
    } else {
        start = has_lo ? zap_slice_neg(lo, n) : n - 1;
        stop = has_hi ? zap_slice_neg(hi, n) : -1;
    }
    ZapByteArray *out = (ZapByteArray *)zap_bytearray_new();
    if (st == 1) {
        if (start < stop) {
            zap_bytearray_append_raw(out, a->data + start, stop - start);
        }
        return out;
    }
    zap_bytearray_reserve(out, n > 0 ? n : 1);
    for (int64_t k = start; (st > 0) ? (k < stop) : (k > stop); k += st) {
        out->data[out->len++] = a->data[k];
    }
    return out;
}

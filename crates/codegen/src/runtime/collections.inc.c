/* zap runtime fragment: collections.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * stdlib: collections extras — deque / bisect / oset (ticket #102, D-0056)
 * ---------------------------------------------------------------------------
 * These build on the existing collection machinery (#21/#38). Elements are i64
 * words (ptr/str element kinds deferred to a follow-up). The module surface is
 * the opaque-handle pattern (like crypto #105): a `_new` returns a GC handle,
 * free functions operate on it; sema types the handle as `bytes` and lowering
 * passes it as a pointer.
 *
 * deque: a double-ended ring buffer with O(1) push/pop at both ends and an
 *   optional bounded `maxlen` (Python collections.deque). The header is a
 *   PRECISE GC object whose single traced field is a pointer-free i64 leaf
 *   buffer (zap_buf) — so the precise collector keeps the buffer alive
 *   generically via `ptr_offsets`, with no zapgc.inc.c change.
 * bisect: pure binary search / sorted-insert over the existing ZapList (i64).
 *   No new GC type.
 * oset (insertion-ordered set): composed from an existing list (insertion
 *   order) + set (O(1) membership); a PRECISE 2-pointer header. Both pointees
 *   are ordinary GC objects that trace themselves.
 *
 * Out-of-range access / pop-from-empty aborts, matching list's no-exception-
 * runtime convention (ticket #22).
 * ========================================================================== */

/* ---- deque: double-ended ring buffer of element words (#983) -------------
 * Generic over the element kind (i64 / pointer / float64). The ring stores raw
 * words; pointer-element deques are traced precisely by the ZAP_TD_DEQUE GC arm,
 * so the backing buffer is always allocated with `zap_ptrbuf` (conservatively
 * scanned under Boehm, a precisely-traced leaf under Immix) — identical to
 * `list`/`heap`. The `ZapDeque` struct and `DEQUE_ELEM_*` kinds are declared
 * near `ZapChan` above so the collector sees the layout. */
static const ZapTypeDesc zap_desc_deque = {(int64_t)sizeof(ZapDeque), ZAP_TD_DEQUE, 0, NULL};

void *zap_deque_new(int64_t kind) {
    ZapDeque *d = (ZapDeque *)zap_obj(&zap_desc_deque);
    d->maxlen = -1; /* unbounded; data/cap/head/len are zeroed by the allocator */
    d->kind = kind;
    return d;
}

/* Bounded deque: a fixed `maxlen`-slot ring. A negative bound is treated as 0. */
void *zap_deque_bounded(int64_t maxlen, int64_t kind) {
    ZapDeque *d = (ZapDeque *)zap_obj(&zap_desc_deque);
    d->maxlen = maxlen < 0 ? 0 : maxlen;
    d->cap = d->maxlen;
    d->kind = kind;
    if (d->cap > 0) {
        d->data = (int64_t *)zap_ptrbuf(d->cap * (int64_t)sizeof(int64_t));
    }
    return d;
}

/* Grow an unbounded deque, re-linearizing the ring into a fresh doubled buffer
 * (head resets to 0). Only ever called for the unbounded path. */
static void zap_deque_grow(ZapDeque *d) {
    int64_t cap = d->cap < 4 ? 4 : d->cap * 2;
    int64_t *data = (int64_t *)zap_ptrbuf(cap * (int64_t)sizeof(int64_t));
    for (int64_t i = 0; i < d->len; i++) {
        data[i] = d->data[(d->head + i) % d->cap];
    }
    d->data = data;
    d->cap = cap;
    d->head = 0;
}

/* --- core ring ops over element words (kind-agnostic) --------------------- */
static void zap_deque_append_word(ZapDeque *d, int64_t v) {
    if (d->maxlen == 0) {
        return; /* a zero-bounded deque silently discards */
    }
    if (d->maxlen > 0 && d->len == d->maxlen) {
        d->head = (d->head + 1) % d->cap; /* full: drop the leftmost */
        d->len--;
    }
    if (d->len == d->cap) {
        zap_deque_grow(d);
    }
    d->data[(d->head + d->len) % d->cap] = v;
    d->len++;
}

static void zap_deque_appendleft_word(ZapDeque *d, int64_t v) {
    if (d->maxlen == 0) {
        return;
    }
    if (d->maxlen > 0 && d->len == d->maxlen) {
        d->len--; /* full: drop the rightmost */
    }
    if (d->len == d->cap) {
        zap_deque_grow(d);
    }
    d->head = (d->head - 1 + d->cap) % d->cap;
    d->data[d->head] = v;
    d->len++;
}

static int64_t zap_deque_pop_word(ZapDeque *d) {
    if (d->len == 0) {
        const char *m = "pop from an empty deque";
        zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    int64_t v = d->data[(d->head + d->len - 1) % d->cap];
    d->len--;
    return v;
}

static int64_t zap_deque_popleft_word(ZapDeque *d) {
    if (d->len == 0) {
        const char *m = "pop from an empty deque";
        zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    int64_t v = d->data[d->head];
    d->head = (d->head + 1) % d->cap;
    d->len--;
    return v;
}

static int64_t zap_deque_get_word(ZapDeque *d, int64_t i) {
    if (i < 0 || i >= d->len) {
        char buf[96];
        int n = snprintf(buf, sizeof(buf), "deque index %lld out of range (len %lld)",
                         (long long)i, (long long)d->len);
        if (n < 0) {
            n = 0;
        }
        if (n > (int)sizeof(buf) - 1) {
            n = (int)sizeof(buf) - 1;
        }
        zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new(buf, (int64_t)n));
    }
    return d->data[(d->head + i) % d->cap];
}

/* --- kind-typed entry points --------------------------------------------- */
void zap_deque_append_i64(void *dp, int64_t v) { zap_deque_append_word((ZapDeque *)dp, v); }
void zap_deque_append_ptr(void *dp, void *v) {
    zap_deque_append_word((ZapDeque *)dp, (int64_t)(intptr_t)v);
}
void zap_deque_append_f64(void *dp, double v) {
    zap_deque_append_word((ZapDeque *)dp, zap_f64_bits(v));
}
void zap_deque_appendleft_i64(void *dp, int64_t v) { zap_deque_appendleft_word((ZapDeque *)dp, v); }
void zap_deque_appendleft_ptr(void *dp, void *v) {
    zap_deque_appendleft_word((ZapDeque *)dp, (int64_t)(intptr_t)v);
}
void zap_deque_appendleft_f64(void *dp, double v) {
    zap_deque_appendleft_word((ZapDeque *)dp, zap_f64_bits(v));
}

int64_t zap_deque_pop_i64(void *dp) { return zap_deque_pop_word((ZapDeque *)dp); }
void *zap_deque_pop_ptr(void *dp) { return (void *)(intptr_t)zap_deque_pop_word((ZapDeque *)dp); }
double zap_deque_pop_f64(void *dp) { return zap_f64_from_bits(zap_deque_pop_word((ZapDeque *)dp)); }
int64_t zap_deque_popleft_i64(void *dp) { return zap_deque_popleft_word((ZapDeque *)dp); }
void *zap_deque_popleft_ptr(void *dp) {
    return (void *)(intptr_t)zap_deque_popleft_word((ZapDeque *)dp);
}
double zap_deque_popleft_f64(void *dp) {
    return zap_f64_from_bits(zap_deque_popleft_word((ZapDeque *)dp));
}

int64_t zap_deque_len(void *dp) { return ((ZapDeque *)dp)->len; }

/* Index from the left (0 == leftmost). */
int64_t zap_deque_get_i64(void *dp, int64_t i) { return zap_deque_get_word((ZapDeque *)dp, i); }
void *zap_deque_get_ptr(void *dp, int64_t i) {
    return (void *)(intptr_t)zap_deque_get_word((ZapDeque *)dp, i);
}
double zap_deque_get_f64(void *dp, int64_t i) {
    return zap_f64_from_bits(zap_deque_get_word((ZapDeque *)dp, i));
}

void zap_deque_clear(void *dp) {
    ZapDeque *d = (ZapDeque *)dp;
    d->head = 0;
    d->len = 0;
}

/* Snapshot the elements left-to-right into a fresh `list[elem]` for `for x in d`
 * (model on zap_set_items / zap_heap_items). */
void *zap_deque_items(void *dp) {
    ZapDeque *d = (ZapDeque *)dp;
    void *out = zap_list_new();
    for (int64_t i = 0; i < d->len; i++) {
        int64_t w = d->data[(d->head + i) % d->cap];
        if (d->kind == DEQUE_ELEM_PTR) {
            zap_list_push_ptr(out, (void *)(intptr_t)w);
        } else if (d->kind == DEQUE_ELEM_F64) {
            zap_list_push_f64(out, zap_f64_from_bits(w));
        } else {
            zap_list_push_i64(out, w);
        }
    }
    return out;
}

/* ---- bisect: binary search / sorted insert over a sorted ZapList[T] ------
 * Generic over any `Comparable` element (#983): the list stores element words
 * and `kind` (BISECT_ELEM_*) selects the ordering — integer, lexical string, or
 * IEEE float total order — reusing the same comparators as `heap`. */
enum { BISECT_ELEM_I64 = 0, BISECT_ELEM_STR = 1, BISECT_ELEM_F64 = 2 };

/* Three-way compare of two element words under the bisect ordering. */
static int64_t zap_bisect_cmp(int64_t a, int64_t b, int64_t kind) {
    if (kind == BISECT_ELEM_STR) {
        return zap_str_cmp((void *)(intptr_t)a, (void *)(intptr_t)b);
    } else if (kind == BISECT_ELEM_F64) {
        return zap_f64_total_cmp(zap_f64_from_bits(a), zap_f64_from_bits(b));
    }
    return (a < b) ? -1 : (a > b ? 1 : 0);
}

static int64_t zap_bisect_left_impl(ZapList *l, int64_t x, int64_t kind) {
    int64_t lo = 0, hi = l->len;
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        if (zap_bisect_cmp(l->data[mid], x, kind) < 0) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    return lo;
}

static int64_t zap_bisect_right_impl(ZapList *l, int64_t x, int64_t kind) {
    int64_t lo = 0, hi = l->len;
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        if (zap_bisect_cmp(x, l->data[mid], kind) < 0) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    return lo;
}

/* Insert word `v` at `idx`, shifting the tail right by one (growing if full). */
static void zap_list_insert_at(ZapList *l, int64_t idx, int64_t v) {
    if (l->len == l->cap) {
        zap_list_grow(l);
    }
    if (idx < l->len) {
        memmove(&l->data[idx + 1], &l->data[idx], (size_t)(l->len - idx) * sizeof(int64_t));
    }
    l->data[idx] = v;
    l->len++;
}

/* Element-typed entry points (#983). Each reinterprets the search value to the
 * element word and selects the matching comparator kind, mirroring `heap`'s
 * `_i64` / `_ptr` / `_f64` suffixing (a `str` element is a `ptr` word compared by
 * bytes; an `f64` element is a reinterpreted double ordered by total order). */
int64_t zap_bisect_left_i64(void *l, int64_t x) {
    return zap_bisect_left_impl((ZapList *)l, x, BISECT_ELEM_I64);
}
int64_t zap_bisect_left_ptr(void *l, void *x) {
    return zap_bisect_left_impl((ZapList *)l, (int64_t)(intptr_t)x, BISECT_ELEM_STR);
}
int64_t zap_bisect_left_f64(void *l, double x) {
    return zap_bisect_left_impl((ZapList *)l, zap_f64_bits(x), BISECT_ELEM_F64);
}
int64_t zap_bisect_right_i64(void *l, int64_t x) {
    return zap_bisect_right_impl((ZapList *)l, x, BISECT_ELEM_I64);
}
int64_t zap_bisect_right_ptr(void *l, void *x) {
    return zap_bisect_right_impl((ZapList *)l, (int64_t)(intptr_t)x, BISECT_ELEM_STR);
}
int64_t zap_bisect_right_f64(void *l, double x) {
    return zap_bisect_right_impl((ZapList *)l, zap_f64_bits(x), BISECT_ELEM_F64);
}

void zap_bisect_insort_left_i64(void *l, int64_t x) {
    ZapList *list = (ZapList *)l;
    zap_list_insert_at(list, zap_bisect_left_impl(list, x, BISECT_ELEM_I64), x);
}
void zap_bisect_insort_left_ptr(void *l, void *x) {
    ZapList *list = (ZapList *)l;
    int64_t w = (int64_t)(intptr_t)x;
    zap_list_insert_at(list, zap_bisect_left_impl(list, w, BISECT_ELEM_STR), w);
}
void zap_bisect_insort_left_f64(void *l, double x) {
    ZapList *list = (ZapList *)l;
    int64_t w = zap_f64_bits(x);
    zap_list_insert_at(list, zap_bisect_left_impl(list, w, BISECT_ELEM_F64), w);
}
void zap_bisect_insort_right_i64(void *l, int64_t x) {
    ZapList *list = (ZapList *)l;
    zap_list_insert_at(list, zap_bisect_right_impl(list, x, BISECT_ELEM_I64), x);
}
void zap_bisect_insort_right_ptr(void *l, void *x) {
    ZapList *list = (ZapList *)l;
    int64_t w = (int64_t)(intptr_t)x;
    zap_list_insert_at(list, zap_bisect_right_impl(list, w, BISECT_ELEM_STR), w);
}
void zap_bisect_insort_right_f64(void *l, double x) {
    ZapList *list = (ZapList *)l;
    int64_t w = zap_f64_bits(x);
    zap_list_insert_at(list, zap_bisect_right_impl(list, w, BISECT_ELEM_F64), w);
}

/* ---- oset: insertion-ordered set = list (order) + set (membership) -------
 * Generic over any `Hashable` element (#983): i64 or pointer/string (floats are
 * not Hashable, as for `set`). `kind` selects the backing set/list entry points;
 * the backing containers set their own `elem_ptr` and so are GC-traced correctly
 * for pointer elements, so `ZapOset` itself stays a plain PRECISE two-pointer
 * struct (no new GC arm — the `order`/`member` sub-containers trace elements). */
enum { OSET_ELEM_I64 = 0, OSET_ELEM_PTR = 1 };
typedef struct {
    void *order;   /* ZapList* — elements in insertion order */
    void *member;  /* ZapSet*  — O(1) membership test        */
    int64_t kind;  /* OSET_ELEM_I64 | OSET_ELEM_PTR           */
} ZapOset;

static const int32_t zap_off_oset[] = {(int32_t)offsetof(ZapOset, order),
                                       (int32_t)offsetof(ZapOset, member)};
static const ZapTypeDesc zap_desc_oset = {(int64_t)sizeof(ZapOset), ZAP_TD_PRECISE, 2,
                                          zap_off_oset};

void *zap_oset_new(int64_t kind) {
    ZapOset *s = (ZapOset *)zap_obj(&zap_desc_oset);
    s->kind = kind;
    /* order/member are NULL until set; the allocator zeroed them, so a GC
     * triggered inside zap_list_new()/zap_set_new() traces NULL safely. */
    s->order = zap_list_new();
    s->member = zap_set_new();
    return s;
}

static int64_t zap_oset_contains_word(ZapOset *s, int64_t v) {
    if (s->kind == OSET_ELEM_PTR) {
        return zap_set_contains_str(s->member, (void *)(intptr_t)v);
    }
    return zap_set_contains_i64(s->member, v);
}

static void zap_oset_add_word(ZapOset *s, int64_t v) {
    if (zap_oset_contains_word(s, v)) {
        return;
    }
    if (s->kind == OSET_ELEM_PTR) {
        zap_set_add_str(s->member, (void *)(intptr_t)v);
        zap_list_push_ptr(s->order, (void *)(intptr_t)v);
    } else {
        zap_set_add_i64(s->member, v);
        zap_list_push_i64(s->order, v);
    }
}

void zap_oset_add_i64(void *sp, int64_t v) { zap_oset_add_word((ZapOset *)sp, v); }
void zap_oset_add_ptr(void *sp, void *v) {
    zap_oset_add_word((ZapOset *)sp, (int64_t)(intptr_t)v);
}

int64_t zap_oset_contains_i64(void *sp, int64_t v) {
    return zap_oset_contains_word((ZapOset *)sp, v);
}
int64_t zap_oset_contains_ptr(void *sp, void *v) {
    return zap_oset_contains_word((ZapOset *)sp, (int64_t)(intptr_t)v);
}

int64_t zap_oset_len(void *sp) { return zap_list_len(((ZapOset *)sp)->order); }

int64_t zap_oset_get_i64(void *sp, int64_t i) {
    return zap_list_get_i64(((ZapOset *)sp)->order, i);
}
void *zap_oset_get_ptr(void *sp, int64_t i) {
    return zap_list_get_ptr(((ZapOset *)sp)->order, i);
}

/* Snapshot the elements in insertion order into a fresh `list[elem]` for
 * `for x in s` (model on zap_set_items). */
void *zap_oset_items(void *sp) {
    ZapOset *s = (ZapOset *)sp;
    int64_t n = zap_list_len(s->order);
    void *out = zap_list_new();
    for (int64_t i = 0; i < n; i++) {
        if (s->kind == OSET_ELEM_PTR) {
            zap_list_push_ptr(out, zap_list_get_ptr(s->order, i));
        } else {
            zap_list_push_i64(out, zap_list_get_i64(s->order, i));
        }
    }
    return out;
}


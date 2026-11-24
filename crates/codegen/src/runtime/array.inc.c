/* zap runtime fragment: array.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * Numeric-array sort / argsort — own floor (#932, D-0190)
 *
 * NumPy-compatible `a.sort()` / `sort(a)` / `a.argsort()` / `argsort(a)` with
 * `axis=` and a `kind` split: UNSTABLE (own introsort — median-of-3 quicksort
 * with a heapsort depth guard + insertion base case) is the default; STABLE is an
 * LSD radix for integer dtypes and a bottom-up mergesort for floats. NaNs sort to
 * the end and -0.0 == +0.0 (NumPy's order). Values match NumPy for every kind;
 * argsort tie-order matches NumPy only for the stable kind (ties keep input
 * order). A unified core computes a permutation of each 1-D lane; value sort
 * applies it (gather), argsort emits it. Lanes are gathered stride-aware, so a
 * slice view sorts correctly (and in place aliases its parent). Verified
 * bit-for-bit against NumPy. NOT `list.sort` (the boxed general path). Complex is
 * rejected upstream (no total order here). ======================================= */

/* total-order float compare: NaN greatest (to the end), -0.0 == +0.0. */
static int zap_asort_fcmp(double x, double y) {
    if (x < y) return -1;
    if (x > y) return 1;
    int nx = isnan(x), ny = isnan(y);
    if (nx && ny) return 0;
    if (nx) return 1;
    if (ny) return -1;
    return 0;
}
static double zap_asort_rdf(const void *b, int64_t k, int64_t dt) {
    return dt == DTYPE_F64 ? ((const double *)b)[k] : (double)((const float *)b)[k];
}
/* compare elements i,j of a contiguous typed lane buffer by dtype. */
static int zap_asort_cmp(const void *b, int64_t i, int64_t j, int64_t dt) {
    switch (dt) {
    case DTYPE_F64:
    case DTYPE_F32:
        return zap_asort_fcmp(zap_asort_rdf(b, i, dt), zap_asort_rdf(b, j, dt));
    case DTYPE_I64: {
        int64_t a = ((const int64_t *)b)[i], c = ((const int64_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    case DTYPE_I32: {
        int32_t a = ((const int32_t *)b)[i], c = ((const int32_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    case DTYPE_I16: {
        int16_t a = ((const int16_t *)b)[i], c = ((const int16_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    case DTYPE_I8: {
        int8_t a = ((const int8_t *)b)[i], c = ((const int8_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    case DTYPE_U64: {
        uint64_t a = ((const uint64_t *)b)[i], c = ((const uint64_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    case DTYPE_U32: {
        uint32_t a = ((const uint32_t *)b)[i], c = ((const uint32_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    case DTYPE_U16: {
        uint16_t a = ((const uint16_t *)b)[i], c = ((const uint16_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    default: { /* U8 / BOOL */
        uint8_t a = ((const uint8_t *)b)[i], c = ((const uint8_t *)b)[j];
        return a < c ? -1 : a > c ? 1 : 0;
    }
    }
}
/* order-preserving uint64 radix key for an integer dtype element. */
static uint64_t zap_asort_ikey(const void *b, int64_t k, int64_t dt) {
    switch (dt) {
    case DTYPE_I64: return (uint64_t)((const int64_t *)b)[k] ^ 0x8000000000000000ULL;
    case DTYPE_I32: return (uint64_t)(int64_t)((const int32_t *)b)[k] ^ 0x8000000000000000ULL;
    case DTYPE_I16: return (uint64_t)(int64_t)((const int16_t *)b)[k] ^ 0x8000000000000000ULL;
    case DTYPE_I8:  return (uint64_t)(int64_t)((const int8_t *)b)[k] ^ 0x8000000000000000ULL;
    case DTYPE_U64: return ((const uint64_t *)b)[k];
    case DTYPE_U32: return ((const uint32_t *)b)[k];
    case DTYPE_U16: return ((const uint16_t *)b)[k];
    default:        return ((const uint8_t *)b)[k]; /* U8 / BOOL */
    }
}
static void zap_asort_swap(int64_t *p, int64_t a, int64_t b) {
    int64_t t = p[a]; p[a] = p[b]; p[b] = t;
}
static void zap_asort_insertion(int64_t *p, int64_t lo, int64_t hi, const void *b, int64_t dt) {
    for (int64_t i = lo + 1; i <= hi; i++) {
        int64_t v = p[i], j = i - 1;
        while (j >= lo && zap_asort_cmp(b, p[j], v, dt) > 0) { p[j + 1] = p[j]; j--; }
        p[j + 1] = v;
    }
}
static void zap_asort_siftdown(int64_t *p, int64_t lo, int64_t n, int64_t i, const void *b, int64_t dt) {
    for (;;) {
        int64_t l = 2 * i + 1, r = l + 1, m = i;
        if (l < n && zap_asort_cmp(b, p[lo + l], p[lo + m], dt) > 0) m = l;
        if (r < n && zap_asort_cmp(b, p[lo + r], p[lo + m], dt) > 0) m = r;
        if (m == i) break;
        zap_asort_swap(p, lo + i, lo + m);
        i = m;
    }
}
static void zap_asort_heap(int64_t *p, int64_t lo, int64_t hi, const void *b, int64_t dt) {
    int64_t n = hi - lo + 1;
    for (int64_t i = n / 2 - 1; i >= 0; i--) zap_asort_siftdown(p, lo, n, i, b, dt);
    for (int64_t e = n - 1; e > 0; e--) { zap_asort_swap(p, lo, lo + e); zap_asort_siftdown(p, lo, e, 0, b, dt); }
}
/* introsort on the index permutation p[lo..hi] (unstable). */
static void zap_asort_intro(int64_t *p, int64_t lo, int64_t hi, int depth, const void *b, int64_t dt) {
    while (hi - lo > 16) {
        if (depth == 0) { zap_asort_heap(p, lo, hi, b, dt); return; }
        depth--;
        int64_t mid = lo + (hi - lo) / 2;
        if (zap_asort_cmp(b, p[mid], p[lo], dt) < 0) zap_asort_swap(p, lo, mid);
        if (zap_asort_cmp(b, p[hi], p[lo], dt) < 0) zap_asort_swap(p, lo, hi);
        if (zap_asort_cmp(b, p[hi], p[mid], dt) < 0) zap_asort_swap(p, mid, hi);
        zap_asort_swap(p, mid, hi - 1);
        int64_t pivot = p[hi - 1], i = lo, j = hi - 1;
        for (;;) {
            while (zap_asort_cmp(b, p[++i], pivot, dt) < 0) {}
            while (zap_asort_cmp(b, pivot, p[--j], dt) < 0) {}
            if (i >= j) break;
            zap_asort_swap(p, i, j);
        }
        zap_asort_swap(p, i, hi - 1);
        if (i - lo < hi - i) { zap_asort_intro(p, lo, i - 1, depth, b, dt); lo = i + 1; }
        else { zap_asort_intro(p, i + 1, hi, depth, b, dt); hi = i - 1; }
    }
    zap_asort_insertion(p, lo, hi, b, dt);
}
/* stable bottom-up mergesort on the permutation p[0..n) by comparator. */
static void zap_asort_merge(int64_t *p, int64_t n, const void *b, int64_t dt) {
    int64_t *tmp = (int64_t *)malloc((size_t)n * sizeof(int64_t));
    for (int64_t w = 1; w < n; w *= 2)
        for (int64_t lo = 0; lo < n; lo += 2 * w) {
            int64_t mid = lo + w < n ? lo + w : n, hi = lo + 2 * w < n ? lo + 2 * w : n;
            int64_t i = lo, j = mid, k = lo;
            while (i < mid && j < hi) {
                if (zap_asort_cmp(b, p[j], p[i], dt) < 0) tmp[k++] = p[j++];
                else tmp[k++] = p[i++];
            }
            while (i < mid) tmp[k++] = p[i++];
            while (j < hi) tmp[k++] = p[j++];
            for (int64_t t = lo; t < hi; t++) p[t] = tmp[t];
        }
    free(tmp);
}
/* stable LSD radix on the permutation p[0..n) by uint64 integer key. */
static void zap_asort_radix(int64_t *p, int64_t n, const void *b, int64_t dt) {
    uint64_t *key = (uint64_t *)malloc((size_t)n * sizeof(uint64_t));
    for (int64_t k = 0; k < n; k++) { p[k] = k; key[k] = zap_asort_ikey(b, k, dt); }
    int64_t *src = p, *dst = (int64_t *)malloc((size_t)n * sizeof(int64_t));
    for (int pass = 0; pass < 8; pass++) {
        int sh = pass * 8;
        size_t cnt[256];
        memset(cnt, 0, sizeof(cnt));
        for (int64_t k = 0; k < n; k++) cnt[(key[src[k]] >> sh) & 0xFF]++;
        size_t acc = 0;
        for (int c = 0; c < 256; c++) { size_t t = cnt[c]; cnt[c] = acc; acc += t; }
        for (int64_t k = 0; k < n; k++) { int by = (key[src[k]] >> sh) & 0xFF; dst[cnt[by]++] = src[k]; }
        int64_t *t = src; src = dst; dst = t;
    }
    if (src != p) { memcpy(p, src, (size_t)n * sizeof(int64_t)); free(src); }
    else free(dst);
    free(key);
}
/* permutation of [0,L) that sorts the contiguous lane buffer `b`. */
static void zap_asort_perm(int64_t *perm, const void *b, int64_t L, int64_t dt, int stable) {
    for (int64_t k = 0; k < L; k++) perm[k] = k;
    if (L < 2) return;
    if (!stable) {
        int depth = 2 * (63 - __builtin_clzll((unsigned long long)L));
        zap_asort_intro(perm, 0, L - 1, depth, b, dt);
    } else if (dt == DTYPE_F64 || dt == DTYPE_F32) {
        zap_asort_merge(perm, L, b, dt);
    } else {
        zap_asort_radix(perm, L, b, dt);
    }
}
/* physical element stride of `axis` (honours a view's explicit strides). */
static int64_t zap_asort_axis_stride(const ZapArray *a, int64_t axis) {
    if (a->strides) return a->strides[axis];
    int64_t s = 1;
    for (int64_t k = axis + 1; k < a->ndim; k++) s *= a->dims[k];
    return s;
}

/* The core driver. Sorts `a` along `axis` (or the flattened logical order when
 * `flatten`). When `out` is non-NULL it is a same-shaped (or 1-D, flattened)
 * contiguous i64 array that receives the argsort indices; otherwise the sort is
 * applied to `a`'s elements in place (stride-aware, so a view aliases its
 * parent). Lanes are gathered into a contiguous buffer so the kernels stay
 * contiguous. */
static void zap_asort_run(ZapArray *a, int64_t axis, int flatten, int stable, ZapArray *out) {
    int64_t dt = a->dtype;
    size_t esz = zap_array_elem_size(dt);
    int64_t count = zap_array_count_of(a);
    if (count == 0) return;
    char *adata = (char *)a->data;
    if (flatten) {
        /* Gather the whole array in row-major logical order into a lane. */
        char *lane = (char *)malloc((size_t)count * esz);
        int64_t nd = a->ndim;
        /* logical flat -> physical, honouring off/strides (zap_array_phys_nd). */
        for (int64_t f = 0; f < count; f++)
            memcpy(lane + (size_t)f * esz, adata + (size_t)zap_array_phys_nd(a, f) * esz, esz);
        int64_t *perm = (int64_t *)malloc((size_t)count * sizeof(int64_t));
        zap_asort_perm(perm, lane, count, dt, stable);
        if (out) {
            int64_t *od = (int64_t *)out->data;
            for (int64_t k = 0; k < count; k++) od[k] = perm[k];
        } else {
            /* in-place flatten only reached via sorted-copy (out==NULL + flatten
             * is used by the copy path on a fresh contiguous buffer). */
            char *tmp = (char *)malloc((size_t)count * esz);
            for (int64_t k = 0; k < count; k++)
                memcpy(tmp + (size_t)k * esz, lane + (size_t)perm[k] * esz, esz);
            memcpy(adata, tmp, (size_t)count * esz);
            free(tmp);
        }
        free(perm);
        free(lane);
        (void)nd;
        return;
    }
    int64_t L = a->dims[axis];
    int64_t sax = zap_asort_axis_stride(a, axis);
    int64_t nlanes = L ? count / L : 0;
    char *lane = (char *)malloc((size_t)L * esz);
    int64_t *perm = (int64_t *)malloc((size_t)L * sizeof(int64_t));
    int64_t nd = a->ndim;
    /* Odometer over every axis except `axis`, giving each lane's physical base. */
    int64_t *idx = (int64_t *)calloc((size_t)nd, sizeof(int64_t));
    int64_t osax = out ? zap_asort_axis_stride(out, axis) : 0;
    for (int64_t ln = 0; ln < nlanes; ln++) {
        int64_t base = a->off;
        for (int64_t k = 0; k < nd; k++)
            if (k != axis) base += idx[k] * zap_asort_axis_stride(a, k);
        for (int64_t m = 0; m < L; m++)
            memcpy(lane + (size_t)m * esz, adata + (size_t)(base + m * sax) * esz, esz);
        zap_asort_perm(perm, lane, L, dt, stable);
        if (out) {
            int64_t obase = out->off;
            for (int64_t k = 0; k < nd; k++)
                if (k != axis) obase += idx[k] * zap_asort_axis_stride(out, k);
            int64_t *od = (int64_t *)out->data;
            for (int64_t m = 0; m < L; m++) od[obase + m * osax] = perm[m];
        } else {
            for (int64_t m = 0; m < L; m++)
                memcpy(adata + (size_t)(base + m * sax) * esz, lane + (size_t)perm[m] * esz, esz);
        }
        /* advance the odometer over the non-axis axes (row-major, last fastest) */
        for (int64_t k = nd - 1; k >= 0; k--) {
            if (k == axis) continue;
            if (++idx[k] < a->dims[k]) break;
            idx[k] = 0;
        }
    }
    free(idx);
    free(perm);
    free(lane);
}

/* SIMD sort accelerator (#933, D-0195); defined in zapsortaccel.inc.c (included
 * after the cpu_features detector). Each returns 1 if it handled the (unstable,
 * contiguous, eligible) sort, else 0 -> fall through to the own floor above. */
int zap_sortaccel_sort_array(void *a, int64_t axis, int64_t flatten, int64_t stable);
int zap_sortaccel_argsort_array(void *a, int64_t axis, int64_t flatten, int64_t stable, void *out);

/* `a.sort(axis=-1, kind=)` — sort in place along `axis` (bad axis raises a
 * catchable ValueError). `stable` selects the kind. */
void zap_array_sort_inplace(void *ap, int64_t axis, int64_t stable) {
    ZapArray *a = (ZapArray *)ap;
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    if (zap_sortaccel_sort_array(a, ax, 0, stable)) return;
    zap_asort_run(a, ax, 0, (int)stable, NULL);
}
/* `sort(a, axis=-1|None, kind=)` — a new sorted copy. `flatten` (axis=None)
 * sorts the flattened array into a 1-D result; otherwise same shape along `axis`. */
void *zap_array_sorted(void *ap, int64_t axis, int64_t flatten, int64_t stable) {
    ZapArray *a = (ZapArray *)ap;
    int64_t count = zap_array_count_of(a);
    if (flatten) {
        ZapArray *res = (ZapArray *)zap_array_new(a->dtype, count);
        size_t esz = zap_array_elem_size(a->dtype);
        for (int64_t f = 0; f < count; f++)
            memcpy((char *)res->data + (size_t)f * esz,
                   (char *)a->data + (size_t)zap_array_phys_nd(a, f) * esz, esz);
        if (!zap_sortaccel_sort_array(res, 0, 1, stable))
            zap_asort_run(res, 0, 0, (int)stable, NULL);
        return res;
    }
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    /* fresh contiguous copy with the same shape, then sort in place along ax */
    ZapArray *res = (ZapArray *)zap_array_new_nd(a->dtype, a->ndim, count);
    for (int64_t k = 0; k < a->ndim; k++) zap_array_set_dim(res, k, a->dims[k]);
    size_t esz = zap_array_elem_size(a->dtype);
    for (int64_t f = 0; f < count; f++)
        memcpy((char *)res->data + (size_t)f * esz,
               (char *)a->data + (size_t)zap_array_phys_nd(a, f) * esz, esz);
    if (!zap_sortaccel_sort_array(res, ax, 0, stable))
        zap_asort_run(res, ax, 0, (int)stable, NULL);
    return res;
}
/* `argsort(a, axis=-1|None, kind=)` / `a.argsort(...)` — a new i64 index array
 * (same shape, or 1-D when flattened). */
void *zap_array_argsort(void *ap, int64_t axis, int64_t flatten, int64_t stable) {
    ZapArray *a = (ZapArray *)ap;
    int64_t count = zap_array_count_of(a);
    if (flatten) {
        ZapArray *out = (ZapArray *)zap_array_new(DTYPE_I64, count);
        if (!zap_sortaccel_argsort_array(a, 0, 1, stable, out))
            zap_asort_run(a, 0, 1, (int)stable, out);
        return out;
    }
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    ZapArray *out = (ZapArray *)zap_array_new_nd(DTYPE_I64, a->ndim, count);
    for (int64_t k = 0; k < a->ndim; k++) zap_array_set_dim(out, k, a->dims[k]);
    if (!zap_sortaccel_argsort_array(a, ax, 0, stable, out))
        zap_asort_run(a, ax, 0, (int)stable, out);
    return out;
}

/* ===========================================================================
 * Numeric-array searching / selection — own kernels (#934, D-0191)
 *
 * searchsorted / unique / partition / argpartition / argmin / argmax over numeric
 * arrays, built on the sort floor (#932). `unique` and `argsort`-style ops reuse
 * the stable permutation; `partition`/`argpartition` use a real quickselect
 * (median-of-3, insertion base case) that places each `kth` in its final slot.
 * NaNs sort/compare to the end (shared comparator); argmin/argmax propagate NaN
 * (the first NaN's index wins, matching NumPy). Verified bit-for-bit vs NumPy.
 * ======================================================================== */

/* compare element ia of buffer A vs element iv of buffer V (same dtype). */
static int zap_srch_cmp(const void *A, int64_t ia, const void *V, int64_t iv, int64_t dt) {
    switch (dt) {
    case DTYPE_F64:
    case DTYPE_F32:
        return zap_asort_fcmp(zap_asort_rdf(A, ia, dt), zap_asort_rdf(V, iv, dt));
    case DTYPE_I64: {
        int64_t a = ((const int64_t *)A)[ia], b = ((const int64_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    case DTYPE_I32: {
        int32_t a = ((const int32_t *)A)[ia], b = ((const int32_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    case DTYPE_I16: {
        int16_t a = ((const int16_t *)A)[ia], b = ((const int16_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    case DTYPE_I8: {
        int8_t a = ((const int8_t *)A)[ia], b = ((const int8_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    case DTYPE_U64: {
        uint64_t a = ((const uint64_t *)A)[ia], b = ((const uint64_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    case DTYPE_U32: {
        uint32_t a = ((const uint32_t *)A)[ia], b = ((const uint32_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    case DTYPE_U16: {
        uint16_t a = ((const uint16_t *)A)[ia], b = ((const uint16_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    default: {
        uint8_t a = ((const uint8_t *)A)[ia], b = ((const uint8_t *)V)[iv];
        return a < b ? -1 : a > b ? 1 : 0;
    }
    }
}
/* Gather `a`'s elements in row-major logical order into a fresh contiguous buffer
 * (honours a view's off/strides). Caller frees. */
static void *zap_srch_gather(ZapArray *a, int64_t *n_out) {
    int64_t n = zap_array_count_of(a);
    size_t esz = zap_array_elem_size(a->dtype);
    char *buf = (char *)malloc((size_t)(n > 0 ? n : 1) * esz);
    for (int64_t f = 0; f < n; f++)
        memcpy(buf + (size_t)f * esz, (char *)a->data + (size_t)zap_array_phys_nd(a, f) * esz, esz);
    *n_out = n;
    return buf;
}

/* searchsorted(a, v, side): a sorted (1-D logical), v any shape -> i64 insertion
 * indices shaped like v. side 0 = left (first i: a[i] >= v), 1 = right (a[i] > v). */
void *zap_array_searchsorted(void *ap, void *vp, int64_t side) {
    ZapArray *a = (ZapArray *)ap, *v = (ZapArray *)vp;
    int64_t dt = a->dtype;
    int64_t na, nv;
    char *abuf = (char *)zap_srch_gather(a, &na);
    char *vbuf = (char *)zap_srch_gather(v, &nv);
    ZapArray *out = (ZapArray *)zap_array_new_nd(DTYPE_I64, v->ndim, nv);
    for (int64_t k = 0; k < v->ndim; k++) zap_array_set_dim(out, k, v->dims[k]);
    int64_t *od = (int64_t *)out->data;
    for (int64_t j = 0; j < nv; j++) {
        int64_t lo = 0, hi = na;
        while (lo < hi) {
            int64_t mid = lo + (hi - lo) / 2;
            int c = zap_srch_cmp(abuf, mid, vbuf, j, dt);
            if (side == 0 ? (c < 0) : (c <= 0)) lo = mid + 1;
            else hi = mid;
        }
        od[j] = lo;
    }
    free(abuf);
    free(vbuf);
    return out;
}

/* argmin/argmax over a contiguous lane buffer; the first NaN's index wins (both),
 * else the first-occurrence extremum (NumPy semantics). */
static int64_t zap_srch_argextreme(const void *b, int64_t n, int64_t dt, int is_max) {
    int is_float = (dt == DTYPE_F64 || dt == DTYPE_F32);
    int64_t best = 0;
    for (int64_t k = 0; k < n; k++) {
        if (is_float && isnan(zap_asort_rdf(b, k, dt))) return k;
        if (k == 0) continue;
        int c = zap_asort_cmp(b, k, best, dt);
        if (is_max ? c > 0 : c < 0) best = k;
    }
    return best;
}
/* argmin/argmax over the flattened array -> a scalar index (NumPy axis=None). */
int64_t zap_array_argminmax(void *ap, int64_t is_max) {
    ZapArray *a = (ZapArray *)ap;
    int64_t n;
    char *buf = (char *)zap_srch_gather(a, &n);
    if (n == 0) {
        free(buf);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("argmin/argmax of an empty array", 31));
    }
    int64_t r = zap_srch_argextreme(buf, n, a->dtype, (int)is_max);
    free(buf);
    return r;
}
/* argmin/argmax along `axis` -> a rank-(n-1) i64 array of per-lane indices. */
void *zap_array_argminmax_axis(void *ap, int64_t axis, int64_t is_max) {
    ZapArray *a = (ZapArray *)ap;
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    int64_t outer, L, inner;
    ZapArray *res = zap_array_reduce_alloc(a, ax, DTYPE_I64, &outer, &L, &inner);
    int64_t dt = a->dtype;
    size_t esz = zap_array_elem_size(dt);
    char *lane = (char *)malloc((size_t)(L > 0 ? L : 1) * esz);
    int64_t *rd = (int64_t *)res->data;
    for (int64_t o = 0; o < outer; o++)
        for (int64_t in = 0; in < inner; in++) {
            int64_t start = o * (L * inner) + in;
            for (int64_t k = 0; k < L; k++)
                memcpy(lane + (size_t)k * esz,
                       (char *)a->data + (size_t)zap_array_phys_nd(a, start + k * inner) * esz, esz);
            rd[o * inner + in] = zap_srch_argextreme(lane, L, dt, (int)is_max);
        }
    free(lane);
    return res;
}

/* unique core: stable-sort a gathered buffer, record each group's first sorted
 * position in `bnd` (with bnd[u] = n). Returns the unique count. */
static int64_t zap_srch_unique_core(const void *buf, int64_t n, int64_t dt, int64_t *perm, int64_t *bnd) {
    zap_asort_perm(perm, buf, n, dt, 1); /* stable */
    int64_t u = 0;
    for (int64_t k = 0; k < n; k++)
        if (k == 0 || zap_asort_cmp(buf, perm[k], perm[k - 1], dt) != 0) bnd[u++] = k;
    bnd[u] = n;
    return u;
}
/* unique(a) -> sorted unique values (1-D, a's dtype). NaNs collapse to one. */
void *zap_array_unique_values(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    int64_t dt = a->dtype, n;
    char *buf = (char *)zap_srch_gather(a, &n);
    size_t esz = zap_array_elem_size(dt);
    int64_t *perm = (int64_t *)malloc((size_t)(n > 0 ? n : 1) * sizeof(int64_t));
    int64_t *bnd = (int64_t *)malloc((size_t)(n + 1) * sizeof(int64_t));
    int64_t u = zap_srch_unique_core(buf, n, dt, perm, bnd);
    ZapArray *res = (ZapArray *)zap_array_new(dt, u);
    for (int64_t t = 0; t < u; t++)
        memcpy((char *)res->data + (size_t)t * esz, buf + (size_t)perm[bnd[t]] * esz, esz);
    free(buf); free(perm); free(bnd);
    return res;
}
/* unique return_index: first-occurrence original index of each unique value. */
void *zap_array_unique_index(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    int64_t dt = a->dtype, n;
    char *buf = (char *)zap_srch_gather(a, &n);
    int64_t *perm = (int64_t *)malloc((size_t)(n > 0 ? n : 1) * sizeof(int64_t));
    int64_t *bnd = (int64_t *)malloc((size_t)(n + 1) * sizeof(int64_t));
    int64_t u = zap_srch_unique_core(buf, n, dt, perm, bnd);
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_I64, u);
    int64_t *rd = (int64_t *)res->data;
    for (int64_t t = 0; t < u; t++) rd[t] = perm[bnd[t]];
    free(buf); free(perm); free(bnd);
    return res;
}
/* unique return_counts: occurrence count of each unique value. */
void *zap_array_unique_counts(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    int64_t dt = a->dtype, n;
    char *buf = (char *)zap_srch_gather(a, &n);
    int64_t *perm = (int64_t *)malloc((size_t)(n > 0 ? n : 1) * sizeof(int64_t));
    int64_t *bnd = (int64_t *)malloc((size_t)(n + 1) * sizeof(int64_t));
    int64_t u = zap_srch_unique_core(buf, n, dt, perm, bnd);
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_I64, u);
    int64_t *rd = (int64_t *)res->data;
    for (int64_t t = 0; t < u; t++) rd[t] = bnd[t + 1] - bnd[t];
    free(buf); free(perm); free(bnd);
    return res;
}
/* unique return_inverse: for each original element, its unique index. 1-D len n. */
void *zap_array_unique_inverse(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    int64_t dt = a->dtype, n;
    char *buf = (char *)zap_srch_gather(a, &n);
    int64_t *perm = (int64_t *)malloc((size_t)(n > 0 ? n : 1) * sizeof(int64_t));
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_I64, n);
    int64_t *rd = (int64_t *)res->data;
    zap_asort_perm(perm, buf, n, dt, 1);
    int64_t ui = -1;
    for (int64_t k = 0; k < n; k++) {
        if (k == 0 || zap_asort_cmp(buf, perm[k], perm[k - 1], dt) != 0) ui++;
        rd[perm[k]] = ui;
    }
    free(buf); free(perm);
    return res;
}

/* quickselect: place each `kth` (ascending, deduped, in [0,L)) of the index perm
 * `p[lo..hi]` over buffer `b` in its final sorted slot; smaller before, larger
 * after. Small ranges fall back to insertion sort (every kth in range placed). */
static void zap_srch_qselect(int64_t *p, int64_t lo, int64_t hi, const void *b, int64_t dt,
                             const int64_t *kth, int64_t kl, int64_t kh) {
    if (kl > kh) return;
    while (lo < hi) {
        if (hi - lo < 16) {
            zap_asort_insertion(p, lo, hi, b, dt);
            return;
        }
        int64_t mid = lo + (hi - lo) / 2;
        if (zap_asort_cmp(b, p[mid], p[lo], dt) < 0) zap_asort_swap(p, lo, mid);
        if (zap_asort_cmp(b, p[hi], p[lo], dt) < 0) zap_asort_swap(p, lo, hi);
        if (zap_asort_cmp(b, p[hi], p[mid], dt) < 0) zap_asort_swap(p, mid, hi);
        zap_asort_swap(p, mid, hi - 1);
        int64_t pivot = p[hi - 1], i = lo, j = hi - 1;
        for (;;) {
            while (zap_asort_cmp(b, p[++i], pivot, dt) < 0) {}
            while (zap_asort_cmp(b, pivot, p[--j], dt) < 0) {}
            if (i >= j) break;
            zap_asort_swap(p, i, j);
        }
        zap_asort_swap(p, i, hi - 1);
        int64_t m = kl;
        while (m <= kh && kth[m] < i) m++;
        if (m <= kh && kth[m] == i) {
            zap_srch_qselect(p, lo, i - 1, b, dt, kth, kl, m - 1);
            zap_srch_qselect(p, i + 1, hi, b, dt, kth, m + 1, kh);
            return;
        }
        zap_srch_qselect(p, lo, i - 1, b, dt, kth, kl, m - 1);
        lo = i + 1;
        kl = m;
    }
}
/* Normalize the kth array for a lane of length L into an ascending, deduped,
 * validated int64 list; returns the count. Raises ValueError on out-of-range. */
static int64_t zap_srch_norm_kth(const int64_t *ksrc, int64_t nk, int64_t L, int64_t *kout) {
    for (int64_t t = 0; t < nk; t++) {
        int64_t k = ksrc[t];
        if (k < 0) k += L;
        if (k < 0 || k >= L)
            zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("kth out of bounds", 17));
        kout[t] = k;
    }
    /* insertion sort + dedup (nk is tiny) */
    for (int64_t i = 1; i < nk; i++) {
        int64_t v = kout[i], j = i - 1;
        while (j >= 0 && kout[j] > v) { kout[j + 1] = kout[j]; j--; }
        kout[j + 1] = v;
    }
    int64_t m = 0;
    for (int64_t i = 0; i < nk; i++)
        if (i == 0 || kout[i] != kout[i - 1]) kout[m++] = kout[i];
    return m;
}
/* The partition lane driver: for each lane along `axis`, gather, quickselect the
 * (normalized) kths, then scatter the partitioned values back (argpartition!=0
 * writes the lane's index perm into `out`). Mirrors zap_asort_run. */
static void zap_srch_partition_run(ZapArray *a, int64_t axis, const int64_t *ksrc, int64_t nk, ZapArray *out) {
    int64_t dt = a->dtype;
    size_t esz = zap_array_elem_size(dt);
    int64_t L = a->dims[axis];
    int64_t sax = zap_asort_axis_stride(a, axis);
    int64_t count = zap_array_count_of(a);
    int64_t nlanes = L ? count / L : 0;
    char *lane = (char *)malloc((size_t)(L > 0 ? L : 1) * esz);
    int64_t *perm = (int64_t *)malloc((size_t)(L > 0 ? L : 1) * sizeof(int64_t));
    int64_t *kth = (int64_t *)malloc((size_t)(nk > 0 ? nk : 1) * sizeof(int64_t));
    int64_t nd = a->ndim;
    int64_t *idx = (int64_t *)calloc((size_t)nd, sizeof(int64_t));
    char *adata = (char *)a->data;
    for (int64_t ln = 0; ln < nlanes; ln++) {
        int64_t base = a->off;
        for (int64_t k = 0; k < nd; k++)
            if (k != axis) base += idx[k] * zap_asort_axis_stride(a, k);
        for (int64_t m = 0; m < L; m++)
            memcpy(lane + (size_t)m * esz, adata + (size_t)(base + m * sax) * esz, esz);
        for (int64_t m = 0; m < L; m++) perm[m] = m;
        int64_t nkk = zap_srch_norm_kth(ksrc, nk, L, kth);
        zap_srch_qselect(perm, 0, L - 1, lane, dt, kth, 0, nkk - 1);
        if (out) {
            int64_t obase = out->off;
            for (int64_t k = 0; k < nd; k++)
                if (k != axis) obase += idx[k] * zap_asort_axis_stride(out, k);
            int64_t osax = zap_asort_axis_stride(out, axis);
            int64_t *od = (int64_t *)out->data;
            for (int64_t m = 0; m < L; m++) od[obase + m * osax] = perm[m];
        } else {
            for (int64_t m = 0; m < L; m++)
                memcpy(adata + (size_t)(base + m * sax) * esz, lane + (size_t)perm[m] * esz, esz);
        }
        for (int64_t k = nd - 1; k >= 0; k--) {
            if (k == axis) continue;
            if (++idx[k] < a->dims[k]) break;
            idx[k] = 0;
        }
    }
    free(idx); free(kth); free(perm); free(lane);
}
/* partition(a, kth, axis) in place. kth is an i64 array (1 element or a list). */
void zap_array_partition_inplace(void *ap, void *kp, int64_t axis) {
    ZapArray *a = (ZapArray *)ap, *k = (ZapArray *)kp;
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    zap_srch_partition_run(a, ax, (int64_t *)k->data + k->off, zap_array_count_of(k), NULL);
}
/* partition(a, kth, axis) -> a partitioned copy (same shape). */
void *zap_array_partitioned(void *ap, void *kp, int64_t axis) {
    ZapArray *a = (ZapArray *)ap, *k = (ZapArray *)kp;
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    int64_t count = zap_array_count_of(a);
    size_t esz = zap_array_elem_size(a->dtype);
    ZapArray *res = (ZapArray *)zap_array_new_nd(a->dtype, a->ndim, count);
    for (int64_t d = 0; d < a->ndim; d++) zap_array_set_dim(res, d, a->dims[d]);
    for (int64_t f = 0; f < count; f++)
        memcpy((char *)res->data + (size_t)f * esz,
               (char *)a->data + (size_t)zap_array_phys_nd(a, f) * esz, esz);
    zap_srch_partition_run(res, ax, (int64_t *)k->data + k->off, zap_array_count_of(k), NULL);
    return res;
}
/* argpartition(a, kth, axis) -> an i64 index array (same shape). */
void *zap_array_argpartition(void *ap, void *kp, int64_t axis) {
    ZapArray *a = (ZapArray *)ap, *k = (ZapArray *)kp;
    int64_t ax = zap_array_axis_norm(axis, a->ndim);
    int64_t count = zap_array_count_of(a);
    ZapArray *out = (ZapArray *)zap_array_new_nd(DTYPE_I64, a->ndim, count);
    for (int64_t d = 0; d < a->ndim; d++) zap_array_set_dim(out, d, a->dims[d]);
    zap_srch_partition_run(a, ax, (int64_t *)k->data + k->off, zap_array_count_of(k), out);
    return out;
}

/* sum/min/max/prod along `axis` (op 0/1/2/3). Result keeps the input dtype. */
void *zap_array_reduce_axis(void *ap, int64_t axis, int64_t op) {
    /* Compact a strided view so the factorization below walks a dense buffer
     * (#502). Validate the axis against the view's rank BEFORE compacting so a
     * bad axis still raises (compaction preserves ndim). */
    (void)zap_array_axis_norm(axis, ((ZapArray *)ap)->ndim);
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    axis = zap_array_axis_norm(axis, a->ndim);
    if ((op == 1 || op == 2) && a->dims[axis] == 0)
        zap_array_empty_reduce();
    int64_t outer, axislen, inner;
    ZapArray *res =
        zap_array_reduce_alloc(a, axis, a->dtype, &outer, &axislen, &inner);
    size_t esz = zap_array_elem_size(a->dtype);
    const void *inb = (const char *)a->data + (size_t)a->off * esz;
    void *outb = res->data;
    switch (a->dtype) {
    case DTYPE_F64:
        zap_reduce_axis_f64(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_F32:
        zap_reduce_axis_f32(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_I64:
        zap_reduce_axis_i64(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_I32:
        zap_reduce_axis_i32(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_I16:
        zap_reduce_axis_i16(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_I8:
        zap_reduce_axis_i8(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_U8:
        zap_reduce_axis_u8(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_U16:
        zap_reduce_axis_u16(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_U32:
        zap_reduce_axis_u32(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_U64:
        zap_reduce_axis_u64(inb, outb, outer, axislen, inner, op);
        break;
    case DTYPE_BOOL:
        zap_reduce_axis_bool(inb, outb, outer, axislen, inner, op);
        break;
    default:
        break;
    }
    return res;
}

/* mean along `axis`: an f64 result (f32 source keeps f32). Accumulates in
 * double; an empty axis yields nan per lane (no raise). */
void *zap_array_mean_axis(void *ap, int64_t axis) {
    (void)zap_array_axis_norm(axis, ((ZapArray *)ap)->ndim);
    ZapArray *a = (ZapArray *)zap_array_contig(ap); /* dense walk (#502) */
    axis = zap_array_axis_norm(axis, a->ndim);
    int64_t out_dtype = (a->dtype == DTYPE_F32) ? DTYPE_F32 : DTYPE_F64;
    int64_t outer, axislen, inner;
    ZapArray *res =
        zap_array_reduce_alloc(a, axis, out_dtype, &outer, &axislen, &inner);
    size_t esz = zap_array_elem_size(a->dtype);
    const void *inb = (const char *)a->data + (size_t)a->off * esz;
    /* Reduce into a double scratch lane buffer, then narrow to the result's
     * float width. For f64 the result buffer is used directly. */
    int64_t total = outer * inner;
    if (out_dtype == DTYPE_F32) {
        double *tmp = (double *)zap_buf((size_t)(total > 0 ? total : 0) *
                                        sizeof(double));
        zap_mean_axis_f32(inb, tmp, outer, axislen, inner);
        float *out = (float *)res->data;
        for (int64_t i = 0; i < total; i++)
            out[i] = (float)tmp[i];
        return res;
    }
    double *out = (double *)res->data;
    switch (a->dtype) {
    case DTYPE_F64:
        zap_mean_axis_f64(inb, out, outer, axislen, inner);
        break;
    case DTYPE_I64:
        zap_mean_axis_i64(inb, out, outer, axislen, inner);
        break;
    case DTYPE_I32:
        zap_mean_axis_i32(inb, out, outer, axislen, inner);
        break;
    case DTYPE_I16:
        zap_mean_axis_i16(inb, out, outer, axislen, inner);
        break;
    case DTYPE_I8:
        zap_mean_axis_i8(inb, out, outer, axislen, inner);
        break;
    case DTYPE_U8:
        zap_mean_axis_u8(inb, out, outer, axislen, inner);
        break;
    case DTYPE_U16:
        zap_mean_axis_u16(inb, out, outer, axislen, inner);
        break;
    case DTYPE_U32:
        zap_mean_axis_u32(inb, out, outer, axislen, inner);
        break;
    case DTYPE_U64:
        zap_mean_axis_u64(inb, out, outer, axislen, inner);
        break;
    case DTYPE_BOOL:
        zap_mean_axis_bool(inb, out, outer, axislen, inner);
        break;
    default:
        break;
    }
    return res;
}

/* ---------------------------------------------------------------------------
 * Array constructors (#504, D-0167): zeros / ones / full / arange / linspace.
 * These allocate a fresh contiguous array (`off == 0`) and fill it. The flat
 * element stores dispatch on dtype; the value crosses the ABI as an int64 word
 * (integer/bool dtypes, truncated/normalized) or a double (float dtypes). */
static void arr_store_i(ZapArray *a, int64_t i, int64_t v) {
    switch (a->dtype) {
    case DTYPE_BOOL:
        ((uint8_t *)a->data)[i] = v ? 1 : 0;
        break;
    case DTYPE_U8:
    case DTYPE_I8:
        ((uint8_t *)a->data)[i] = (uint8_t)v;
        break;
    case DTYPE_I16:
    case DTYPE_U16:
        ((uint16_t *)a->data)[i] = (uint16_t)v;
        break;
    case DTYPE_I32:
    case DTYPE_U32:
        ((uint32_t *)a->data)[i] = (uint32_t)v;
        break;
    default: /* i64 / u64 */
        ((uint64_t *)a->data)[i] = (uint64_t)v;
        break;
    }
}
static void arr_store_f(ZapArray *a, int64_t i, double v) {
    if (a->dtype == DTYPE_F32) {
        ((float *)a->data)[i] = (float)v;
    } else {
        ((double *)a->data)[i] = v;
    }
}

/* --- matrix multiply / dot product (#509, D-0172) ------------------------
 * `a @ b` and `dot(a, b)`. Operands are compacted to contiguous first, then the
 * element at logical flat index `i` is read with its source dtype widened to an
 * int64 or double accumulator (`mm_load_i` / `mm_load_f`). A shape mismatch on
 * the contracted axis raises a catchable ValueError. */
static int64_t mm_load_i(const ZapArray *a, int64_t i) {
    int64_t p = a->off + i;
    switch (a->dtype) {
    case DTYPE_BOOL:
    case DTYPE_U8:
        return ((const uint8_t *)a->data)[p];
    case DTYPE_I8:
        return ((const int8_t *)a->data)[p];
    case DTYPE_I16:
        return ((const int16_t *)a->data)[p];
    case DTYPE_U16:
        return ((const uint16_t *)a->data)[p];
    case DTYPE_I32:
        return ((const int32_t *)a->data)[p];
    case DTYPE_U32:
        return ((const uint32_t *)a->data)[p];
    case DTYPE_U64:
        return (int64_t)((const uint64_t *)a->data)[p];
    case DTYPE_F32:
        return (int64_t)((const float *)a->data)[p];
    case DTYPE_F64:
        return (int64_t)((const double *)a->data)[p];
    default: /* I64 */
        return ((const int64_t *)a->data)[p];
    }
}
static double mm_load_f(const ZapArray *a, int64_t i) {
    int64_t p = a->off + i;
    switch (a->dtype) {
    case DTYPE_F64:
        return ((const double *)a->data)[p];
    case DTYPE_F32:
        return (double)((const float *)a->data)[p];
    case DTYPE_BOOL:
    case DTYPE_U8:
        return (double)((const uint8_t *)a->data)[p];
    case DTYPE_I8:
        return (double)((const int8_t *)a->data)[p];
    case DTYPE_I16:
        return (double)((const int16_t *)a->data)[p];
    case DTYPE_U16:
        return (double)((const uint16_t *)a->data)[p];
    case DTYPE_I32:
        return (double)((const int32_t *)a->data)[p];
    case DTYPE_U32:
        return (double)((const uint32_t *)a->data)[p];
    case DTYPE_U64:
        return (double)((const uint64_t *)a->data)[p];
    default: /* I64 */
        return (double)((const int64_t *)a->data)[p];
    }
}
/* Read element `i` as a raw `uint64_t` (for the unsigned comparison domain,
 * #510): u8/u16/u32/u64/bool widen exactly; signed types are never asked for in
 * this domain. Flat index, so the caller contiguates (or uses a stride-aware
 * offset) first, exactly like `mm_load_i`/`mm_load_f`. */
static uint64_t mm_load_u(const ZapArray *a, int64_t i) {
    int64_t p = a->off + i;
    switch (a->dtype) {
    case DTYPE_U64:
        return ((const uint64_t *)a->data)[p];
    case DTYPE_U32:
        return (uint64_t)((const uint32_t *)a->data)[p];
    case DTYPE_U16:
        return (uint64_t)((const uint16_t *)a->data)[p];
    case DTYPE_BOOL:
    case DTYPE_U8:
        return (uint64_t)((const uint8_t *)a->data)[p];
    default: /* a same-family unsigned pair never reaches here with a signed */
        return (uint64_t)mm_load_i(a, i);
    }
}
static void zap_array_matmul_mismatch(void) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                    zap_str_new("matmul: shapes are not aligned", 30));
}

/* 1-D · 1-D dot product -> a scalar word. `doti` accumulates in int64 (integer
 * result dtypes), `dotf` in double (float). Lengths must match. */
int64_t zap_array_doti(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t n = a->dims[0];
    if (b->dims[0] != n) {
        zap_array_matmul_mismatch();
    }
    int64_t acc = 0;
    for (int64_t k = 0; k < n; k++) {
        acc += mm_load_i(a, k) * mm_load_i(b, k);
    }
    return acc;
}
double zap_array_dotf(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t n = a->dims[0];
    if (b->dims[0] != n) {
        zap_array_matmul_mismatch();
    }
    double acc = 0.0;
    for (int64_t k = 0; k < n; k++) {
        acc += mm_load_f(a, k) * mm_load_f(b, k);
    }
    return acc;
}

/* Blocked + SIMD matmul floor kernel (#913, D-0180). The D-0172 scalar triple
 * loop read every element through `mm_load_*` (a per-element dtype switch) and
 * walked B down a column (stride n), so it neither vectorized nor used cache.
 * Here we materialize A (m×kk) and B (kk×n) into contiguous accumulator-typed
 * buffers ONCE, then run the cache-friendly `ikj` order: the innermost loop is a
 * contiguous SAXPY over a result row (`C[i,j] += a * B[t,j]`), which the compiler
 * vectorizes at -O2. Per output cell the k-accumulation keeps its increasing-`t`
 * order, so results are BIT-IDENTICAL to the scalar kernel for every dtype (no
 * floating-point reassociation). This is the own-kernel FLOOR (#914): simple,
 * exact, portable; register/cache tiling and the BLAS hand-off are the ceiling,
 * dispatched at #904. Accumulator is `double` (float out) or `int64_t` (integer
 * out), matching D-0172; `arr_store_*` narrows to the result dtype. */
#define ZAP_MATMUL_KERNEL(suffix, acctype, load)                               \
    static void zap_matmul_##suffix(ZapArray *a, ZapArray *b, ZapArray *res,    \
                                    int64_t m, int64_t n, int64_t kk) {         \
        int64_t na = m * kk, nb = kk * n, nc = m * n;                          \
        acctype *A = (acctype *)zap_buf((size_t)(na > 0 ? na : 0) * sizeof(acctype)); \
        acctype *B = (acctype *)zap_buf((size_t)(nb > 0 ? nb : 0) * sizeof(acctype)); \
        acctype *C = (acctype *)zap_buf((size_t)(nc > 0 ? nc : 0) * sizeof(acctype)); \
        for (int64_t x = 0; x < na; x++)                                       \
            A[x] = (acctype)load(a, x);                                        \
        for (int64_t x = 0; x < nb; x++)                                       \
            B[x] = (acctype)load(b, x);                                        \
        for (int64_t x = 0; x < nc; x++)                                       \
            C[x] = (acctype)0;                                                 \
        for (int64_t i = 0; i < m; i++) {                                      \
            for (int64_t t = 0; t < kk; t++) {                                 \
                acctype av = A[i * kk + t];                                    \
                const acctype *restrict brow = B + t * n;                     \
                acctype *restrict crow = C + i * n;                           \
                for (int64_t j = 0; j < n; j++)                               \
                    crow[j] += av * brow[j];                                   \
            }                                                                  \
        }                                                                      \
        for (int64_t x = 0; x < nc; x++)                                       \
            arr_store_##suffix(res, x, C[x]);                                  \
    }
ZAP_MATMUL_KERNEL(f, double, mm_load_f)
ZAP_MATMUL_KERNEL(i, int64_t, mm_load_i)

/* Numeric acceleration seam (#912, D-0179), defined in zapblas.inc.c at the end
 * of this file. Forward-declared here so the matmul dispatch can consult the
 * tier/availability and pin the provider's threads before a BLAS hand-off. */
int64_t zap_accel_tier(void);
int64_t zap_accel_available(void);
void zap_accel_ensure_threads(void);

/* Automatic matmul dispatch (#904, D-0181). Zap picks the fastest kernel for the
 * (dtype, shape, CPU-budget): the own blocked+SIMD FLOOR (#913) by default, with
 * a BLAS gemm CEILING auto-selected for large FLOAT products when a provider is
 * linked — "the language optimizes for you," overridable via ZAP_ACCEL.
 *   - integer out  -> always own (BLAS routes ints through f64, inexact > 2^53);
 *   - ZAP_ACCEL=native -> own; =fast -> BLAS (if linked), else own;
 *   - auto (default) -> BLAS for large float (work >= the floor), else own.
 * The floor is hardcoded and deliberately conservative (jitter-safe default; the
 * own kernel has no thread pool to misconfigure); it is tunable after the
 * 1/4/32-core ±container benchmarking in #904 (not an API promise). */
#define ZAP_MATMUL_BLAS_FLOOR (64 * 64 * 64) /* ~n>=64 square; tunable (#904) */

#if defined(ZAP_HAVE_BLAS)
#if defined(ZAP_BLAS_ACCELERATE)
/* Use Accelerate's modern CBLAS (LP64, `int` dims) — the classic `cblas_dgemm`
 * spelling is deprecated since macOS 13.3 but the NEW_LAPACK interface keeps the
 * same name and signature. (Not ILP64, so the `(int)` dim casts below stand.) */
#define ACCELERATE_NEW_LAPACK 1
#include <Accelerate/Accelerate.h>
#else
#include <cblas.h>
#endif
/* BLAS gemm hand-off: materialize the operands as the gemm element type, pin the
 * provider pool to Zap's CPU budget (#905), then `cblas_{d,s}gemm`. BLAS packs
 * and blocks, so it reassociates the contraction — float results match the own
 * kernel only within fp tolerance (as NumPy's BLAS-backed `@` does); integers
 * never take this path, so they stay exact. */
#define ZAP_MATMUL_BLAS(suffix, ctype, gemm)                                   \
    static void zap_matmul_blas_##suffix(ZapArray *a, ZapArray *b,             \
                                         ZapArray *res, int64_t m, int64_t n,   \
                                         int64_t kk) {                          \
        int64_t na = m * kk, nb = kk * n, nc = m * n;                          \
        ctype *A = (ctype *)zap_buf((size_t)(na > 0 ? na : 0) * sizeof(ctype)); \
        ctype *B = (ctype *)zap_buf((size_t)(nb > 0 ? nb : 0) * sizeof(ctype)); \
        ctype *C = (ctype *)zap_buf((size_t)(nc > 0 ? nc : 0) * sizeof(ctype)); \
        for (int64_t x = 0; x < na; x++)                                       \
            A[x] = (ctype)mm_load_f(a, x);                                     \
        for (int64_t x = 0; x < nb; x++)                                       \
            B[x] = (ctype)mm_load_f(b, x);                                     \
        zap_accel_ensure_threads();                                            \
        gemm(CblasRowMajor, CblasNoTrans, CblasNoTrans, (int)m, (int)n,        \
             (int)kk, (ctype)1, A, (int)kk, B, (int)n, (ctype)0, C, (int)n);   \
        for (int64_t x = 0; x < nc; x++)                                       \
            arr_store_f(res, x, (double)C[x]);                                 \
    }
ZAP_MATMUL_BLAS(f64, double, cblas_dgemm)
ZAP_MATMUL_BLAS(f32, float, cblas_sgemm)

/* --- linear algebra (#926, D-0183), LAPACK-backed ------------------------
 * `import linalg`: solve / inv / det via the Fortran LAPACK interface
 * (`dgesv_`/`dgetrf_`/`dgetri_`), which BOTH providers expose — OpenBLAS and
 * Apple Accelerate (NEW_LAPACK, declared by <Accelerate/Accelerate.h> above).
 * LAPACK is column-major while Zap arrays are row-major, so each matrix is
 * transposed into a column-major scratch buffer and the result transposed back.
 * The first slice computes in f64 (any input promoted; f64 result) — one path,
 * no f32-precision LAPACK yet (a follow-up). The public entry points below are
 * ALWAYS compiled (so a linalg program links); only these impls need a provider. */
#if defined(ZAP_BLAS_OPENBLAS)
/* OpenBLAS ships the Fortran symbols but no standard C declaration header; the
 * ABI is stable (all args by pointer, trailing underscore). Accelerate declares
 * them itself via the header already included for cblas. */
extern void dgesv_(const int *n, const int *nrhs, double *a, const int *lda,
                   int *ipiv, double *b, const int *ldb, int *info);
extern void dgetrf_(const int *m, const int *n, double *a, const int *lda,
                    int *ipiv, int *info);
extern void dgetri_(const int *n, double *a, const int *lda, const int *ipiv,
                    double *work, const int *lwork, int *info);
extern void dpotrf_(const char *uplo, const int *n, double *a, const int *lda,
                    int *info);
extern void dgeqrf_(const int *m, const int *n, double *a, const int *lda,
                    double *tau, double *work, const int *lwork, int *info);
extern void dorgqr_(const int *m, const int *n, const int *k, double *a,
                    const int *lda, const double *tau, double *work,
                    const int *lwork, int *info);
extern void dgesdd_(const char *jobz, const int *m, const int *n, double *a,
                    const int *lda, double *s, double *u, const int *ldu,
                    double *vt, const int *ldvt, double *work, const int *lwork,
                    int *iwork, int *info);
/* #928: general eigenvalues (geev), symmetric eigendecomposition (syevd),
 * minimum-norm least squares (gelsd). */
extern void dgeev_(const char *jobvl, const char *jobvr, const int *n, double *a,
                   const int *lda, double *wr, double *wi, double *vl,
                   const int *ldvl, double *vr, const int *ldvr, double *work,
                   const int *lwork, int *info);
extern void dsyevd_(const char *jobz, const char *uplo, const int *n, double *a,
                    const int *lda, double *w, double *work, const int *lwork,
                    int *iwork, const int *liwork, int *info);
extern void dgelsd_(const int *m, const int *n, const int *nrhs, double *a,
                    const int *lda, double *b, const int *ldb, double *s,
                    const double *rcond, int *rank, double *work,
                    const int *lwork, int *iwork, int *info);
#endif

static void zap_linalg_err(const char *m) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}
/* Transpose a's row-major n*n into a fresh column-major double buffer (a's
 * elements read dtype-generically, so integer / f32 inputs promote to f64). */
static double *zap_linalg_colmajor(ZapArray *a, int64_t n) {
    double *cm = (double *)zap_buf((size_t)(n * n) * sizeof(double));
    for (int64_t i = 0; i < n; i++)
        for (int64_t j = 0; j < n; j++)
            cm[i + j * n] = mm_load_f(a, i * n + j);
    return cm;
}

static void *zap_linalg_solve_impl(ZapArray *a, ZapArray *b, int64_t n,
                                   int64_t nrhs, int b_is_1d) {
    double *A = zap_linalg_colmajor(a, n);
    double *X = (double *)zap_buf((size_t)(n * nrhs) * sizeof(double));
    for (int64_t i = 0; i < n; i++)
        for (int64_t r = 0; r < nrhs; r++)
            X[i + r * n] = b_is_1d ? mm_load_f(b, i) : mm_load_f(b, i * nrhs + r);
    int *ipiv = (int *)zap_buf((size_t)n * sizeof(int));
    int ni = (int)n, nr = (int)nrhs, info = 0;
    dgesv_(&ni, &nr, A, &ni, ipiv, X, &ni, &info);
    if (info != 0)
        zap_linalg_err("linalg.solve: singular matrix");
    ZapArray *res;
    if (b_is_1d) {
        res = (ZapArray *)zap_array_new(DTYPE_F64, n);
        for (int64_t i = 0; i < n; i++)
            arr_store_f(res, i, X[i]);
    } else {
        res = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * nrhs);
        zap_array_set_dim(res, 0, n);
        zap_array_set_dim(res, 1, nrhs);
        for (int64_t i = 0; i < n; i++)
            for (int64_t r = 0; r < nrhs; r++)
                arr_store_f(res, i * nrhs + r, X[i + r * n]);
    }
    return res;
}

static void *zap_linalg_inv_impl(ZapArray *a, int64_t n) {
    double *A = zap_linalg_colmajor(a, n);
    int *ipiv = (int *)zap_buf((size_t)n * sizeof(int));
    int ni = (int)n, info = 0;
    dgetrf_(&ni, &ni, A, &ni, ipiv, &info);
    if (info != 0)
        zap_linalg_err("linalg.inv: singular matrix");
    int lwork = (int)(n * 64);
    double *work = (double *)zap_buf((size_t)lwork * sizeof(double));
    dgetri_(&ni, A, &ni, ipiv, work, &lwork, &info);
    if (info != 0)
        zap_linalg_err("linalg.inv: singular matrix");
    ZapArray *res = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * n);
    zap_array_set_dim(res, 0, n);
    zap_array_set_dim(res, 1, n);
    for (int64_t i = 0; i < n; i++)
        for (int64_t j = 0; j < n; j++)
            arr_store_f(res, i * n + j, A[i + j * n]); /* col-major A^-1 -> row */
    return res;
}

static double zap_linalg_det_impl(ZapArray *a, int64_t n) {
    double *A = zap_linalg_colmajor(a, n);
    int *ipiv = (int *)zap_buf((size_t)n * sizeof(int));
    int ni = (int)n, info = 0;
    dgetrf_(&ni, &ni, A, &ni, ipiv, &info);
    if (info > 0)
        return 0.0; /* singular -> exactly-0 determinant (NumPy returns 0.0) */
    if (info < 0)
        zap_linalg_err("linalg.det: invalid matrix");
    double d = 1.0;
    for (int64_t i = 0; i < n; i++) {
        d *= A[i + i * n];
        if (ipiv[i] != (int)(i + 1))
            d = -d; /* each row swap flips the sign */
    }
    return d;
}

/* Transpose a's row-major m*n into a fresh column-major double buffer (#927). */
static double *zap_linalg_colmajor_mn(ZapArray *a, int64_t m, int64_t n) {
    int64_t t = m * n;
    double *cm = (double *)zap_buf((size_t)(t > 0 ? t : 0) * sizeof(double));
    for (int64_t i = 0; i < m; i++)
        for (int64_t j = 0; j < n; j++)
            cm[i + j * m] = mm_load_f(a, i * n + j);
    return cm;
}

/* cholesky (#927): a row-major n*n symmetric positive-definite -> lower L (with
 * A = L Lᵀ), strict upper zeroed. dpotrf is in-place on the lower triangle. */
static void *zap_linalg_cholesky_impl(ZapArray *a, int64_t n) {
    double *A = zap_linalg_colmajor(a, n);
    int ni = (int)n, info = 0;
    dpotrf_("L", &ni, A, &ni, &info);
    if (info != 0)
        zap_linalg_err("linalg.cholesky: matrix is not positive definite");
    ZapArray *res = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * n);
    zap_array_set_dim(res, 0, n);
    zap_array_set_dim(res, 1, n);
    for (int64_t i = 0; i < n; i++)
        for (int64_t j = 0; j < n; j++)
            arr_store_f(res, i * n + j, (i >= j) ? A[i + j * n] : 0.0);
    return res;
}

/* qr reduced (#927), NumPy mode='reduced': a (m*n) -> Q (m*k) orthonormal and R
 * (k*n) upper-triangular, k=min(m,n). Q and R get their own entry points (each
 * re-runs the factorization — a shared-factorization out-param form is a
 * follow-up). `dgeqrf` yields R + Householder vectors; `dorgqr` forms Q. */
static double *zap_linalg_qr_factor(ZapArray *a, int64_t m, int64_t n, double **tau_out) {
    int64_t k = m < n ? m : n;
    double *cm = zap_linalg_colmajor_mn(a, m, n);
    double *tau = (double *)zap_buf((size_t)(k > 0 ? k : 1) * sizeof(double));
    int im = (int)m, in = (int)n, info = 0, lw = -1;
    double wq = 0;
    dgeqrf_(&im, &in, cm, &im, tau, &wq, &lw, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    dgeqrf_(&im, &in, cm, &im, tau, work, &lw, &info);
    *tau_out = tau;
    return cm;
}
static void *zap_linalg_qr_r_impl(ZapArray *a, int64_t m, int64_t n) {
    int64_t k = m < n ? m : n;
    double *tau = NULL;
    double *cm = zap_linalg_qr_factor(a, m, n, &tau);
    ZapArray *R = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, k * n);
    zap_array_set_dim(R, 0, k);
    zap_array_set_dim(R, 1, n);
    for (int64_t i = 0; i < k; i++)
        for (int64_t j = 0; j < n; j++)
            arr_store_f(R, i * n + j, (i <= j) ? cm[i + j * m] : 0.0);
    return R;
}
static void *zap_linalg_qr_q_impl(ZapArray *a, int64_t m, int64_t n) {
    int64_t k = m < n ? m : n;
    double *tau = NULL;
    double *cm = zap_linalg_qr_factor(a, m, n, &tau);
    int im = (int)m, ik = (int)k, info = 0, lw = -1;
    double wq = 0;
    dorgqr_(&im, &ik, &ik, cm, &im, tau, &wq, &lw, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    dorgqr_(&im, &ik, &ik, cm, &im, tau, work, &lw, &info);
    ZapArray *Q = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, m * k);
    zap_array_set_dim(Q, 0, m);
    zap_array_set_dim(Q, 1, k);
    for (int64_t i = 0; i < m; i++)
        for (int64_t j = 0; j < k; j++)
            arr_store_f(Q, i * k + j, cm[i + j * m]);
    return Q;
}

/* svd reduced (#927), NumPy full_matrices=False: a (m*n) -> U (m*k), S (k), Vt
 * (k*n), k=min(m,n), via `dgesdd`. U/S/Vt get their own entry points (each
 * re-runs gesdd — a shared-factorization form is a follow-up). */
static void zap_linalg_svd_run(ZapArray *a, int64_t m, int64_t n, int64_t k,
                               double *S, double *Uc, double *Vc) {
    double *cm = zap_linalg_colmajor_mn(a, m, n);
    int im = (int)m, in = (int)n, ik = (int)k, info = 0, lw = -1;
    double wq = 0;
    int *iw = (int *)zap_buf((size_t)(8 * (k > 0 ? k : 1)) * sizeof(int));
    dgesdd_("S", &im, &in, cm, &im, S, Uc, &im, Vc, &ik, &wq, &lw, iw, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    dgesdd_("S", &im, &in, cm, &im, S, Uc, &im, Vc, &ik, work, &lw, iw, &info);
    if (info != 0)
        zap_linalg_err("linalg.svd: did not converge");
}
static void *zap_linalg_svd_u_impl(ZapArray *a, int64_t m, int64_t n) {
    int64_t k = m < n ? m : n;
    double *S = (double *)zap_buf((size_t)(k > 0 ? k : 1) * sizeof(double));
    double *Uc = (double *)zap_buf((size_t)(m * k > 0 ? m * k : 1) * sizeof(double));
    double *Vc = (double *)zap_buf((size_t)(k * n > 0 ? k * n : 1) * sizeof(double));
    zap_linalg_svd_run(a, m, n, k, S, Uc, Vc);
    ZapArray *U = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, m * k);
    zap_array_set_dim(U, 0, m);
    zap_array_set_dim(U, 1, k);
    for (int64_t i = 0; i < m; i++)
        for (int64_t j = 0; j < k; j++)
            arr_store_f(U, i * k + j, Uc[i + j * m]);
    return U;
}
static void *zap_linalg_svd_s_impl(ZapArray *a, int64_t m, int64_t n) {
    int64_t k = m < n ? m : n;
    double *S = (double *)zap_buf((size_t)(k > 0 ? k : 1) * sizeof(double));
    double *Uc = (double *)zap_buf((size_t)(m * k > 0 ? m * k : 1) * sizeof(double));
    double *Vc = (double *)zap_buf((size_t)(k * n > 0 ? k * n : 1) * sizeof(double));
    zap_linalg_svd_run(a, m, n, k, S, Uc, Vc);
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, k);
    for (int64_t i = 0; i < k; i++)
        arr_store_f(res, i, S[i]);
    return res;
}
static void *zap_linalg_svd_vt_impl(ZapArray *a, int64_t m, int64_t n) {
    int64_t k = m < n ? m : n;
    double *S = (double *)zap_buf((size_t)(k > 0 ? k : 1) * sizeof(double));
    double *Uc = (double *)zap_buf((size_t)(m * k > 0 ? m * k : 1) * sizeof(double));
    double *Vc = (double *)zap_buf((size_t)(k * n > 0 ? k * n : 1) * sizeof(double));
    zap_linalg_svd_run(a, m, n, k, S, Uc, Vc);
    ZapArray *Vt = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, k * n);
    zap_array_set_dim(Vt, 0, k);
    zap_array_set_dim(Vt, 1, n);
    for (int64_t i = 0; i < k; i++)
        for (int64_t j = 0; j < n; j++)
            arr_store_f(Vt, i * n + j, Vc[i + j * k]);
    return Vt;
}

/* eigvals (#928, migrated in #928-eig): general real matrix eigenvalues via
 * `dgeev` (no eigenvectors), as an `array[complex]` (1-D). Eigenvalues of a real
 * matrix are real or come in conjugate pairs; LAPACK yields each pair
 * consecutively with the +imag member first. Order is otherwise LAPACK's (not
 * sorted — matches numpy.linalg.eigvals). */
static void *zap_linalg_eigvals_impl(ZapArray *a, int64_t n) {
    double *A = zap_linalg_colmajor(a, n); /* dgeev overwrites A */
    double *wr = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    double *wi = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    int ni = (int)n, info = 0, lw = -1;
    double wq = 0;
    dgeev_("N", "N", &ni, A, &ni, wr, wi, NULL, &ni, NULL, &ni, &wq, &lw, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    dgeev_("N", "N", &ni, A, &ni, wr, wi, NULL, &ni, NULL, &ni, work, &lw, &info);
    if (info != 0)
        zap_linalg_err("linalg.eigvals: eigenvalue iteration did not converge");
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_COMPLEX, n);
    for (int64_t i = 0; i < n; i++)
        zap_cx_store(res, i, wr[i], wi[i]);
    return res;
}

/* eig eigenvectors (#928-eig), general matrix via `dgeev` JOBVR='V'. The right
 * eigenvectors come back REAL-packed in `vr` (column-major): a real eigenvalue's
 * eigenvector is that column; a complex conjugate pair (wi[j] > 0, wi[j+1] < 0)
 * has eigenvectors vr[:,j] ± i·vr[:,j+1]. Returns the complex eigenvector matrix
 * whose COLUMNS are the eigenvectors, matching numpy.linalg.eig's `v`. (The
 * eigenvalues `w` reuse `eigvals_impl`.) */
static void *zap_linalg_eig_v_impl(ZapArray *a, int64_t n) {
    double *A = zap_linalg_colmajor(a, n);
    double *wr = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    double *wi = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    double *vr = (double *)zap_buf((size_t)(n * n > 0 ? n * n : 1) * sizeof(double));
    int ni = (int)n, info = 0, lw = -1;
    double wq = 0;
    dgeev_("N", "V", &ni, A, &ni, wr, wi, NULL, &ni, vr, &ni, &wq, &lw, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    dgeev_("N", "V", &ni, A, &ni, wr, wi, NULL, &ni, vr, &ni, work, &lw, &info);
    if (info != 0)
        zap_linalg_err("linalg.eig: eigenvalue iteration did not converge");
    ZapArray *res = (ZapArray *)zap_array_new_nd(DTYPE_COMPLEX, 2, n * n);
    zap_array_set_dim(res, 0, n);
    zap_array_set_dim(res, 1, n);
    for (int64_t col = 0; col < n; col++) {
        if (wi[col] > 0.0 && col + 1 < n) {
            /* columns col, col+1 are a conjugate pair */
            for (int64_t row = 0; row < n; row++) {
                double re = vr[row + col * n];
                double im = vr[row + (col + 1) * n];
                zap_cx_store(res, row * n + col, re, im);
                zap_cx_store(res, row * n + (col + 1), re, -im);
            }
        } else if (wi[col] == 0.0) {
            for (int64_t row = 0; row < n; row++)
                zap_cx_store(res, row * n + col, vr[row + col * n], 0.0);
        }
        /* wi[col] < 0: the second of a pair, already filled above. */
    }
    return res;
}

/* eigh / eigvalsh (#928), symmetric matrix via `dsyevd` (lower triangle, UPLO=L —
 * a symmetric matrix is its own transpose, so the column-major buffer's lower
 * triangle is correct). Eigenvalues `w` come back in ASCENDING order (NumPy's
 * eigh/eigvalsh convention); with `want_vec`, `A` is overwritten with the
 * orthonormal eigenvectors as columns (column i pairs with w[i]). */
static void zap_linalg_syevd(ZapArray *a, int64_t n, double *w, double *A_out, int want_vec) {
    double *A = zap_linalg_colmajor(a, n);
    const char *jobz = want_vec ? "V" : "N";
    int ni = (int)n, info = 0, lw = -1, liw = -1;
    double wq = 0;
    int iwq = 0;
    dsyevd_(jobz, "L", &ni, A, &ni, w, &wq, &lw, &iwq, &liw, &info);
    lw = (int)wq;
    liw = iwq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    int *iwork = (int *)zap_buf((size_t)(liw > 0 ? liw : 1) * sizeof(int));
    dsyevd_(jobz, "L", &ni, A, &ni, w, work, &lw, iwork, &liw, &info);
    if (info != 0)
        zap_linalg_err("linalg.eigh: eigenvalue computation did not converge");
    if (want_vec && A_out)
        memcpy(A_out, A, (size_t)(n * n) * sizeof(double));
}
static void *zap_linalg_eigh_w_impl(ZapArray *a, int64_t n) {
    double *w = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    zap_linalg_syevd(a, n, w, NULL, 0);
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, n);
    for (int64_t i = 0; i < n; i++)
        arr_store_f(res, i, w[i]);
    return res;
}
static void *zap_linalg_eigh_v_impl(ZapArray *a, int64_t n) {
    double *w = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    double *A = (double *)zap_buf((size_t)(n * n > 0 ? n * n : 1) * sizeof(double));
    zap_linalg_syevd(a, n, w, A, 1);
    ZapArray *res = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * n);
    zap_array_set_dim(res, 0, n);
    zap_array_set_dim(res, 1, n);
    for (int64_t i = 0; i < n; i++)
        for (int64_t j = 0; j < n; j++)
            arr_store_f(res, i * n + j, A[i + j * n]); /* col-major cols -> row */
    return res;
}

/* lstsq (#928), minimum-norm least squares via `dgelsd` (SVD-based, handles
 * rank-deficient a). a is (m x n), b is (m,) or (m x nrhs). Fills: X (the n x nrhs
 * solution, column-major in a max(m,n)-row buffer), S (the min(m,n) singular
 * values of a), the effective rank, and — only when m>n and rank==n — the
 * per-column residual sum of squares (NumPy returns an empty residuals array
 * otherwise). rcond=-1 uses machine precision (NumPy's default cutoff). */
static int zap_linalg_gelsd_run(ZapArray *a, ZapArray *b, int64_t m, int64_t n,
                                int64_t nrhs, int b_is_1d, double *Bout, double *Sout,
                                int64_t *rank_out, int64_t ldb) {
    double *A = zap_linalg_colmajor_mn(a, m, n);
    double *B = (double *)zap_buf((size_t)(ldb * nrhs > 0 ? ldb * nrhs : 1) * sizeof(double));
    memset(B, 0, (size_t)(ldb * nrhs) * sizeof(double));
    for (int64_t i = 0; i < m; i++)
        for (int64_t r = 0; r < nrhs; r++)
            B[i + r * ldb] = b_is_1d ? mm_load_f(b, i) : mm_load_f(b, i * nrhs + r);
    int64_t minmn = m < n ? m : n;
    int im = (int)m, in = (int)n, inr = (int)nrhs, ild = (int)ldb, rank = 0, info = 0, lw = -1;
    double rcond = -1.0, wq = 0;
    int iwq = 0;
    dgelsd_(&im, &in, &inr, A, &im, B, &ild, Sout, &rcond, &rank, &wq, &lw, &iwq, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    int *iwork = (int *)zap_buf((size_t)(iwq > 0 ? iwq : 1) * sizeof(int));
    dgelsd_(&im, &in, &inr, A, &im, B, &ild, Sout, &rcond, &rank, work, &lw, iwork, &info);
    if (info != 0)
        zap_linalg_err("linalg.lstsq: SVD did not converge");
    (void)minmn;
    memcpy(Bout, B, (size_t)(ldb * nrhs) * sizeof(double));
    *rank_out = rank;
    return rank;
}
static void *zap_linalg_lstsq_x_impl(ZapArray *a, ZapArray *b, int64_t m, int64_t n,
                                     int64_t nrhs, int b_is_1d) {
    int64_t ldb = m > n ? m : n;
    double *B = (double *)zap_buf((size_t)(ldb * nrhs) * sizeof(double));
    double *S = (double *)zap_buf((size_t)((m < n ? m : n) > 0 ? (m < n ? m : n) : 1) * sizeof(double));
    int64_t rank = 0;
    zap_linalg_gelsd_run(a, b, m, n, nrhs, b_is_1d, B, S, &rank, ldb);
    ZapArray *res;
    if (b_is_1d) {
        res = (ZapArray *)zap_array_new(DTYPE_F64, n);
        for (int64_t i = 0; i < n; i++)
            arr_store_f(res, i, B[i]);
    } else {
        res = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * nrhs);
        zap_array_set_dim(res, 0, n);
        zap_array_set_dim(res, 1, nrhs);
        for (int64_t i = 0; i < n; i++)
            for (int64_t r = 0; r < nrhs; r++)
                arr_store_f(res, i * nrhs + r, B[i + r * ldb]);
    }
    return res;
}
static void *zap_linalg_lstsq_resid_impl(ZapArray *a, ZapArray *b, int64_t m, int64_t n,
                                         int64_t nrhs, int b_is_1d) {
    int64_t ldb = m > n ? m : n;
    double *B = (double *)zap_buf((size_t)(ldb * nrhs) * sizeof(double));
    double *S = (double *)zap_buf((size_t)((m < n ? m : n) > 0 ? (m < n ? m : n) : 1) * sizeof(double));
    int64_t rank = 0;
    zap_linalg_gelsd_run(a, b, m, n, nrhs, b_is_1d, B, S, &rank, ldb);
    /* NumPy: residuals present only when m>n and rank==n (full column rank, an
     * over-determined system); otherwise an empty (0,) array. */
    if (m > n && rank == n) {
        ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, nrhs);
        for (int64_t r = 0; r < nrhs; r++) {
            double acc = 0.0;
            for (int64_t i = n; i < m; i++) {
                double e = B[i + r * ldb];
                acc += e * e;
            }
            arr_store_f(res, r, acc);
        }
        return res;
    }
    return zap_array_new(DTYPE_F64, 0);
}
static int64_t zap_linalg_lstsq_rank_impl(ZapArray *a, ZapArray *b, int64_t m, int64_t n,
                                          int64_t nrhs, int b_is_1d) {
    int64_t ldb = m > n ? m : n;
    double *B = (double *)zap_buf((size_t)(ldb * nrhs) * sizeof(double));
    double *S = (double *)zap_buf((size_t)((m < n ? m : n) > 0 ? (m < n ? m : n) : 1) * sizeof(double));
    int64_t rank = 0;
    zap_linalg_gelsd_run(a, b, m, n, nrhs, b_is_1d, B, S, &rank, ldb);
    return rank;
}
static void *zap_linalg_lstsq_s_impl(ZapArray *a, ZapArray *b, int64_t m, int64_t n,
                                     int64_t nrhs, int b_is_1d) {
    int64_t ldb = m > n ? m : n;
    int64_t minmn = m < n ? m : n;
    double *B = (double *)zap_buf((size_t)(ldb * nrhs) * sizeof(double));
    double *S = (double *)zap_buf((size_t)(minmn > 0 ? minmn : 1) * sizeof(double));
    int64_t rank = 0;
    zap_linalg_gelsd_run(a, b, m, n, nrhs, b_is_1d, B, S, &rank, ldb);
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, minmn);
    for (int64_t i = 0; i < minmn; i++)
        arr_store_f(res, i, S[i]);
    return res;
}

/* Singular values only (#928), for the matrix 2-/-2-norm — `dgesdd` with jobz=N
 * (no vectors). Descending order. */
static void zap_linalg_singvals(ZapArray *a, int64_t m, int64_t n, double *S) {
    double *cm = zap_linalg_colmajor_mn(a, m, n);
    int im = (int)m, in = (int)n, info = 0, lw = -1;
    int64_t k = m < n ? m : n;
    double wq = 0;
    int *iw = (int *)zap_buf((size_t)(8 * (k > 0 ? k : 1)) * sizeof(int));
    dgesdd_("N", &im, &in, cm, &im, S, NULL, &im, NULL, &im, &wq, &lw, iw, &info);
    lw = (int)wq;
    double *work = (double *)zap_buf((size_t)(lw > 0 ? lw : 1) * sizeof(double));
    dgesdd_("N", &im, &in, cm, &im, S, NULL, &im, NULL, &im, work, &lw, iw, &info);
    if (info != 0)
        zap_linalg_err("linalg.norm: SVD did not converge");
}

/* pinv (#928), Moore-Penrose pseudo-inverse of an (m x n) matrix via the reduced
 * SVD: pinv = V Σ⁺ Uᵀ, reciprocating singular values above rcond·σ_max (NumPy's
 * default rcond = 1e-15). Result is (n x m). */
static void *zap_linalg_pinv_impl(ZapArray *a, int64_t m, int64_t n) {
    int64_t k = m < n ? m : n;
    double *S = (double *)zap_buf((size_t)(k > 0 ? k : 1) * sizeof(double));
    double *Uc = (double *)zap_buf((size_t)(m * k > 0 ? m * k : 1) * sizeof(double));
    double *Vc = (double *)zap_buf((size_t)(k * n > 0 ? k * n : 1) * sizeof(double));
    zap_linalg_svd_run(a, m, n, k, S, Uc, Vc);
    double smax = 0.0;
    for (int64_t i = 0; i < k; i++)
        if (S[i] > smax)
            smax = S[i];
    double cut = smax * 1e-15;
    ZapArray *res = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * m);
    zap_array_set_dim(res, 0, n);
    zap_array_set_dim(res, 1, m);
    for (int64_t j = 0; j < n; j++)
        for (int64_t l = 0; l < m; l++) {
            double acc = 0.0;
            for (int64_t i = 0; i < k; i++)
                if (S[i] > cut)
                    acc += (Vc[i + j * k] / S[i]) * Uc[l + i * m];
            arr_store_f(res, j * m + l, acc);
        }
    return res;
}

/* norm (#928). Vector (1-D) ords: none/2 -> Euclidean, 1 -> Σ|x|, +inf -> max|x|,
 * -inf -> min|x|, p -> (Σ|x|^p)^(1/p). Matrix (2-D) ords: none -> Frobenius,
 * 1/-1 -> max/min column Σ|·|, +inf/-inf -> max/min row Σ|·|, 2/-2 -> largest /
 * smallest singular value. `ord` is NaN when omitted. ('fro'/'nuc' string ords are
 * a follow-up; the default already gives Frobenius.) */
static double zap_linalg_norm_impl(ZapArray *a, double ord, int64_t ndim,
                                   int64_t m, int64_t n) {
    int none = isnan(ord);
    if (ndim == 1) {
        int64_t len = m;
        if (none || ord == 2.0) {
            double s = 0.0;
            for (int64_t i = 0; i < len; i++) {
                double v = mm_load_f(a, i);
                s += v * v;
            }
            return sqrt(s);
        }
        if (ord == 1.0) {
            double s = 0.0;
            for (int64_t i = 0; i < len; i++)
                s += fabs(mm_load_f(a, i));
            return s;
        }
        if (isinf(ord)) {
            double best = (ord > 0) ? 0.0 : HUGE_VAL;
            for (int64_t i = 0; i < len; i++) {
                double v = fabs(mm_load_f(a, i));
                if (ord > 0 ? (v > best) : (v < best))
                    best = v;
            }
            return best;
        }
        double s = 0.0;
        for (int64_t i = 0; i < len; i++)
            s += pow(fabs(mm_load_f(a, i)), ord);
        return pow(s, 1.0 / ord);
    }
    /* matrix */
    if (none) {
        double s = 0.0;
        for (int64_t i = 0; i < m * n; i++) {
            double v = mm_load_f(a, i);
            s += v * v;
        }
        return sqrt(s);
    }
    if (ord == 2.0 || ord == -2.0) {
        int64_t k = m < n ? m : n;
        double *S = (double *)zap_buf((size_t)(k > 0 ? k : 1) * sizeof(double));
        zap_linalg_singvals(a, m, n, S);
        double best = S[0];
        for (int64_t i = 1; i < k; i++)
            best = (ord > 0) ? (S[i] > best ? S[i] : best) : (S[i] < best ? S[i] : best);
        return best;
    }
    if (ord == 1.0 || ord == -1.0) { /* max/min column absolute sum */
        double best = (ord > 0) ? 0.0 : HUGE_VAL;
        for (int64_t j = 0; j < n; j++) {
            double c = 0.0;
            for (int64_t i = 0; i < m; i++)
                c += fabs(mm_load_f(a, i * n + j));
            if (ord > 0 ? (c > best) : (c < best))
                best = c;
        }
        return best;
    }
    if (isinf(ord)) { /* max/min row absolute sum */
        double best = (ord > 0) ? 0.0 : HUGE_VAL;
        for (int64_t i = 0; i < m; i++) {
            double r = 0.0;
            for (int64_t j = 0; j < n; j++)
                r += fabs(mm_load_f(a, i * n + j));
            if (ord > 0 ? (r > best) : (r < best))
                best = r;
        }
        return best;
    }
    zap_linalg_err("linalg.norm: unsupported matrix ord (use 1, -1, 2, -2, inf, -inf, or none)");
    return 0.0;
}
#endif

/* Matrix multiply for the array-producing cases (not 1-D·1-D, which is `dot`):
 * a 1-D operand is promoted to a row (left) or column (right) and its singleton
 * axis dropped from the result. `out_dtype` is the NEP-50 promotion the lowering
 * computed; the accumulator is int64 (integer out) or double (float out). */
void *zap_array_matmul(void *ap, void *bp, int64_t out_dtype) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t na = a->ndim, nb = b->ndim;
    int64_t m = (na == 1) ? 1 : a->dims[0];
    int64_t ka = (na == 1) ? a->dims[0] : a->dims[1];
    int64_t kb = b->dims[0];
    int64_t n = (nb == 1) ? 1 : b->dims[1];
    if (ka != kb) {
        zap_array_matmul_mismatch();
    }
    int64_t kk = ka;
    ZapArray *res;
    if (na == 2 && nb == 2) {
        res = (ZapArray *)zap_array_new_nd(out_dtype, 2, m * n);
        zap_array_set_dim(res, 0, m);
        zap_array_set_dim(res, 1, n);
    } else {
        /* (1,k)·(k,n) -> (n,) ; (m,k)·(k,) -> (m,) */
        int64_t len = (na == 1) ? n : m;
        res = (ZapArray *)zap_array_new(out_dtype, len);
    }
    int is_float = (out_dtype == DTYPE_F32 || out_dtype == DTYPE_F64);
    /* Dispatch (#904, D-0181): integer always own (exact); float -> BLAS when a
     * provider is linked and (ZAP_ACCEL=fast, or auto past the floor), else own.
     * The whole BLAS branch is compiled out when no provider is built in, so a
     * no-BLAS build is always the own floor and references no cblas symbol. */
    int use_blas = 0;
#if defined(ZAP_HAVE_BLAS)
    if (is_float && zap_accel_available()) {
        int64_t tier = zap_accel_tier(); /* 0 auto, 1 native, 2 fast */
        if (tier != 1 &&
            (tier == 2 || (double)m * (double)n * (double)kk >= ZAP_MATMUL_BLAS_FLOOR)) {
            use_blas = 1;
        }
    }
    if (use_blas) {
        if (out_dtype == DTYPE_F64) {
            zap_matmul_blas_f64(a, b, res, m, n, kk);
        } else {
            zap_matmul_blas_f32(a, b, res, m, n, kk);
        }
    }
#endif
    if (!use_blas) {
        if (is_float) {
            zap_matmul_f(a, b, res, m, n, kk);
        } else {
            zap_matmul_i(a, b, res, m, n, kk);
        }
    }
    return res;
}

/* Public `import linalg` entry points (#926, D-0183). ALWAYS compiled so a
 * program that uses linalg links; the LAPACK impls above need a provider, so a
 * build with none raises a catchable ValueError (the rare exotic host — normal
 * builds ship a provider). Shape validation (square 2-D `a`, compatible `b`) is
 * here, provider-independent. Results are f64. */
static void zap_linalg_need_provider(void) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                    zap_str_new("linalg requires a BLAS/LAPACK provider "
                                "(none linked in this build)",
                                58));
}
static int64_t zap_linalg_square(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim != 2 || a->dims[0] != a->dims[1])
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg: expected a square 2-D matrix", 36));
    return a->dims[0];
}

void *zap_linalg_solve(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t n = zap_linalg_square(a);
    int b_is_1d = (b->ndim == 1);
    if (b->ndim != 1 && b->ndim != 2)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg.solve: b must be 1-D or 2-D", 34));
    if (b->dims[0] != n)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg.solve: a and b shapes are not aligned", 44));
    int64_t nrhs = b_is_1d ? 1 : b->dims[1];
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_solve_impl(a, b, n, nrhs, b_is_1d);
#else
    (void)nrhs;
    zap_linalg_need_provider();
    return NULL;
#endif
}

void *zap_linalg_inv(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_inv_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}

double zap_linalg_det(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_det_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return 0.0;
#endif
}

/* Validate a 2-D matrix (qr/svd accept any m*n, not just square), returning the
 * dims via out-params. */
static void zap_linalg_rect(void *ap, int64_t *m, int64_t *n) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim != 2)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg: expected a 2-D matrix", 29));
    *m = a->dims[0];
    *n = a->dims[1];
}

void *zap_linalg_cholesky(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_cholesky_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_qr_q(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t m = 0, n = 0;
    zap_linalg_rect(a, &m, &n);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_qr_q_impl(a, m, n);
#else
    (void)m;
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_qr_r(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t m = 0, n = 0;
    zap_linalg_rect(a, &m, &n);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_qr_r_impl(a, m, n);
#else
    (void)m;
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_svd_u(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t m = 0, n = 0;
    zap_linalg_rect(a, &m, &n);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_svd_u_impl(a, m, n);
#else
    (void)m;
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_svd_s(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t m = 0, n = 0;
    zap_linalg_rect(a, &m, &n);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_svd_s_impl(a, m, n);
#else
    (void)m;
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_svd_vt(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t m = 0, n = 0;
    zap_linalg_rect(a, &m, &n);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_svd_vt_impl(a, m, n);
#else
    (void)m;
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}

/* --- #928 eigensolvers / least squares / norms / pinv / matrix_power ------- */

void *zap_linalg_eigvals(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_eigvals_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
/* eig -> (w, v): the eigenvalues (reusing eigvals) and the complex eigenvector
 * matrix (#928-eig). Two entry points assembled into a tuple by the lowering. */
void *zap_linalg_eig_w(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_eigvals_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_eig_v(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_eig_v_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_eigh_w(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_eigh_w_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_eigh_v(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_eigh_v_impl(a, n);
#else
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}

/* lstsq shape check: a is (m x n), b is (m,) or (m x nrhs). */
static void zap_linalg_lstsq_shapes(ZapArray *a, ZapArray *b, int64_t *m, int64_t *n,
                                    int64_t *nrhs, int *b_is_1d) {
    if (a->ndim != 2)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg.lstsq: a must be a 2-D matrix", 36));
    *m = a->dims[0];
    *n = a->dims[1];
    *b_is_1d = (b->ndim == 1);
    if (b->ndim != 1 && b->ndim != 2)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg.lstsq: b must be 1-D or 2-D", 34));
    if (b->dims[0] != *m)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg.lstsq: a and b shapes are not aligned", 44));
    *nrhs = *b_is_1d ? 1 : b->dims[1];
}
void *zap_linalg_lstsq_x(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t m = 0, n = 0, nrhs = 0;
    int b1 = 0;
    zap_linalg_lstsq_shapes(a, b, &m, &n, &nrhs, &b1);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_lstsq_x_impl(a, b, m, n, nrhs, b1);
#else
    zap_linalg_need_provider();
    return NULL;
#endif
}
void *zap_linalg_lstsq_resid(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t m = 0, n = 0, nrhs = 0;
    int b1 = 0;
    zap_linalg_lstsq_shapes(a, b, &m, &n, &nrhs, &b1);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_lstsq_resid_impl(a, b, m, n, nrhs, b1);
#else
    zap_linalg_need_provider();
    return NULL;
#endif
}
int64_t zap_linalg_lstsq_rank(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t m = 0, n = 0, nrhs = 0;
    int b1 = 0;
    zap_linalg_lstsq_shapes(a, b, &m, &n, &nrhs, &b1);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_lstsq_rank_impl(a, b, m, n, nrhs, b1);
#else
    zap_linalg_need_provider();
    return 0;
#endif
}
void *zap_linalg_lstsq_s(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    int64_t m = 0, n = 0, nrhs = 0;
    int b1 = 0;
    zap_linalg_lstsq_shapes(a, b, &m, &n, &nrhs, &b1);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_lstsq_s_impl(a, b, m, n, nrhs, b1);
#else
    zap_linalg_need_provider();
    return NULL;
#endif
}
double zap_linalg_norm(void *ap, double ord) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    if (a->ndim != 1 && a->ndim != 2)
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("linalg.norm: a must be a 1-D or 2-D array", 41));
    int64_t m = a->dims[0];
    int64_t n = (a->ndim == 2) ? a->dims[1] : 1;
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_norm_impl(a, ord, a->ndim, m, n);
#else
    (void)m;
    (void)n;
    (void)ord;
    zap_linalg_need_provider();
    return 0.0;
#endif
}
void *zap_linalg_pinv(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t m = 0, n = 0;
    zap_linalg_rect(a, &m, &n);
#if defined(ZAP_HAVE_BLAS)
    return zap_linalg_pinv_impl(a, m, n);
#else
    (void)m;
    (void)n;
    zap_linalg_need_provider();
    return NULL;
#endif
}
/* matrix_power (#928): a**p for an integer p, by binary exponentiation over the
 * array matmul (own floor or BLAS, #904). p==0 -> identity; p<0 -> (a⁻¹)**|p|
 * (the inverse needs a provider, so a negative power on a provider-less build
 * raises through `zap_linalg_inv`). Computed in f64. Always compiled: a
 * non-negative power is pure matmul and needs no provider. */
static void *zap_linalg_identity(int64_t n) {
    ZapArray *id = (ZapArray *)zap_array_new_nd(DTYPE_F64, 2, n * n);
    zap_array_set_dim(id, 0, n);
    zap_array_set_dim(id, 1, n);
    for (int64_t i = 0; i < n; i++)
        for (int64_t j = 0; j < n; j++)
            arr_store_f(id, i * n + j, i == j ? 1.0 : 0.0);
    return id;
}
void *zap_linalg_matrix_power(void *ap, int64_t p) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_linalg_square(a);
    void *base = a;
    int64_t e = p;
    if (p < 0) {
        base = zap_linalg_inv(ap); /* raises if no provider */
        e = -p;
    }
    void *result = zap_linalg_identity(n);
    while (e > 0) {
        if (e & 1)
            result = zap_array_matmul(result, base, DTYPE_F64);
        e >>= 1;
        if (e > 0)
            base = zap_array_matmul(base, base, DTYPE_F64);
    }
    return result;
}

/* --- element-wise comparison & mask combination (#510, D-0175) -----------
 * `a > 3`, `a == b`, ... produce a fresh `array[bool]` (NumPy); `&`/`|`/`^`
 * combine bool masks and `~` inverts one. These are the mask *source* for
 * boolean-mask indexing. Comparison picks a numeric domain matching NumPy's
 * result_type: any float operand -> double; a `u64` paired with a signed
 * operand -> double (NumPy promotes that pair to float64); two unsigned
 * operands -> uint64; otherwise int64. `pred` is 0 Eq, 1 Ne, 2 Lt, 3 Le,
 * 4 Gt, 5 Ge. Broadcasting reuses `zap_array_bcast_offsets` (stride-aware),
 * so a strided view never needs compacting first. */
static int zap_dt_is_float(int64_t d) { return d == DTYPE_F32 || d == DTYPE_F64; }
static int zap_dt_is_unsigned(int64_t d) {
    return d == DTYPE_U8 || d == DTYPE_U16 || d == DTYPE_U32 || d == DTYPE_U64 ||
           d == DTYPE_BOOL;
}
enum { CMP_DOM_I = 0, CMP_DOM_U = 1, CMP_DOM_F = 2 };
static int cmp_domain(int64_t da, int64_t db) {
    if (zap_dt_is_float(da) || zap_dt_is_float(db)) {
        return CMP_DOM_F;
    }
    int ua = zap_dt_is_unsigned(da), ub = zap_dt_is_unsigned(db);
    if (ua && ub) {
        return CMP_DOM_U;
    }
    if ((da == DTYPE_U64 && !ub) || (db == DTYPE_U64 && !ua)) {
        return CMP_DOM_F; /* u64 vs signed -> float64, matching NumPy */
    }
    return CMP_DOM_I;
}
static int cmp_f(double x, double y, int64_t pred) {
    switch (pred) {
    case 0: return x == y;
    case 1: return x != y;
    case 2: return x < y;
    case 3: return x <= y;
    case 4: return x > y;
    default: return x >= y;
    }
}
static int cmp_i(int64_t x, int64_t y, int64_t pred) {
    switch (pred) {
    case 0: return x == y;
    case 1: return x != y;
    case 2: return x < y;
    case 3: return x <= y;
    case 4: return x > y;
    default: return x >= y;
    }
}
static int cmp_u(uint64_t x, uint64_t y, int64_t pred) {
    switch (pred) {
    case 0: return x == y;
    case 1: return x != y;
    case 2: return x < y;
    case 3: return x <= y;
    case 4: return x > y;
    default: return x >= y;
    }
}

/* `a <cmp> b` element-wise -> array[bool], with NumPy broadcasting. */
void *zap_array_cmp(void *ap, void *bp, int64_t pred) {
    zap_array_broadcast_check(ap, bp);
    ZapArray *res = (ZapArray *)zap_array_broadcast_new(ap, bp, DTYPE_BOOL);
    int64_t n = zap_array_count_of(res);
    ZapArray *a = (ZapArray *)ap, *b = (ZapArray *)bp;
    int dom = cmp_domain(a->dtype, b->dtype);
    uint8_t *out = (uint8_t *)res->data;
    for (int64_t f = 0; f < n; f++) {
        int64_t oa = 0, ob = 0;
        zap_array_bcast_offsets(ap, bp, res, f, &oa, &ob);
        int v;
        if (dom == CMP_DOM_F) {
            v = cmp_f(mm_load_f(a, oa), mm_load_f(b, ob), pred);
        } else if (dom == CMP_DOM_U) {
            v = cmp_u(mm_load_u(a, oa), mm_load_u(b, ob), pred);
        } else {
            v = cmp_i(mm_load_i(a, oa), mm_load_i(b, ob), pred);
        }
        out[f] = v ? 1 : 0;
    }
    return res;
}

/* `a <cmp> scalar` (and `scalar <cmp> a` when `scalar_lhs`) -> array[bool].
 * The weak scalar crosses as its natural word: `si` (int64) or `sf` (double),
 * selected by `s_is_float`. The domain follows NumPy weak-scalar rules. */
void *zap_array_cmp_scalar(void *ap, int64_t si, double sf, int64_t s_is_float,
                           int64_t pred, int64_t scalar_lhs) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *res = (ZapArray *)zap_array_new_like(a, DTYPE_BOOL);
    int64_t n = zap_array_count_of(a);
    uint8_t *out = (uint8_t *)res->data;
    int dom;
    if (s_is_float || zap_dt_is_float(a->dtype)) {
        dom = CMP_DOM_F;
    } else if (zap_dt_is_unsigned(a->dtype)) {
        if (a->dtype == DTYPE_U64 && si < 0) {
            dom = CMP_DOM_F;
        } else if (si >= 0) {
            dom = CMP_DOM_U;
        } else {
            dom = CMP_DOM_I; /* small unsigned vs negative fits int64 exactly */
        }
    } else {
        dom = CMP_DOM_I;
    }
    for (int64_t i = 0; i < n; i++) {
        int v;
        if (dom == CMP_DOM_F) {
            double x = mm_load_f(a, i), y = s_is_float ? sf : (double)si;
            v = scalar_lhs ? cmp_f(y, x, pred) : cmp_f(x, y, pred);
        } else if (dom == CMP_DOM_U) {
            uint64_t x = mm_load_u(a, i), y = (uint64_t)si;
            v = scalar_lhs ? cmp_u(y, x, pred) : cmp_u(x, y, pred);
        } else {
            int64_t x = mm_load_i(a, i), y = si;
            v = scalar_lhs ? cmp_i(y, x, pred) : cmp_i(x, y, pred);
        }
        out[i] = v ? 1 : 0;
    }
    return res;
}

/* `mask1 & mask2` / `| ` / `^` on bool arrays -> array[bool] (NumPy treats
 * these as logical on bool dtype), with broadcasting. `op`: 0 and, 1 or,
 * 2 xor. Operands are read truthily, so any numeric dtype works, but the
 * lowering restricts this to bool-dtype operands (mask combination). */
static int log_apply(int x, int y, int64_t op) {
    x = x ? 1 : 0;
    y = y ? 1 : 0;
    switch (op) {
    case 0: return x && y;
    case 1: return x || y;
    default: return x ^ y;
    }
}
void *zap_array_logical(void *ap, void *bp, int64_t op) {
    zap_array_broadcast_check(ap, bp);
    ZapArray *res = (ZapArray *)zap_array_broadcast_new(ap, bp, DTYPE_BOOL);
    int64_t n = zap_array_count_of(res);
    ZapArray *a = (ZapArray *)ap, *b = (ZapArray *)bp;
    uint8_t *out = (uint8_t *)res->data;
    for (int64_t f = 0; f < n; f++) {
        int64_t oa = 0, ob = 0;
        zap_array_bcast_offsets(ap, bp, res, f, &oa, &ob);
        out[f] = log_apply((int)mm_load_i(a, oa), (int)mm_load_i(b, ob), op) ? 1 : 0;
    }
    return res;
}
/* Raised for the truth value of an array in a boolean context (`if a:`,
 * `a and b`, `not a`, chained `lo < a < hi`) — NumPy's "ambiguous" rule. */
void zap_array_truth_ambiguous(void) {
    const char *m = "the truth value of an array is ambiguous; use .any() or "
                    ".all(), or combine masks with & / | / ~";
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}
/* `~mask` -> array[bool], logical not over a bool array. */
void *zap_array_lognot(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *res = (ZapArray *)zap_array_new_like(a, DTYPE_BOOL);
    int64_t n = zap_array_count_of(a);
    uint8_t *out = (uint8_t *)res->data;
    for (int64_t i = 0; i < n; i++) {
        out[i] = mm_load_i(a, i) ? 0 : 1;
    }
    return res;
}

/* --- boolean-mask indexing (#510, D-0175) --------------------------------
 * `a[mask]`: the mask is applied to the LEADING `mask.ndim` axes of `a`
 * (NumPy), which must match `a`'s dims there. Each True position selects the
 * contiguous trailing "block" (product of `a.dims[mask.ndim:]`). The result is
 * a COPY (advanced indexing never aliases, unlike a slice view) of rank
 * `1 + (a.ndim - mask.ndim)`: a leading axis of length = #True, then the
 * untouched trailing dims (so a full-shape mask yields a 1-D array). Assignment
 * writes back into `a`'s real storage, so it maps each slot through
 * `zap_array_phys_nd` (never compacting the target). A shape/rank mismatch
 * raises a catchable `IndexError`, matching NumPy. */
static void zap_array_index_err(const char *m) {
    zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}
static void mask_check(const ZapArray *a, const ZapArray *m) {
    if (m->ndim > a->ndim) {
        zap_array_index_err("boolean index has more dimensions than the array");
    }
    for (int64_t k = 0; k < m->ndim; k++) {
        if (m->dims[k] != a->dims[k]) {
            zap_array_index_err(
                "boolean index did not match the indexed array: dimension mismatch");
        }
    }
}
static int64_t mask_block(const ZapArray *a, const ZapArray *m) {
    int64_t b = 1;
    for (int64_t k = m->ndim; k < a->ndim; k++) {
        b *= a->dims[k];
    }
    return b;
}
static int64_t mask_ntrue(const ZapArray *m) {
    int64_t n = zap_array_count_of(m), c = 0;
    for (int64_t i = 0; i < n; i++) {
        c += mm_load_i(m, i) ? 1 : 0;
    }
    return c;
}

void *zap_array_mask_select(void *ap, void *mp) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *m = (ZapArray *)zap_array_contig(mp);
    mask_check(a, m);
    int64_t block = mask_block(a, m);
    int64_t mcount = zap_array_count_of(m);
    int64_t ntrue = mask_ntrue(m);
    int64_t rndim = 1 + (a->ndim - m->ndim);
    ZapArray *res = (ZapArray *)zap_array_new_nd(a->dtype, rndim, ntrue * block);
    res->dims[0] = ntrue;
    for (int64_t k = m->ndim; k < a->ndim; k++) {
        res->dims[1 + (k - m->ndim)] = a->dims[k];
    }
    size_t esz = zap_array_elem_size(a->dtype);
    char *ad = (char *)zap_array_data(a);
    char *rd = (char *)res->data;
    int64_t w = 0;
    for (int64_t i = 0; i < mcount; i++) {
        if (mm_load_i(m, i)) {
            memcpy(rd + (size_t)w * block * esz, ad + (size_t)i * block * esz,
                   (size_t)block * esz);
            w++;
        }
    }
    return res;
}

/* `a[mask] = scalar` (weak scalar word si/sf) — fill every selected slot. */
void zap_array_mask_assign_scalar(void *ap, void *mp, int64_t si, double sf,
                                  int64_t is_float) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *m = (ZapArray *)zap_array_contig(mp);
    mask_check(a, m);
    int64_t block = mask_block(a, m);
    int64_t mcount = zap_array_count_of(m);
    int af = zap_dt_is_float(a->dtype);
    for (int64_t i = 0; i < mcount; i++) {
        if (!mm_load_i(m, i)) {
            continue;
        }
        for (int64_t j = 0; j < block; j++) {
            int64_t phys = zap_array_phys_nd(a, i * block + j);
            if (af) {
                arr_store_f(a, phys, is_float ? sf : (double)si);
            } else {
                arr_store_i(a, phys, is_float ? (int64_t)sf : si);
            }
        }
    }
}

/* `a[mask] = values` — values must have exactly `#True * block` elements (the
 * selection's element count), or be a single element (broadcast fill). */
void zap_array_mask_assign_array(void *ap, void *mp, void *vp) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *m = (ZapArray *)zap_array_contig(mp);
    mask_check(a, m);
    int64_t block = mask_block(a, m);
    int64_t mcount = zap_array_count_of(m);
    int64_t ntrue = mask_ntrue(m);
    ZapArray *v = (ZapArray *)zap_array_contig(vp);
    int64_t vcount = zap_array_count_of(v);
    int64_t need = ntrue * block;
    int vscalar = (vcount == 1);
    if (!vscalar && vcount != need) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_str_new("cannot assign the input values to the masked selection: "
                        "count mismatch",
                        61));
    }
    int af = zap_dt_is_float(a->dtype);
    int64_t sel = 0;
    for (int64_t i = 0; i < mcount; i++) {
        if (!mm_load_i(m, i)) {
            continue;
        }
        for (int64_t j = 0; j < block; j++) {
            int64_t phys = zap_array_phys_nd(a, i * block + j);
            int64_t vidx = vscalar ? 0 : (sel * block + j);
            if (af) {
                arr_store_f(a, phys, mm_load_f(v, vidx));
            } else {
                arr_store_i(a, phys, mm_load_i(v, vidx));
            }
        }
        sel++;
    }
}

/* --- fancy integer-array indexing (#510, D-0175) -------------------------
 * `a[idx]` gathers along axis 0: the result is `idx.shape + a.shape[1:]`, a
 * fresh COPY. Each index wraps once for negatives (Python/NumPy) and is bounds-
 * checked against axis 0 (a catchable IndexError). Assignment scatters back into
 * `a`'s real storage (mapped through `zap_array_phys_nd`); duplicate indices
 * resolve last-write-wins, matching NumPy (not `add.at` accumulation). */
static int64_t fancy_axis0(int64_t j, int64_t n0) {
    if (j < 0) {
        j += n0;
    }
    if (j < 0 || j >= n0) {
        zap_array_index_err("fancy index out of bounds for axis 0");
    }
    return j;
}

void *zap_array_take(void *ap, void *ip) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *idx = (ZapArray *)zap_array_contig(ip);
    int64_t n0 = a->dims[0];
    int64_t block = 1;
    for (int64_t k = 1; k < a->ndim; k++) {
        block *= a->dims[k];
    }
    int64_t icount = zap_array_count_of(idx);
    int64_t rndim = idx->ndim + a->ndim - 1;
    ZapArray *res = (ZapArray *)zap_array_new_nd(a->dtype, rndim, icount * block);
    for (int64_t k = 0; k < idx->ndim; k++) {
        res->dims[k] = idx->dims[k];
    }
    for (int64_t k = 1; k < a->ndim; k++) {
        res->dims[idx->ndim + (k - 1)] = a->dims[k];
    }
    size_t esz = zap_array_elem_size(a->dtype);
    char *ad = (char *)zap_array_data(a);
    char *rd = (char *)res->data;
    for (int64_t ii = 0; ii < icount; ii++) {
        int64_t j = fancy_axis0(mm_load_i(idx, ii), n0);
        memcpy(rd + (size_t)ii * block * esz, ad + (size_t)j * block * esz,
               (size_t)block * esz);
    }
    return res;
}

/* `a[idx] = scalar` — set the gathered block of every indexed row. */
void zap_array_scatter_scalar(void *ap, void *ip, int64_t si, double sf,
                              int64_t is_float) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *idx = (ZapArray *)zap_array_contig(ip);
    int64_t n0 = a->dims[0];
    int64_t block = 1;
    for (int64_t k = 1; k < a->ndim; k++) {
        block *= a->dims[k];
    }
    int64_t icount = zap_array_count_of(idx);
    int af = zap_dt_is_float(a->dtype);
    for (int64_t ii = 0; ii < icount; ii++) {
        int64_t j = fancy_axis0(mm_load_i(idx, ii), n0);
        for (int64_t t = 0; t < block; t++) {
            int64_t phys = zap_array_phys_nd(a, j * block + t);
            if (af) {
                arr_store_f(a, phys, is_float ? sf : (double)si);
            } else {
                arr_store_i(a, phys, is_float ? (int64_t)sf : si);
            }
        }
    }
}

/* `a[idx] = values` — values has `idx.count * block` elements (the gather's
 * element count), or a single element (broadcast fill). */
void zap_array_scatter_array(void *ap, void *ip, void *vp) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *idx = (ZapArray *)zap_array_contig(ip);
    int64_t n0 = a->dims[0];
    int64_t block = 1;
    for (int64_t k = 1; k < a->ndim; k++) {
        block *= a->dims[k];
    }
    int64_t icount = zap_array_count_of(idx);
    ZapArray *v = (ZapArray *)zap_array_contig(vp);
    int64_t vcount = zap_array_count_of(v);
    int64_t need = icount * block;
    int vscalar = (vcount == 1);
    if (!vscalar && vcount != need) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_str_new("cannot assign the input values to the fancy selection: "
                        "count mismatch",
                        60));
    }
    int af = zap_dt_is_float(a->dtype);
    for (int64_t ii = 0; ii < icount; ii++) {
        int64_t j = fancy_axis0(mm_load_i(idx, ii), n0);
        for (int64_t t = 0; t < block; t++) {
            int64_t phys = zap_array_phys_nd(a, j * block + t);
            int64_t vidx = vscalar ? 0 : (ii * block + t);
            if (af) {
                arr_store_f(a, phys, mm_load_f(v, vidx));
            } else {
                arr_store_i(a, phys, mm_load_i(v, vidx));
            }
        }
    }
}

/* --- general advanced indexing (#510, D-0175) ----------------------------
 * `a[p0, p1, ...]` where at least one part is an integer index array. Each
 * axis is FULL, a SLICE, an INT scalar, or ADV (an index array); a boolean
 * array part is expanded by the lowering into one ADV int array per axis via
 * nonzero. The advanced arrays broadcast together to shape B; the result places
 * B at the advanced block when the advanced axes are contiguous in the
 * subscript, else at the front (NumPy's transpose rule), with the kept
 * (FULL/SLICE) axes preserved in order. A fresh COPY; out-of-bounds indices
 * raise IndexError, a broadcast mismatch ValueError. */
static void fancy_slice_norm(int64_t n, int64_t lo, int64_t hi, int64_t step,
                             int64_t has_lo, int64_t has_hi, int64_t has_step,
                             int64_t *o_start, int64_t *o_step, int64_t *o_len) {
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        zap_shape_err("slice step cannot be zero");
    }
    int64_t start, stop;
    if (st > 0) {
        start = has_lo ? zap_slice_pos(lo, n) : 0;
        stop = has_hi ? zap_slice_pos(hi, n) : n;
    } else {
        start = has_lo ? zap_slice_neg(lo, n) : n - 1;
        stop = has_hi ? zap_slice_neg(hi, n) : -1;
    }
    int64_t len;
    if (st > 0) {
        len = stop > start ? (stop - start + st - 1) / st : 0;
    } else {
        len = start > stop ? (start - stop + (-st) - 1) / (-st) : 0;
    }
    *o_start = start;
    *o_step = st;
    *o_len = len;
}

void *zap_array_fancy(void *ap, void *specp, int64_t ndim, void *advp) {
    ZapArray *a = (ZapArray *)ap; /* read-only; mapped via phys_nd (handles views) */
    int64_t *spec = (int64_t *)specp;
    ZapList *adv = (ZapList *)advp;
    int64_t *kstart = (int64_t *)zap_buf(ndim * 8);
    int64_t *kstep = (int64_t *)zap_buf(ndim * 8);
    int64_t *klen = (int64_t *)zap_buf(ndim * 8);
    int64_t *fixed = (int64_t *)zap_buf(ndim * 8);
    int64_t *tag = (int64_t *)zap_buf(ndim * 8);
    int64_t *advg = (int64_t *)zap_buf(ndim * 8);
    int64_t G = 0, adv_first = -1, adv_last = -1;
    for (int64_t ax = 0; ax < ndim; ax++) {
        int64_t *r = spec + ax * 7;
        int64_t n = a->dims[ax];
        tag[ax] = r[0];
        if (r[0] == 3) {
            advg[ax] = r[1];
            if (adv_first < 0) {
                adv_first = ax;
            }
            adv_last = ax;
            G++;
        } else if (r[0] == 2) {
            int64_t j = r[1];
            if (j < 0) {
                j += n;
            }
            if (j < 0 || j >= n) {
                zap_array_index_err("index out of bounds for the indexed axis");
            }
            fixed[ax] = j;
        } else if (r[0] == 1) {
            fancy_slice_norm(n, r[1], r[2], r[3], r[4], r[5], r[6], &kstart[ax],
                             &kstep[ax], &klen[ax]);
        } else {
            kstart[ax] = 0;
            kstep[ax] = 1;
            klen[ax] = n;
        }
    }
    /* broadcast the advanced index arrays to shape B */
    int64_t b = 0;
    for (int64_t g = 0; g < G; g++) {
        ZapArray *ia = (ZapArray *)(uintptr_t)adv->data[g];
        if (ia->ndim > b) {
            b = ia->ndim;
        }
    }
    int64_t bb = b > 0 ? b : 1;
    int64_t *B = (int64_t *)zap_buf(bb * 8);
    for (int64_t k = 0; k < b; k++) {
        B[k] = 1;
    }
    for (int64_t g = 0; g < G; g++) {
        ZapArray *ia = (ZapArray *)(uintptr_t)adv->data[g];
        for (int64_t k = 0; k < b; k++) {
            int64_t dk = zap_array_bcast_dim(ia, b, k);
            if (dk != 1) {
                if (B[k] != 1 && B[k] != dk) {
                    zap_panic_typed(
                        ZAP_EXC_VALUE_ERROR,
                        zap_str_new("advanced index arrays could not be broadcast together",
                                    54));
                }
                B[k] = dk;
            }
        }
    }
    int adv_contig = (G == 0) ? 1 : (adv_last - adv_first + 1 == G);
    int64_t K = 0;
    for (int64_t ax = 0; ax < ndim; ax++) {
        if (tag[ax] == 0 || tag[ax] == 1) {
            K++;
        }
    }
    int64_t rndim = b + K;
    if (rndim < 1) {
        rndim = 1;
    }
    int64_t *rk_kind = (int64_t *)zap_buf(rndim * 8); /* 0 = kept axis, 1 = B axis */
    int64_t *rk_val = (int64_t *)zap_buf(rndim * 8);
    int64_t *rdims = (int64_t *)zap_buf(rndim * 8);
    int64_t ri = 0;
    if (adv_contig) {
        int64_t pre_hi = (adv_first < 0) ? ndim : adv_first;
        for (int64_t ax = 0; ax < pre_hi; ax++) {
            if (tag[ax] == 0 || tag[ax] == 1) {
                rk_kind[ri] = 0;
                rk_val[ri] = ax;
                rdims[ri] = klen[ax];
                ri++;
            }
        }
        for (int64_t k = 0; k < b; k++) {
            rk_kind[ri] = 1;
            rk_val[ri] = k;
            rdims[ri] = B[k];
            ri++;
        }
        int64_t post_lo = (adv_last < 0) ? ndim : adv_last + 1;
        for (int64_t ax = post_lo; ax < ndim; ax++) {
            if (tag[ax] == 0 || tag[ax] == 1) {
                rk_kind[ri] = 0;
                rk_val[ri] = ax;
                rdims[ri] = klen[ax];
                ri++;
            }
        }
    } else {
        for (int64_t k = 0; k < b; k++) {
            rk_kind[ri] = 1;
            rk_val[ri] = k;
            rdims[ri] = B[k];
            ri++;
        }
        for (int64_t ax = 0; ax < ndim; ax++) {
            if (tag[ax] == 0 || tag[ax] == 1) {
                rk_kind[ri] = 0;
                rk_val[ri] = ax;
                rdims[ri] = klen[ax];
                ri++;
            }
        }
    }
    int64_t total = 1;
    for (int64_t k = 0; k < rndim; k++) {
        total *= rdims[k];
    }
    ZapArray *res = (ZapArray *)zap_array_new_nd(a->dtype, rndim, total);
    for (int64_t k = 0; k < rndim; k++) {
        res->dims[k] = rdims[k];
    }
    size_t esz = zap_array_elem_size(a->dtype);
    char *rd = (char *)res->data;
    char *ad = (char *)a->data;
    int64_t *srcco = (int64_t *)zap_buf(ndim * 8);
    int64_t *bco = (int64_t *)zap_buf(bb * 8);
    for (int64_t f = 0; f < total; f++) {
        int64_t rem = f;
        for (int64_t k = 0; k < b; k++) {
            bco[k] = 0;
        }
        for (int64_t ax = 0; ax < ndim; ax++) {
            if (tag[ax] == 2) {
                srcco[ax] = fixed[ax];
            }
        }
        for (int64_t rax = rndim - 1; rax >= 0; rax--) {
            int64_t c = rem % rdims[rax];
            rem /= rdims[rax];
            if (rk_kind[rax] == 0) {
                int64_t ax = rk_val[rax];
                srcco[ax] = kstart[ax] + c * kstep[ax];
            } else {
                bco[rk_val[rax]] = c;
            }
        }
        for (int64_t ax = 0; ax < ndim; ax++) {
            if (tag[ax] != 3) {
                continue;
            }
            ZapArray *ia = (ZapArray *)(uintptr_t)adv->data[advg[ax]];
            int64_t off = 0;
            for (int64_t k = 0; k < b; k++) {
                int64_t iax = k - (b - ia->ndim);
                if (iax >= 0 && ia->dims[iax] != 1) {
                    off += bco[k] * zap_array_axis_stride(ia, iax);
                }
            }
            int64_t j = mm_load_i(ia, off);
            int64_t n = a->dims[ax];
            if (j < 0) {
                j += n;
            }
            if (j < 0 || j >= n) {
                zap_array_index_err("fancy index out of bounds");
            }
            srcco[ax] = j;
        }
        int64_t lf = 0;
        for (int64_t ax = 0; ax < ndim; ax++) {
            lf = lf * a->dims[ax] + srcco[ax];
        }
        int64_t phys = zap_array_phys_nd(a, lf);
        memcpy(rd + (size_t)f * esz, ad + (size_t)phys * esz, esz);
    }
    return res;
}

/* --- where(cond, x, y) + nonzero (#510, D-0175) --------------------------
 * `where` selects element-wise from `x` where `cond` is true, else `y`, with
 * full NumPy broadcasting of all three (result rank = max). A scalar `x`/`y`
 * crosses as a 1-element array (built by the lowering) so one code path covers
 * scalars and arrays; the lowering computes `out_dtype` (the NEP-50 promotion of
 * x and y, weak scalars). `nonzero_axis` returns the `axis`-coordinate of each
 * true element in row-major order — the 1-arg `where(cond)` builds a tuple of
 * one such array per axis. */
void *zap_array_from_scalar_i(int64_t v) {
    ZapArray *a = (ZapArray *)zap_array_new(DTYPE_I64, 1);
    ((int64_t *)a->data)[0] = v;
    return a;
}
void *zap_array_from_scalar_f(double v) {
    ZapArray *a = (ZapArray *)zap_array_new(DTYPE_F64, 1);
    ((double *)a->data)[0] = v;
    return a;
}
/* Window-relative element offset of result flat index `f` into one operand
 * under broadcasting (right-aligned axes; a size-1 or missing axis contributes
 * 0). Stride-aware, so a view operand needs no compaction. */
static int64_t zap_bcast_off1(const ZapArray *a, const ZapArray *res, int64_t f) {
    int64_t r = res->ndim, o = 0, rem = f;
    for (int64_t k = r - 1; k >= 0; k--) {
        int64_t idx = rem % res->dims[k];
        rem /= res->dims[k];
        int64_t ax = k - (r - a->ndim);
        if (ax >= 0 && a->dims[ax] != 1) {
            o += idx * zap_array_axis_stride(a, ax);
        }
    }
    return o;
}
void *zap_array_where(void *cp, void *xp, void *yp, int64_t out_dtype) {
    ZapArray *c = (ZapArray *)cp, *x = (ZapArray *)xp, *y = (ZapArray *)yp;
    int64_t r = c->ndim;
    if (x->ndim > r) {
        r = x->ndim;
    }
    if (y->ndim > r) {
        r = y->ndim;
    }
    int64_t *dims = (int64_t *)zap_buf(r * (int64_t)sizeof(int64_t));
    int64_t total = 1;
    for (int64_t k = 0; k < r; k++) {
        int64_t dc = zap_array_bcast_dim(c, r, k);
        int64_t dx = zap_array_bcast_dim(x, r, k);
        int64_t dy = zap_array_bcast_dim(y, r, k);
        int64_t dim = 1;
        int64_t ds[3];
        ds[0] = dc;
        ds[1] = dx;
        ds[2] = dy;
        for (int t = 0; t < 3; t++) {
            int64_t d = ds[t];
            if (d != 1) {
                if (dim != 1 && dim != d) {
                    zap_panic_typed(
                        ZAP_EXC_VALUE_ERROR,
                        zap_str_new("where: operands could not be broadcast together", 48));
                }
                dim = d;
            }
        }
        dims[k] = dim;
        total *= dim;
    }
    ZapArray *res = (ZapArray *)zap_array_new_nd(out_dtype, r, total);
    for (int64_t k = 0; k < r; k++) {
        res->dims[k] = dims[k];
    }
    int outf = zap_dt_is_float(out_dtype);
    for (int64_t f = 0; f < total; f++) {
        int64_t oc = zap_bcast_off1(c, res, f);
        int pick = mm_load_i(c, oc) ? 1 : 0;
        int64_t ox = zap_bcast_off1(x, res, f);
        int64_t oy = zap_bcast_off1(y, res, f);
        if (outf) {
            arr_store_f(res, f, pick ? mm_load_f(x, ox) : mm_load_f(y, oy));
        } else {
            arr_store_i(res, f, pick ? mm_load_i(x, ox) : mm_load_i(y, oy));
        }
    }
    return res;
}
void *zap_array_nonzero_axis(void *cp, int64_t axis) {
    ZapArray *c = (ZapArray *)zap_array_contig(cp);
    int64_t n = zap_array_count_of(c);
    int64_t ntrue = 0;
    for (int64_t i = 0; i < n; i++) {
        ntrue += mm_load_i(c, i) ? 1 : 0;
    }
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_I64, ntrue);
    int64_t *out = (int64_t *)res->data;
    int64_t w = 0;
    for (int64_t i = 0; i < n; i++) {
        if (!mm_load_i(c, i)) {
            continue;
        }
        int64_t rem = i, coord = 0;
        for (int64_t k = c->ndim - 1; k >= 0; k--) {
            int64_t cc = rem % c->dims[k];
            rem /= c->dims[k];
            if (k == axis) {
                coord = cc;
            }
        }
        out[w++] = coord;
    }
    return res;
}

/* --- shape ops (#511, D-0174) --------------------------------------------
 * reshape / transpose / flatten / concatenate / stack over the N-D shape model.
 * transpose is a pure stride view; reshape is a view (contiguous source) or a
 * compacted copy; flatten/concatenate/stack produce fresh contiguous copies.
 * All error paths raise a catchable ValueError. */
static void zap_shape_err(const char *m) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}

/* `a.transpose()` / `a.T` (#511): reverse all axes. A view — the dims and
 * (effective) strides are reversed, so a contiguous source becomes a strided
 * view; a 1-D array transposes to itself (stays contiguous). */
void *zap_array_transpose(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    int64_t nd = a->ndim;
    ZapArray *v = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf((size_t)nd * sizeof(int64_t));
    for (int64_t k = 0; k < nd; k++) {
        dims[k] = a->dims[nd - 1 - k];
    }
    int64_t *strides = NULL;
    if (nd >= 2) {
        strides = (int64_t *)zap_buf((size_t)nd * sizeof(int64_t));
        for (int64_t k = 0; k < nd; k++) {
            strides[k] = zap_array_stride_of(a, nd - 1 - k);
        }
    }
    v->dtype = a->dtype;
    v->ndim = nd;
    v->dims = dims;
    v->strides = strides; /* NULL for 1-D (still contiguous) */
    v->data = a->data;
    v->off = a->off;
    return v;
}

/* `a.flatten()` (#511): a fresh contiguous 1-D COPY (row-major). */
void *zap_array_flatten(void *ap) {
    ZapArray *c = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_array_count_of(c);
    size_t esz = zap_array_elem_size(c->dtype);
    ZapArray *r = (ZapArray *)zap_array_new(c->dtype, n);
    if (n > 0) {
        memcpy(r->data, (const char *)c->data + (size_t)c->off * esz,
               (size_t)n * esz);
    }
    return r;
}

/* A small atomic i64 buffer the lowering fills with reshape's target dims (one
 * `-1` allowed). Atomic (no GC pointers). */
void *zap_i64buf(int64_t n) {
    return zap_buf((size_t)(n > 0 ? n : 1) * sizeof(int64_t));
}
void zap_i64set(void *b, int64_t i, int64_t v) { ((int64_t *)b)[i] = v; }

/* `a.reshape(dims...)` (#511): a view sharing the (compacted) buffer with new
 * dims. One dim may be -1 (inferred). Total must match the element count. */
void *zap_array_reshape(void *ap, int64_t ndim, int64_t *dims) {
    ZapArray *c = (ZapArray *)zap_array_contig(ap);
    int64_t count = zap_array_count_of(c);
    int64_t prod = 1, neg = -1;
    for (int64_t k = 0; k < ndim; k++) {
        if (dims[k] < 0) {
            if (neg >= 0) {
                zap_shape_err("reshape: only one dimension can be -1");
            }
            neg = k;
        } else {
            prod *= dims[k];
        }
    }
    int64_t *out = (int64_t *)zap_buf((size_t)(ndim > 0 ? ndim : 1) * sizeof(int64_t));
    for (int64_t k = 0; k < ndim; k++) {
        out[k] = dims[k];
    }
    if (neg >= 0) {
        if (prod == 0 || count % prod != 0) {
            zap_shape_err("reshape: cannot infer the -1 dimension");
        }
        out[neg] = count / prod;
        prod *= out[neg];
    }
    if (prod != count) {
        zap_shape_err("reshape: total size must be unchanged");
    }
    ZapArray *v = (ZapArray *)zap_obj(&zap_desc_array);
    v->dtype = c->dtype;
    v->ndim = ndim;
    v->dims = out;
    v->strides = NULL; /* c is contiguous, so the reshaped view is too */
    v->data = c->data;
    v->off = c->off;
    return v;
}

/* `concatenate([a, b, ...], axis)` (#511): join same-rank, same-dtype arrays
 * along `axis` (all other dims must match). A fresh contiguous copy. */
void *zap_array_concat(void *listp, int64_t axis) {
    ZapList *L = (ZapList *)listp;
    int64_t n = L->len;
    if (n < 1) {
        zap_shape_err("concatenate needs at least one array");
    }
    ZapArray *first = (ZapArray *)(uintptr_t)L->data[0];
    int64_t nd = first->ndim, dt = first->dtype;
    if (axis < 0) {
        axis += nd;
    }
    if (axis < 0 || axis >= nd) {
        zap_shape_err("concatenate: axis out of range");
    }
    int64_t axis_total = 0;
    for (int64_t i = 0; i < n; i++) {
        ZapArray *x = (ZapArray *)(uintptr_t)L->data[i];
        if (x->ndim != nd || x->dtype != dt) {
            zap_shape_err("concatenate: all arrays need the same rank and dtype");
        }
        for (int64_t k = 0; k < nd; k++) {
            if (k != axis && x->dims[k] != first->dims[k]) {
                zap_shape_err("concatenate: dims must match except on the join axis");
            }
        }
        axis_total += x->dims[axis];
    }
    int64_t outer = 1, inner = 1;
    for (int64_t k = 0; k < axis; k++) {
        outer *= first->dims[k];
    }
    for (int64_t k = axis + 1; k < nd; k++) {
        inner *= first->dims[k];
    }
    ZapArray *r = (ZapArray *)zap_array_new_nd(dt, nd, outer * axis_total * inner);
    for (int64_t k = 0; k < nd; k++) {
        zap_array_set_dim(r, k, k == axis ? axis_total : first->dims[k]);
    }
    size_t esz = zap_array_elem_size(dt);
    char *rd = (char *)r->data;
    int64_t run = 0;
    for (int64_t i = 0; i < n; i++) {
        ZapArray *xi = (ZapArray *)zap_array_contig((void *)(uintptr_t)L->data[i]);
        int64_t ax = xi->dims[axis];
        const char *xd = (const char *)xi->data + (size_t)xi->off * esz;
        for (int64_t o = 0; o < outer; o++) {
            memcpy(rd + (size_t)((o * axis_total + run) * inner) * esz,
                   xd + (size_t)(o * ax * inner) * esz, (size_t)(ax * inner) * esz);
        }
        run += ax;
    }
    return r;
}

/* `stack([a, b, ...], axis)` (#511): join same-shape arrays along a NEW axis
 * (result rank = input rank + 1). A fresh contiguous copy. */
void *zap_array_stack(void *listp, int64_t axis) {
    ZapList *L = (ZapList *)listp;
    int64_t n = L->len;
    if (n < 1) {
        zap_shape_err("stack needs at least one array");
    }
    ZapArray *first = (ZapArray *)(uintptr_t)L->data[0];
    int64_t nd = first->ndim, dt = first->dtype, rnd = nd + 1;
    if (axis < 0) {
        axis += rnd;
    }
    if (axis < 0 || axis >= rnd) {
        zap_shape_err("stack: axis out of range");
    }
    for (int64_t i = 0; i < n; i++) {
        ZapArray *x = (ZapArray *)(uintptr_t)L->data[i];
        if (x->ndim != nd || x->dtype != dt) {
            zap_shape_err("stack: all arrays need the same rank and dtype");
        }
        for (int64_t k = 0; k < nd; k++) {
            if (x->dims[k] != first->dims[k]) {
                zap_shape_err("stack: all arrays need the same shape");
            }
        }
    }
    int64_t count = zap_array_count_of(first);
    ZapArray *r = (ZapArray *)zap_array_new_nd(dt, rnd, n * count);
    for (int64_t k = 0; k < rnd; k++) {
        zap_array_set_dim(r, k, k == axis ? n : first->dims[k < axis ? k : k - 1]);
    }
    int64_t outer = 1, inner = 1;
    for (int64_t k = 0; k < axis; k++) {
        outer *= first->dims[k];
    }
    for (int64_t k = axis; k < nd; k++) {
        inner *= first->dims[k];
    }
    size_t esz = zap_array_elem_size(dt);
    char *rd = (char *)r->data;
    for (int64_t i = 0; i < n; i++) {
        ZapArray *xi = (ZapArray *)zap_array_contig((void *)(uintptr_t)L->data[i]);
        const char *xd = (const char *)xi->data + (size_t)xi->off * esz;
        for (int64_t o = 0; o < outer; o++) {
            memcpy(rd + (size_t)((o * n + i) * inner) * esz,
                   xd + (size_t)(o * inner) * esz, (size_t)inner * esz);
        }
    }
    return r;
}

/* zeros/ones/full: fill an already-allocated array with a constant. `_i` takes
 * the value as an int64 word (integer/bool dtypes), `_f` as a double (floats);
 * the lowering picks the variant by the dtype's ABI word. */
void zap_array_fill_i(void *ap, int64_t v) {
    ZapArray *a = (ZapArray *)ap;
    int64_t n = zap_array_count_of(a);
    for (int64_t i = 0; i < n; i++) {
        arr_store_i(a, i, v);
    }
}
void zap_array_fill_f(void *ap, double v) {
    ZapArray *a = (ZapArray *)ap;
    int64_t n = zap_array_count_of(a);
    for (int64_t i = 0; i < n; i++) {
        arr_store_f(a, i, v);
    }
}

/* arange: a 1-D array of `start, start+step, ...` stopping before `stop` (the
 * length is runtime, like NumPy). `_i` for integer dtypes, `_f` for floats.
 * A zero step raises a catchable ValueError. */
void *zap_array_arange_i(int64_t dtype, int64_t start, int64_t stop, int64_t step) {
    if (step == 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("arange step must be nonzero", 27));
    }
    int64_t n = 0;
    if (step > 0 && stop > start) {
        n = (stop - start + step - 1) / step;
    } else if (step < 0 && stop < start) {
        n = (start - stop + (-step) - 1) / (-step);
    }
    ZapArray *a = (ZapArray *)zap_array_new(dtype, n);
    for (int64_t i = 0; i < n; i++) {
        arr_store_i(a, i, start + i * step);
    }
    return a;
}
void *zap_array_arange_f(int64_t dtype, double start, double stop, double step) {
    if (step == 0.0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("arange step must be nonzero", 27));
    }
    double raw = (stop - start) / step;
    int64_t n = raw > 0.0 ? (int64_t)ceil(raw) : 0;
    ZapArray *a = (ZapArray *)zap_array_new(dtype, n);
    int is_f = (dtype == DTYPE_F32 || dtype == DTYPE_F64);
    for (int64_t i = 0; i < n; i++) {
        double v = start + (double)i * step;
        if (is_f) {
            arr_store_f(a, i, v);
        } else {
            arr_store_i(a, i, (int64_t)v);
        }
    }
    return a;
}

/* linspace: `num` evenly-spaced samples from `start` to `stop` inclusive of both
 * ends (NumPy default). `num == 1` yields `[start]`; `num <= 0` an empty array.
 * The last sample is set to `stop` exactly to avoid floating-point drift. */
void *zap_array_linspace(int64_t dtype, double start, double stop, int64_t num) {
    int64_t n = num > 0 ? num : 0;
    ZapArray *a = (ZapArray *)zap_array_new(dtype, n);
    int is_f = (dtype == DTYPE_F32 || dtype == DTYPE_F64);
    double step = n > 1 ? (stop - start) / (double)(n - 1) : 0.0;
    for (int64_t i = 0; i < n; i++) {
        double v = (i == n - 1) ? stop : start + (double)i * step;
        if (is_f) {
            arr_store_f(a, i, v);
        } else {
            arr_store_i(a, i, (int64_t)v);
        }
    }
    return a;
}

/* ---------------------------------------------------------------------------
 * Exceptions / unwinding (ticket #22, design/RUNTIME-ABI.md §"Exception /
 * unwinding ABI"). Zap uses the platform C++ unwinder: `zap_panic` raises,
 * landing pads catch via the libstdc++ personality (`__gxx_personality_v0` on
 * Linux/macOS, `__gxx_personality_seh0` on MinGW-w64 Windows — the name is chosen
 * by codegen from the target triple, ticket #197/A3; the glue below is identical
 * across both unwind models). The C++ clause catches Zap's single transport
 * type; language-level dispatch uses the runtime type id in the Zap object.
 *
 * The thrown C++ exception boxes a pointer to a typed Zap exception object.
 * Keeping all the C++-ABI glue here lets codegen stay a mechanical ZIR->LLVM
 * translator (it only emits `invoke`/`landingpad` and names these entry points).
 * ------------------------------------------------------------------------- */

/* libstdc++/libc++abi entry points (declared here, linked via -lstdc++). */
extern void *__cxa_allocate_exception(size_t size);
extern void __cxa_throw(void *thrown, void *type_info, void (*dest)(void *));
extern void *__cxa_begin_catch(void *exc);
extern void __cxa_end_catch(void);

/* A minimal `std::type_info` for Zap exceptions: its vtable pointer is that of
 * libstdc++'s `__cxxabiv1::__class_type_info` (past the two reserved slots), so
 * the personality treats it as an ordinary class type. A catch-all landing-pad
 * clause (`catch ptr null`) matches it regardless of its name. */
extern const void *const _ZTVN10__cxxabiv117__class_type_infoE[];
typedef struct {
    const void *vtable;
    const char *name;
} ZapTypeInfo;
const ZapTypeInfo zap_exception_type_info = {
    (const void *)&_ZTVN10__cxxabiv117__class_type_infoE[2],
    "ZapException",
};

/* Keep the in-flight Zap exception object reachable while it is boxed inside
 * the (malloc'd, unscanned) C++ exception buffer. */
/* Legacy `raise "message"` and unclassified runtime faults are Exception. */
void zap_panic(void *message) {
    zap_raise(zap_exception_new(ZAP_EXC_EXCEPTION, message));
}

/* Begin unwinding with an already-typed Zap exception object. */
void zap_raise(void *exc) {
    zap_current_exc = exc;
    void **slot = (void **)__cxa_allocate_exception(sizeof(void *));
    *slot = exc;
    __cxa_throw(slot, (void *)&zap_exception_type_info, NULL);
}

void zap_panic_typed(int64_t type_id, void *message) {
    zap_raise(zap_exception_new(type_id, message));
}

/* Arena cap / heap-OOM (#553/#882, D-0158). A generated NULL-check on an arena
 * allocation (`a.alloc`/`a.new`) calls this when the runtime returned NULL; it
 * raises a catchable first-class `MemoryError` (#882,
 * decisions/lang/882-memoryerror-exception.md) and never returns, so the
 * enclosing arena `with` tears the arena down on the unwind path. */
__attribute__((noreturn)) void zap_arena_oom(void) {
    /* The "MemoryError:" prefix is retained in the message because the uncaught
     * reporter (`zap_report_exc_line`) renders only the message string, never
     * the exception's type name, so this keeps an uncaught arena OOM legible. */
    const char *m = "MemoryError: arena allocation exceeds its cap or the heap is exhausted";
    zap_panic_typed(ZAP_EXC_MEMORY_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    abort(); /* unreachable: zap_panic_typed unwinds */
}

/* Landing-pad entry: recover the Zap exception object from the in-flight C++
 * exception and enter the catch. Paired with `zap_eh_end_catch`. */
void *zap_eh_begin_catch(void *unwind_exc) {
    void **slot = (void **)__cxa_begin_catch(unwind_exc);
    return *slot;
}

void zap_eh_end_catch(void) {
    zap_current_exc = NULL;
    __cxa_end_catch();
}

/* An exception reached the program's top-level handler: report it and exit
 * non-zero (RUNTIME-ABI: "unwinds to the top frame and aborts with a message").
 * A richer stack trace is a follow-up. */
/* ---------------------------------------------------------------------------
 * REPL support (I#159, design/0025-repl.md).
 *
 * The REPL (`zap repl`) JIT-compiles each entry into one long-lived process and
 * runs it through `zap_repl_call`, which installs a non-exiting catch so an
 * uncaught exception in an entry returns to the loop instead of ending the
 * session. Session `let`/binding values persist in a fixed table of 64-bit
 * slots whose addresses are stable for the process lifetime: a pointer slot is
 * registered once as a precise-GC root (so its target survives collection and
 * is forwarded under evacuation), and under Boehm the static array is scanned
 * conservatively, so both collectors keep slot pointers alive with no per-entry
 * work. Only the driver (which assigns slot indices) emits slot access, via the
 * `zap_repl_slot_*` entry points; see design/RUNTIME-ABI.md.
 * ------------------------------------------------------------------------- */

/* Installed catch for the entry currently running under `zap_repl_call`, and
 * the exception it caught (read by the driver via `zap_repl_last_exc` right
 * after a non-zero return, before any allocation). Thread-local: entries run on
 * the main thread, like `main`. */
static __thread jmp_buf *zap_repl_catch = NULL;
static __thread void *zap_repl_exc = NULL;

/* Run one REPL entry thunk. Returns 0 on normal completion, 1 if an exception
 * propagated out of the thunk (caught here instead of aborting the process).
 * The thunk is a JIT-compiled `__repl_entry` function carrying `main`'s
 * implicit top-level landing pad, so an uncaught exception reaches
 * `zap_uncaught`, which longjmps back here when a catch is installed. */
int64_t zap_repl_call(void (*thunk)(void)) {
    jmp_buf env;
    jmp_buf *saved = zap_repl_catch;
    zap_repl_catch = &env;
    zap_repl_exc = NULL;
    int64_t rc;
    if (setjmp(env) == 0) {
        thunk();
        rc = 0;
    } else {
        rc = 1;
    }
    zap_repl_catch = saved;
    return rc;
}

/* The `ZapStr *` message of the exception caught by the most recent
 * `zap_repl_call` that returned non-zero (or NULL). */
void *zap_repl_last_exc(void) { return zap_exception_message(zap_repl_exc); }

/* Expose the caught exception message as (data, len) for the Rust driver, which
 * copies it out immediately (before any allocation). Returns the length and
 * sets `*out_data` to the message bytes (NULL / 0 if there was none). */
int64_t zap_repl_last_exc_bytes(const char **out_data) {
    ZapStr *s = (ZapStr *)zap_exception_message(zap_repl_exc);
    if (s == NULL) {
        *out_data = NULL;
        return 0;
    }
    *out_data = s->data;
    return s->len;
}

/* Session binding slots. A fixed, process-lifetime static array gives every
 * cell a stable address; pointer slots are rooted on first store (idempotent). */
#define ZAP_REPL_MAX_SLOTS 4096
static int64_t zap_repl_slots[ZAP_REPL_MAX_SLOTS];
static uint8_t zap_repl_slot_rooted[ZAP_REPL_MAX_SLOTS];

static void zap_sys_fail(const char *op, const char *detail); /* defined below */

static void zap_repl_slot_check(int64_t i) {
    if (i < 0 || i >= ZAP_REPL_MAX_SLOTS) {
        zap_sys_fail("repl", "session has too many live bindings");
    }
}

int64_t zap_repl_slot_get_i64(int64_t i) {
    zap_repl_slot_check(i);
    return zap_repl_slots[i];
}
void zap_repl_slot_set_i64(int64_t i, int64_t v) {
    zap_repl_slot_check(i);
    zap_repl_slots[i] = v;
}
/* Float bindings (I#252) reuse the same 64-bit slot cell, reinterpreted as a
 * `double`. A float is not a GC pointer, so the slot is never rooted. `float32`
 * bindings round-trip exactly (widened to `double` at the store, narrowed back
 * at the load). */
double zap_repl_slot_get_f64(int64_t i) {
    zap_repl_slot_check(i);
    union {
        int64_t i;
        double d;
    } u;
    u.i = zap_repl_slots[i];
    return u.d;
}
void zap_repl_slot_set_f64(int64_t i, double v) {
    zap_repl_slot_check(i);
    union {
        int64_t i;
        double d;
    } u;
    u.d = v;
    zap_repl_slots[i] = u.i;
}
void *zap_repl_slot_get_ptr(int64_t i) {
    zap_repl_slot_check(i);
    return (void *)zap_repl_slots[i];
}
void zap_repl_slot_set_ptr(int64_t i, void *p) {
    zap_repl_slot_check(i);
    zap_repl_slots[i] = (int64_t)p;
    if (!zap_repl_slot_rooted[i]) {
        zap_repl_slot_rooted[i] = 1;
        /* No-op under Boehm (the static array is scanned conservatively); under
         * precise Immix this registers the stable slot address so the target is
         * marked and forwarded. */
        zap_gc_add_root((void **)&zap_repl_slots[i]);
    }
}

/* One "Uncaught exception: <message>" line (the single-section render). */
static void zap_report_exc_line(void *exc) {
    ZapStr *msg = (ZapStr *)zap_exception_message(exc);
    fputs("Uncaught exception: ", stderr);
    if (msg != NULL && msg->len > 0) {
        fwrite(msg->data, 1, (size_t)msg->len, stderr);
    }
    fputc('\n', stderr);
}

/* Render an exception and its explicit `raise ... from` cause chain (#604,
 * D-0103). The cause's own section prints first (deepest cause at the top),
 * each followed by the Python "direct cause" separator, so an uncaught chained
 * exception reads cause-first like CPython. A plain (causeless) exception is
 * exactly one line, unchanged from before. */
static void zap_report_exc_chain(void *exc, int depth) {
    void *cause = zap_exception_cause(exc);
    /* `depth` caps a pathological `from` cycle (a reused caught object chained
     * back onto itself) so the report cannot recurse without bound. */
    if (cause != NULL && cause != exc && depth > 0) {
        zap_report_exc_chain(cause, depth - 1);
        fputs("The above exception was the direct cause of the following "
              "exception:\n",
              stderr);
    }
    zap_report_exc_line(exc);
}

void zap_uncaught(void *exc) {
    if (zap_repl_catch != NULL) {
        /* REPL mode: hand the message to the loop and unwind to zap_repl_call
         * instead of exiting. The unwinder has already run frame cleanups and
         * the top-level landing pad restored the shadow head, so the longjmp
         * leaves the runtime in a consistent state. */
        zap_repl_exc = exc;
        zap_eh_end_catch();
        longjmp(*zap_repl_catch, 1);
    }
    zap_eh_end_catch();
    zap_report_exc_chain(exc, 128);
    exit(1);
}

/* ---------------------------------------------------------------------------
 * Decorators & reflection (ticket #16, D-0036, design/0004-decorators.md).
 *
 * Two kinds of support live here, both kept out of the mechanical codegen path:
 *   1. helpers for the compile-time `@derive` decorators — `zap_i64_to_str`
 *      (used by generated `repr` methods) and `zap_str_eq` (used by generated
 *      `eq` methods);
 *   2. the runtime-reflection registry — a flat table of (type, name) pairs
 *      that generated bootstrap code fills at the top of `main`
 *      (`zap_reflect_register`) and that `reflect_has` / `reflect_names` query.
 *
 * The registry is a GC-scanned growable array, so the registered ZapStr*
 * objects stay reachable. Reflection is pay-only-when-used: a program that
 * never calls a `reflect_*` entry point never touches this table.
 * ------------------------------------------------------------------------- */

/* Decimal string for an integer (used by generated `@derive(Repr)` code). */
void *zap_i64_to_str(int64_t v) {
    char tmp[24];
    int n = snprintf(tmp, sizeof(tmp), "%lld", (long long)v);
    return zap_str_new(tmp, (int64_t)n);
}

/* Unsigned decimal string (#552). The argument word carries an unsigned value
 * (narrower unsigned types are zero-extended into the i64 word by lowering), so
 * it is reinterpreted as `uint64_t` and printed with `%llu`: no phantom sign,
 * and a value above INT64_MAX (e.g. 2^64-1 = 18446744073709551615) prints its
 * true magnitude rather than a negative number. 20 digits + NUL fits in 24. */
void *zap_u64_to_str(int64_t v) {
    char tmp[24];
    int n = snprintf(tmp, sizeof(tmp), "%llu", (unsigned long long)(uint64_t)v);
    return zap_str_new(tmp, (int64_t)n);
}

/* Render a double the way CPython's `str(float)`/`repr(float)` does (I#374/B1):
 * the **shortest** decimal that round-trips back to the same double, formatted
 * with CPython's fixed-vs-exponent rule — exponent notation when the decimal
 * point would fall at or before position -4 or after position 16, fixed
 * otherwise — and a trailing `.0` on an integer-valued fixed result so a float
 * stays visibly a float (`3.0` -> "3.0"). `inf`/`-inf`/`nan` render as CPython
 * does. Replaces the previous `%g` (6 significant digits), which silently lost
 * precision (`str(1/3)` was "0.333333"). `out` needs room for ~32 bytes;
 * returns the length written.
 *
 * The shortest significant-digit count is found by the standard printf
 * round-trip: the smallest precision whose `%e` rendering parses back to the
 * original double. glibc's correctly-rounded `%e`/`strtod` make this agree with
 * CPython's dtoa for IEEE-754 doubles. */
static int zap_f64_fmt_g(double v, int upper, char *out, size_t outsz) {
    char echar = upper ? 'E' : 'e';
    if (isnan(v)) {
        return snprintf(out, outsz, "%s", upper ? "NAN" : "nan");
    }
    if (isinf(v)) {
        const char *s = (v < 0) ? (upper ? "-INF" : "-inf") : (upper ? "INF" : "inf");
        return snprintf(out, outsz, "%s", s);
    }
    int negative = signbit(v);
    double mag = negative ? -v : v;

    /* Smallest precision (0..16 fractional digits = 1..17 significant) whose
     * %e rendering round-trips. 17 significant digits always suffice. */
    char sci[40];
    int k = 0;
    for (k = 0; k <= 16; k++) {
        snprintf(sci, sizeof(sci), "%.*e", k, mag);
        if (strtod(sci, NULL) == mag) {
            break;
        }
    }
    if (k > 16) {
        k = 16;
        snprintf(sci, sizeof(sci), "%.*e", k, mag);
    }

    /* Parse "d[.ddd]e±XX": significant digits + base-10 exponent of the lead. */
    char digits[20];
    int nd = 0;
    const char *p = sci;
    digits[nd++] = *p++;
    if (*p == '.') {
        p++;
        while (*p >= '0' && *p <= '9') {
            digits[nd++] = *p++;
        }
    }
    int E = 0;
    if (*p == 'e' || *p == 'E') {
        p++;
        E = (int)strtol(p, NULL, 10);
    }
    while (nd > 1 && digits[nd - 1] == '0') {
        nd--; /* defensive: a minimal-k %e has no droppable trailing zero */
    }
    int decpt = E + 1; /* count of significant digits before the decimal point */

    char tmp[48];
    int o = 0;
    if (negative) {
        tmp[o++] = '-';
    }
    if (decpt <= -4 || decpt > 16) {
        /* Exponential: d[.ddd]e±XX, exponent sign always shown, >= 2 digits. */
        tmp[o++] = digits[0];
        if (nd > 1) {
            tmp[o++] = '.';
            for (int i = 1; i < nd; i++) {
                tmp[o++] = digits[i];
            }
        }
        tmp[o++] = echar;
        int e = decpt - 1;
        tmp[o++] = (e < 0) ? '-' : '+';
        int ea = (e < 0) ? -e : e;
        char ebuf[8];
        int en = snprintf(ebuf, sizeof(ebuf), "%02d", ea);
        for (int i = 0; i < en; i++) {
            tmp[o++] = ebuf[i];
        }
    } else if (decpt <= 0) {
        tmp[o++] = '0';
        tmp[o++] = '.';
        for (int i = 0; i < -decpt; i++) {
            tmp[o++] = '0';
        }
        for (int i = 0; i < nd; i++) {
            tmp[o++] = digits[i];
        }
    } else if (decpt >= nd) {
        for (int i = 0; i < nd; i++) {
            tmp[o++] = digits[i];
        }
        for (int i = 0; i < decpt - nd; i++) {
            tmp[o++] = '0';
        }
        tmp[o++] = '.';
        tmp[o++] = '0';
    } else {
        for (int i = 0; i < decpt; i++) {
            tmp[o++] = digits[i];
        }
        tmp[o++] = '.';
        for (int i = decpt; i < nd; i++) {
            tmp[o++] = digits[i];
        }
    }
    tmp[o] = '\0';
    return snprintf(out, outsz, "%s", tmp);
}

/* Decimal string for a double (`str(float)`, print, default f-string interp,
 * JSON number serialization; also power-assert diagnostics, D-0054). */
void *zap_f64_to_str(double v) {
    char tmp[32];
    int n = zap_f64_fmt_g(v, 0, tmp, sizeof(tmp));
    return zap_str_new(tmp, (int64_t)n);
}

/* `"True"` / `"False"` for a boolean (power-assert diagnostics, D-0054). */
void *zap_bool_to_str(int64_t v) {
    return v ? zap_str_new("True", 4) : zap_str_new("False", 5);
}

/* Content equality of two Zap strings (1 = equal), for `@derive(Eq)`. */
int64_t zap_str_eq(void *a, void *b) {
    ZapStr *x = (ZapStr *)a;
    ZapStr *y = (ZapStr *)b;
    if (x == y) {
        return 1;
    }
    if (x == NULL || y == NULL || x->len != y->len) {
        return 0;
    }
    return memcmp(x->data, y->data, (size_t)x->len) == 0 ? 1 : 0;
}

/* ---------------------------------------------------------------------------
 * String surface (ticket #284, design/0007-strings.md).
 *
 * `str` is immutable UTF-8. Indexing, slicing, `len` and iteration are all by
 * **Unicode codepoint** (Python semantics, D-0025) with an ASCII fast path;
 * `char` is a Unicode scalar passed as `i32`. The byte buffer is reachable for
 * byte-level work (SigV4 percent-encoding) via `ord`/iteration on the ASCII
 * fast path and `zap_str_encode` (UTF-8 byte values as a `list[int]`).
 *
 * Case folding and whitespace stripping are ASCII-only in v1 (documented in
 * SPEC/STABILITY); the full Unicode tables are a follow-up.
 * ------------------------------------------------------------------------- */

/* Byte width of the UTF-8 sequence whose lead byte is `c`. An invalid lead is
 * treated as width 1 so a malformed buffer still makes forward progress (the
 * lexer guarantees literals are valid UTF-8; this is defensive). */
static int zap_utf8_width(unsigned char c) {
    if (c < 0x80) {
        return 1;
    }
    if ((c >> 5) == 0x6) {
        return 2;
    }
    if ((c >> 4) == 0xE) {
        return 3;
    }
    if ((c >> 3) == 0x1E) {
        return 4;
    }
    return 1;
}

/* Decode the codepoint of the `w`-byte UTF-8 sequence at `d + off`. */
static int32_t zap_utf8_decode(const unsigned char *d, int64_t off, int w) {
    switch (w) {
    case 2:
        return (int32_t)(((d[off] & 0x1F) << 6) | (d[off + 1] & 0x3F));
    case 3:
        return (int32_t)(((d[off] & 0x0F) << 12) | ((d[off + 1] & 0x3F) << 6) |
                         (d[off + 2] & 0x3F));
    case 4:
        return (int32_t)(((d[off] & 0x07) << 18) | ((d[off + 1] & 0x3F) << 12) |
                         ((d[off + 2] & 0x3F) << 6) | (d[off + 3] & 0x3F));
    default:
        return (int32_t)d[off];
    }
}

/* Encode scalar `cp` into `out` (up to 4 bytes); returns the byte width. */
static int zap_utf8_encode(int32_t cp, char *out) {
    unsigned int c = (unsigned int)cp;
    if (c < 0x80) {
        out[0] = (char)c;
        return 1;
    }
    if (c < 0x800) {
        out[0] = (char)(0xC0 | (c >> 6));
        out[1] = (char)(0x80 | (c & 0x3F));
        return 2;
    }
    if (c < 0x10000) {
        out[0] = (char)(0xE0 | (c >> 12));
        out[1] = (char)(0x80 | ((c >> 6) & 0x3F));
        out[2] = (char)(0x80 | (c & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (c >> 18));
    out[1] = (char)(0x80 | ((c >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((c >> 6) & 0x3F));
    out[3] = (char)(0x80 | (c & 0x3F));
    return 4;
}

/* ascii(x) escaping (#1384): the `repr` output with every non-ASCII codepoint
 * replaced by a Python backslash escape — `\xNN` (cp <= 0xff), `\uNNNN`
 * (cp <= 0xffff), else `\UNNNNNNNN`. ASCII bytes (the quotes/short-escapes repr
 * already produced included) pass through unchanged; a malformed UTF-8 lead byte
 * is escaped as `\xNN` of that raw byte, so the function is total and never
 * raises. */
void *zap_ascii_escape(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    int64_t n = s->len;
    /* worst case: each input byte -> `\UXXXXXXXX` (10 chars). */
    char *out = (char *)zap_buf(n * 10 + 1);
    int64_t o = 0;
    static const char hx[] = "0123456789abcdef";
    int64_t i = 0;
    while (i < n) {
        unsigned char c = d[i];
        if (c < 0x80) {
            out[o++] = (char)c;
            i++;
            continue;
        }
        int w = zap_utf8_width(c);
        if (w <= 1 || i + w > n) {
            /* malformed lead byte: escape the raw byte itself. */
            out[o++] = '\\';
            out[o++] = 'x';
            out[o++] = hx[(c >> 4) & 0xF];
            out[o++] = hx[c & 0xF];
            i++;
            continue;
        }
        int32_t cp = zap_utf8_decode(d, i, w);
        i += w;
        if (cp <= 0xff) {
            out[o++] = '\\';
            out[o++] = 'x';
            out[o++] = hx[(cp >> 4) & 0xF];
            out[o++] = hx[cp & 0xF];
        } else if (cp <= 0xffff) {
            out[o++] = '\\';
            out[o++] = 'u';
            for (int k = 12; k >= 0; k -= 4) out[o++] = hx[(cp >> k) & 0xF];
        } else {
            out[o++] = '\\';
            out[o++] = 'U';
            for (int k = 28; k >= 0; k -= 4) out[o++] = hx[(cp >> k) & 0xF];
        }
    }
    return zap_str_new(out, o);
}

/* Build a `str`/`bytes` object from `len` scratch bytes. With the one-object
 * layout (#837) a string owns its bytes inline, so the scratch buffer is copied
 * in (it was a short-lived GC/stack buffer the caller no longer needs); the name
 * is kept for the ~14 builder call sites that produced bytes into a temporary. */
static void *zap_str_adopt(const char *buf, int64_t len) {
    return zap_str_new(buf, len);
}

static int zap_is_ascii_ws(unsigned char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v';
}

/* len(s): codepoint count (O(1) on the ASCII fast path, O(n) otherwise). */
int64_t zap_str_len_cp(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    int64_t n = 0;
    int64_t i = 0;
    while (i < s->len) {
        i += zap_utf8_width(d[i]);
        n++;
    }
    return n;
}

/* Byte offset of codepoint index `cp_idx` (clamped at the end). */
static int64_t zap_str_byteoff(const ZapStr *s, int64_t cp_idx) {
    const unsigned char *d = (const unsigned char *)s->data;
    int64_t i = 0;
    int64_t k = 0;
    while (i < s->len && k < cp_idx) {
        i += zap_utf8_width(d[i]);
        k++;
    }
    return i;
}

/* s[i] -> char. Negative indices count from the end; out-of-range raises a
 * catchable Zap exception (#383), so a surrounding `try/except` intercepts it
 * (matching CPython's `IndexError`); an uncaught one still terminates via
 * `zap_uncaught`. */
int32_t zap_str_index(void *sp, int64_t idx) {
    const ZapStr *s = (const ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    int64_t count = zap_str_len_cp(sp);
    if (idx < 0) {
        idx += count;
    }
    if (idx < 0 || idx >= count) {
        zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new("string index out of range", 25));
    }
    int64_t off = zap_str_byteoff(s, idx);
    return zap_utf8_decode(d, off, zap_utf8_width(d[off]));
}

/* Codepoint at byte offset `off` (for iteration). */
int32_t zap_str_cp_at_byte(void *sp, int64_t off) {
    const ZapStr *s = (const ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    return zap_utf8_decode(d, off, zap_utf8_width(d[off]));
}

/* Byte offset of the codepoint after the one at `off` (for iteration). */
int64_t zap_str_adv_byte(void *sp, int64_t off) {
    const ZapStr *s = (const ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    return off + zap_utf8_width(d[off]);
}

/* Normalize a positive-step slice bound to [0, n]. */
static int64_t zap_slice_pos(int64_t v, int64_t n) {
    if (v < 0) {
        v += n;
    }
    if (v < 0) {
        v = 0;
    }
    if (v > n) {
        v = n;
    }
    return v;
}

/* Normalize a negative-step slice bound to [-1, n-1]. */
static int64_t zap_slice_neg(int64_t v, int64_t n) {
    if (v < 0) {
        v += n;
    }
    if (v < -1) {
        v = -1;
    }
    if (v > n - 1) {
        v = n - 1;
    }
    return v;
}

/* s[lo:hi:step] -> str, Python slice semantics by codepoint. The `has_*` flags
 * mark which bounds were written (an omitted bound defaults by the step sign).
 * step==1 copies the contiguous byte range; a general step re-encodes the
 * selected codepoints. */
void *zap_str_slice(void *sp, int64_t lo, int64_t hi, int64_t step, int64_t has_lo,
                    int64_t has_hi, int64_t has_step) {
    const ZapStr *s = (const ZapStr *)sp;
    int64_t n = zap_str_len_cp(sp);
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        fprintf(stderr, "zap: slice step cannot be zero\n");
        abort();
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
    if (st == 1) {
        if (start >= stop) {
            return zap_str_new("", 0);
        }
        int64_t a = zap_str_byteoff(s, start);
        int64_t b = zap_str_byteoff(s, stop);
        return zap_str_new(s->data + a, b - a);
    }
    /* General step: collect codepoints and re-encode. */
    char *buf = (char *)zap_buf((size_t)s->len + 4);
    int64_t outlen = 0;
    const unsigned char *d = (const unsigned char *)s->data;
    for (int64_t k = start; (st > 0) ? (k < stop) : (k > stop); k += st) {
        int64_t off = zap_str_byteoff(s, k);
        int w = zap_utf8_width(d[off]);
        int32_t cp = zap_utf8_decode(d, off, w);
        outlen += zap_utf8_encode(cp, buf + outlen);
    }
    return zap_str_adopt(buf, outlen);
}

/* char(n) / chr(n): validate `n` is a Unicode scalar (not a surrogate, in
 * range) and return it as a `char`. Faults on an invalid value (the typed
 * `ValueError` lands with the exception runtime). */
int32_t zap_chr(int64_t n) {
    if (n < 0 || n > 0x10FFFF || (n >= 0xD800 && n <= 0xDFFF)) {
        fprintf(stderr, "zap: chr() argument is not a valid Unicode scalar value\n");
        abort();
    }
    return (int32_t)n;
}

/* str(ch): the one-codepoint string that is `ch`'s UTF-8 encoding. */
void *zap_char_to_str(int32_t cp) {
    char buf[4];
    int w = zap_utf8_encode(cp, buf);
    return zap_str_new(buf, w);
}

/* int(s): parse a decimal integer, Python-style (surrounding ASCII whitespace
 * allowed, optional sign, at least one digit, nothing else). Raises a catchable
 * `ValueError` on a malformed string (Python parity), so `try/except` can
 * recover. Distinct from the lenient `zap_str_to_i64` the CSV codec uses. */
int64_t zap_str_parse_int(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    const char *d = s->data;
    int64_t i = 0;
    int64_t n = s->len;
    while (i < n && zap_is_ascii_ws((unsigned char)d[i])) {
        i++;
    }
    int neg = 0;
    if (i < n && (d[i] == '+' || d[i] == '-')) {
        neg = d[i] == '-';
        i++;
    }
    int64_t start = i;
    unsigned long long acc = 0;
    while (i < n && d[i] >= '0' && d[i] <= '9') {
        acc = acc * 10 + (unsigned long long)(d[i] - '0');
        i++;
    }
    int64_t digits = i - start;
    while (i < n && zap_is_ascii_ws((unsigned char)d[i])) {
        i++;
    }
    if (digits == 0 || i != n) {
        char buf[128];
        int m = snprintf(buf, sizeof buf,
                         "invalid literal for int() with base 10: '%.*s'",
                         (int)s->len, s->data);
        if (m < 0) {
            m = 0;
        }
        if (m >= (int)sizeof buf) {
            m = (int)sizeof buf - 1;
        }
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(buf, (int64_t)m));
    }
    return neg ? -(int64_t)acc : (int64_t)acc;
}

/* float(s): parse a float, Python-style (surrounding ASCII whitespace allowed,
 * a C float literal otherwise). Raises a catchable ValueError on a malformed
 * string, matching int()'s strictness and CPython (epic #374, issue #741).
 * Distinct from the lenient `zap_str_to_f64` the CSV codec uses. */
double zap_str_parse_float(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    char stackbuf[64];
    char *tmp = stackbuf;
    char *heap = NULL;
    if (s->len >= (int64_t)sizeof(stackbuf)) {
        heap = (char *)malloc((size_t)s->len + 1);
        tmp = heap;
    }
    memcpy(tmp, s->data, (size_t)s->len);
    tmp[s->len] = '\0';
    char *p = tmp;
    while (*p && zap_is_ascii_ws((unsigned char)*p)) {
        p++;
    }
    char *end = NULL;
    double v = strtod(p, &end);
    int bad = (end == p);
    if (end != NULL) {
        while (*end && zap_is_ascii_ws((unsigned char)*end)) {
            end++;
        }
        if (*end != '\0') {
            bad = 1;
        }
    }
    if (heap) {
        free(heap);
    }
    if (bad) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
            zap_str_new("could not convert string to float", 33));
    }
    return v;
}

/* ---------------------------------------------------------------------------
 * f-string / `format()` formatting mini-language (ticket #285, D-0026).
 *
 * The compiler parses the spec at compile time and passes it decomposed as
 * integer arguments (width/precision use -1 for "unset"; `align`/`fill` are
 * character codepoints; `sign` is 0='-' only, 1='+', 2=' '). These helpers do
 * the base conversion / `printf` rendering plus fill/align/width padding.
 *
 * Padding width is counted in BYTES (so it is exact for ASCII, the overwhelming
 * case, and matches the v1 byte-length `len`, design/0007-strings.md); string
 * precision truncates on a UTF-8 codepoint boundary. Non-ASCII fill characters
 * fall back to a space. A full Unicode-width formatter is a follow-up.
 * ------------------------------------------------------------------------- */

/* Place `head` (sign/prefix) and `body` (digits/text) into a width-`width`
 * field with `fill`/`align`. `align`: '<' left, '>' right, '^' center, '='
 * pad between head and body (sign-aware). */
static void *zap_fmt_pad(const char *head, int64_t hl, const char *body, int64_t bl,
                         int64_t width, int fill, int align) {
    int64_t content = hl + bl;
    if (width < 0 || content >= width) {
        char *buf = (char *)zap_buf((size_t)content);
        memcpy(buf, head, (size_t)hl);
        memcpy(buf + hl, body, (size_t)bl);
        return zap_str_adopt(buf, content);
    }
    int64_t pad = width - content;
    char fc = (fill > 0 && fill < 128) ? (char)fill : ' ';
    char *buf = (char *)zap_buf((size_t)width);
    int64_t pos = 0;
    if (align == '=') {
        memcpy(buf, head, (size_t)hl);
        pos = hl;
        memset(buf + pos, fc, (size_t)pad);
        pos += pad;
        memcpy(buf + pos, body, (size_t)bl);
    } else if (align == '<') {
        memcpy(buf, head, (size_t)hl);
        pos = hl;
        memcpy(buf + pos, body, (size_t)bl);
        pos += bl;
        memset(buf + pos, fc, (size_t)pad);
    } else if (align == '^') {
        int64_t left = pad / 2;
        int64_t right = pad - left;
        memset(buf, fc, (size_t)left);
        pos = left;
        memcpy(buf + pos, head, (size_t)hl);
        pos += hl;
        memcpy(buf + pos, body, (size_t)bl);
        pos += bl;
        memset(buf + pos, fc, (size_t)right);
    } else { /* '>' or default right */
        memset(buf, fc, (size_t)pad);
        pos = pad;
        memcpy(buf + pos, head, (size_t)hl);
        pos += hl;
        memcpy(buf + pos, body, (size_t)bl);
    }
    return zap_str_adopt(buf, width);
}

void *zap_fmt_int(int64_t v, int64_t base, int64_t upper, int64_t alt, int64_t sign,
                  int64_t width, int64_t fill, int64_t align, int64_t zero) {
    uint64_t mag;
    int neg = 0;
    if (v < 0) {
        neg = 1;
        mag = (uint64_t)(-(v + 1)) + 1ULL; /* avoids UB at INT64_MIN */
    } else {
        mag = (uint64_t)v;
    }
    const char *alpha = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    char rev[72];
    int di = 0;
    if (mag == 0) {
        rev[di++] = '0';
    }
    while (mag > 0) {
        rev[di++] = alpha[mag % (uint64_t)base];
        mag /= (uint64_t)base;
    }
    char body[72];
    for (int k = 0; k < di; k++) {
        body[k] = rev[di - 1 - k];
    }
    char head[8];
    int hl = 0;
    if (neg) {
        head[hl++] = '-';
    } else if (sign == 1) {
        head[hl++] = '+';
    } else if (sign == 2) {
        head[hl++] = ' ';
    }
    if (alt) {
        if (base == 16) {
            head[hl++] = '0';
            head[hl++] = upper ? 'X' : 'x';
        } else if (base == 8) {
            head[hl++] = '0';
            head[hl++] = 'o';
        } else if (base == 2) {
            head[hl++] = '0';
            head[hl++] = 'b';
        }
    }
    int al = (int)align;
    int fc = (int)fill;
    if (zero && al == 0) {
        al = '=';
        fc = '0';
    }
    if (al == 0) {
        al = '>'; /* numbers are right-aligned by default */
    }
    return zap_fmt_pad(head, hl, body, (int64_t)di, width, fc, al);
}

void *zap_fmt_flt(double v, int64_t ftype, int64_t sign, int64_t width, int64_t prec,
                  int64_t fill, int64_t align, int64_t zero) {
    char t = (char)ftype;
    int neg = 0;
    double mag = v;
    if (v < 0.0 || (v == 0.0 && signbit(v))) {
        neg = 1;
        mag = -v;
    }
    /* A wide fixed-point/exponent spec (a large magnitude or a big precision)
     * can exceed any fixed buffer, so size dynamically off snprintf's return
     * value rather than silently truncating the text (#374/B5). The default
     * %g rendering is always short and stays on the stack. */
    char stackbody[128];
    char *body = stackbody;
    char *heapbody = NULL;
    if (prec < 0 && (t == 'g' || t == 'G')) {
        /* Default float rendering: match str(float) — compact %g with a
         * trailing `.0` restored on integral values (I#384). */
        zap_f64_fmt_g(mag, t == 'G', stackbody, sizeof(stackbody));
    } else {
        int p = (prec >= 0) ? (int)prec : 6;
        char spec[12];
        snprintf(spec, sizeof(spec), "%%.%d%c", p, t);
        int need = snprintf(stackbody, sizeof(stackbody), spec, mag);
        if (need >= (int)sizeof(stackbody)) {
            heapbody = (char *)malloc((size_t)need + 1);
            if (heapbody) {
                snprintf(heapbody, (size_t)need + 1, spec, mag);
                body = heapbody;
            }
        }
    }
    int64_t bl = (int64_t)strlen(body);
    char head[2];
    int hl = 0;
    if (neg) {
        head[hl++] = '-';
    } else if (sign == 1) {
        head[hl++] = '+';
    } else if (sign == 2) {
        head[hl++] = ' ';
    }
    int al = (int)align;
    int fc = (int)fill;
    if (zero && al == 0) {
        al = '=';
        fc = '0';
    }
    if (al == 0) {
        al = '>';
    }
    void *result = zap_fmt_pad(head, hl, body, bl, width, fc, al);
    free(heapbody);
    return result;
}

void *zap_fmt_str(void *sp, int64_t width, int64_t prec, int64_t fill, int64_t align) {
    const ZapStr *s = (const ZapStr *)sp;
    int64_t bl = s->len;
    if (prec >= 0) {
        /* Truncate to `prec` codepoints (UTF-8 boundary safe). */
        int64_t cp = 0;
        int64_t i = 0;
        while (i < s->len && cp < prec) {
            unsigned char c = (unsigned char)s->data[i];
            i += (c < 0x80) ? 1 : (c < 0xE0) ? 2 : (c < 0xF0) ? 3 : 4;
            cp++;
        }
        bl = (i < s->len) ? i : s->len;
    }
    int al = (int)align;
    int fc = (int)fill;
    if (al == 0) {
        al = '<'; /* strings are left-aligned by default */
    }
    return zap_fmt_pad("", 0, s->data, bl, width, fc, al);
}

void *zap_str_upper(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    char *buf = (char *)zap_buf((size_t)s->len);
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        buf[i] = (c >= 'a' && c <= 'z') ? (char)(c - 32) : (char)c;
    }
    return zap_str_adopt(buf, s->len);
}

void *zap_str_lower(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    char *buf = (char *)zap_buf((size_t)s->len);
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + 32) : (char)c;
    }
    return zap_str_adopt(buf, s->len);
}

/* strip(mode): 0 both ends, 1 left only, 2 right only. ASCII whitespace. */
void *zap_str_strip(void *sp, int64_t mode) {
    const ZapStr *s = (const ZapStr *)sp;
    int64_t a = 0;
    int64_t b = s->len;
    if (mode != 2) {
        while (a < b && zap_is_ascii_ws((unsigned char)s->data[a])) {
            a++;
        }
    }
    if (mode != 1) {
        while (b > a && zap_is_ascii_ws((unsigned char)s->data[b - 1])) {
            b--;
        }
    }
    return zap_str_new(s->data + a, b - a);
}

int64_t zap_str_startswith(void *sp, void *pp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *p = (const ZapStr *)pp;
    if (p->len > s->len) {
        return 0;
    }
    return memcmp(s->data, p->data, (size_t)p->len) == 0 ? 1 : 0;
}

int64_t zap_str_endswith(void *sp, void *pp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *p = (const ZapStr *)pp;
    if (p->len > s->len) {
        return 0;
    }
    return memcmp(s->data + (s->len - p->len), p->data, (size_t)p->len) == 0 ? 1 : 0;
}

/* SIMD substring search (#945, epic #139, D-0194), defined in zapmemchr.inc.c
 * (#included near the end of this file, after the cpu_features detector it
 * dispatches on). Forward-declared here so the string paths can route through it. */
ptrdiff_t zap_memmem_off(const void *hay, size_t hn, const void *ndl, size_t nn);

/* First byte offset of `sub` in `s`, or -1. Both are valid UTF-8, so a byte
 * match is codepoint-aligned. Routed through the multiversioned SIMD memmem
 * (#945): scan for the needle's first byte with a dispatched SIMD memchr, verify
 * the rest with memcmp — far faster than the former O(n*m) memcmp-at-every-offset
 * scan, and identical in result (first offset, or -1; empty needle -> 0). */
static int64_t zap_str_find_bytes(const ZapStr *s, const ZapStr *sub) {
    return (int64_t)zap_memmem_off(s->data, (size_t)s->len, sub->data, (size_t)sub->len);
}

int64_t zap_str_contains(void *sp, void *subp) {
    return zap_str_find_bytes((const ZapStr *)sp, (const ZapStr *)subp) >= 0 ? 1 : 0;
}

/* ====================================================================== */
/* Borrowed-bytes view ABI (#1082, borrowed-bytes RFC, discussion #1097).  */
/*                                                                          */
/* A `strview` is a non-owning view `(backing, byte_off, byte_len)` into an */
/* existing `ZapStr`'s bytes. The front-end lowers it to a by-value tuple   */
/* whose `backing` field is a traced GC pointer, so a moving-Immix          */
/* evacuation rewrites it in place (the "2b" base+offset representation,    */
/* via the aggregate-root pairs, #586) — these entry points therefore read  */
/* `backing->data` fresh on every call and NEVER cache a raw data pointer   */
/* across anything that could be a safepoint. They allocate nothing and     */
/* copy nothing (except `zap_strview_to_str`, the explicit materializer).   */
/*                                                                          */
/* All indices on the surface are Unicode codepoints (D-0025, Python        */
/* clamping incl. negatives); the stored `byte_off`/`byte_len` are BYTES.   */

/* Byte offset within the window `[data, data+bytelen)` of codepoint index
 * `cp`, Python slice-bound semantics (negatives from the end; clamp to the
 * valid `[0, cp_count]` range). O(bytelen) UTF-8 walk. */
static int64_t zap_sv_cp_byte(const char *data, int64_t bytelen, int64_t cp) {
    const unsigned char *d = (const unsigned char *)data;
    int64_t cplen = 0;
    for (int64_t i = 0; i < bytelen;) {
        i += zap_utf8_width(d[i]);
        cplen++;
    }
    if (cp < 0) {
        cp += cplen;
    }
    if (cp < 0) {
        cp = 0;
    }
    if (cp > cplen) {
        cp = cplen;
    }
    int64_t i = 0, k = 0;
    while (k < cp && i < bytelen) {
        i += zap_utf8_width(d[i]);
        k++;
    }
    return i;
}

/* Creation: byte offset of codepoint `lo` within the whole backing (0 when
 * `has_lo` is false → a full view's start). */
int64_t zap_strview_off(void *bp, int64_t lo, int64_t has_lo) {
    const ZapStr *s = (const ZapStr *)bp;
    return has_lo ? zap_sv_cp_byte(s->data, s->len, lo) : 0;
}

/* Creation: byte length of the view from byte offset `off` to codepoint `hi`
 * within the whole backing (to the end when `has_hi` is false). Never
 * negative (an inverted range yields an empty view). */
int64_t zap_strview_len(void *bp, int64_t off, int64_t hi, int64_t has_hi) {
    const ZapStr *s = (const ZapStr *)bp;
    int64_t end = has_hi ? zap_sv_cp_byte(s->data, s->len, hi) : s->len;
    return end > off ? end - off : 0;
}

/* Sub-view (`v.view(lo, hi)`): byte offset within the BACKING of codepoint
 * `lo` measured inside the parent view window `[voff, voff+vlen)`. */
int64_t zap_strview_sub_off(void *bp, int64_t voff, int64_t vlen, int64_t lo, int64_t has_lo) {
    const ZapStr *s = (const ZapStr *)bp;
    return voff + (has_lo ? zap_sv_cp_byte(s->data + voff, vlen, lo) : 0);
}

/* Sub-view: byte length from the sub-view's byte offset `noff` to codepoint
 * `hi` inside the parent window (to the parent end when `has_hi` is false). */
int64_t zap_strview_sub_len(void *bp, int64_t voff, int64_t vlen, int64_t noff, int64_t hi,
                            int64_t has_hi) {
    const ZapStr *s = (const ZapStr *)bp;
    int64_t end = has_hi ? voff + zap_sv_cp_byte(s->data + voff, vlen, hi) : voff + vlen;
    return end > noff ? end - noff : 0;
}

/* startswith/endswith/contains, comparing two borrowed byte windows. Both
 * windows are valid UTF-8, so a byte match is codepoint-aligned. */
int64_t zap_strview_startswith(void *bp, int64_t off, int64_t len, void *ap, int64_t aoff,
                               int64_t alen) {
    if (alen > len) {
        return 0;
    }
    const char *h = ((const ZapStr *)bp)->data + off;
    const char *n = ((const ZapStr *)ap)->data + aoff;
    return memcmp(h, n, (size_t)alen) == 0 ? 1 : 0;
}

int64_t zap_strview_endswith(void *bp, int64_t off, int64_t len, void *ap, int64_t aoff,
                             int64_t alen) {
    if (alen > len) {
        return 0;
    }
    const char *h = ((const ZapStr *)bp)->data + off + (len - alen);
    const char *n = ((const ZapStr *)ap)->data + aoff;
    return memcmp(h, n, (size_t)alen) == 0 ? 1 : 0;
}

int64_t zap_strview_contains(void *bp, int64_t off, int64_t len, void *ap, int64_t aoff,
                             int64_t alen) {
    const char *h = ((const ZapStr *)bp)->data + off;
    const char *n = ((const ZapStr *)ap)->data + aoff;
    return zap_memmem_off(h, (size_t)len, n, (size_t)alen) >= 0 ? 1 : 0;
}

/* Codepoint count of the borrowed range (Python `len`). */
int64_t zap_strview_cp_count(void *bp, int64_t off, int64_t len) {
    const unsigned char *d = (const unsigned char *)((const ZapStr *)bp)->data + off;
    int64_t k = 0;
    for (int64_t i = 0; i < len;) {
        i += zap_utf8_width(d[i]);
        k++;
    }
    return k;
}

/* Materialize an owned `str` copy of the borrowed range — the one allocating
 * entry point, the explicit escape hatch out of the borrow. */
void *zap_strview_to_str(void *bp, int64_t off, int64_t len) {
    return zap_str_new(((const ZapStr *)bp)->data + off, len);
}

/* Hash the borrowed bytes with the same seeded wyhash the owned `str` path and
 * the dict use, so a view hashes identically to its materialized `str` (#1064).
 * Used for dict/set probing and `v.__hash__()`. */
int64_t zap_strview_hash(void *bp, int64_t off, int64_t len) {
    return (int64_t)zap_wyhash(((const ZapStr *)bp)->data + off, (size_t)len, zap_hash_seed);
}

/* Equality of a borrowed view with an owned `str` (the `strview == str`
 * operator path): same length and bytes. */
int64_t zap_strview_eq_str(void *bp, int64_t off, int64_t len, void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    if (s->len != len) {
        return 0;
    }
    return memcmp(((const ZapStr *)bp)->data + off, s->data, (size_t)len) == 0 ? 1 : 0;
}

/* Equality of two borrowed views. */
int64_t zap_strview_eq(void *bp, int64_t off, int64_t len, void *ap, int64_t aoff, int64_t alen) {
    if (len != alen) {
        return 0;
    }
    const char *x = ((const ZapStr *)bp)->data + off;
    const char *y = ((const ZapStr *)ap)->data + aoff;
    return memcmp(x, y, (size_t)len) == 0 ? 1 : 0;
}

/* ---- bytesview creation (#1082 follow-up) -------------------------------- */
/* The `bytes` counterpart of the strview creation helpers. `bytes` is BYTE-
 * indexed (I#337), so these do no UTF-8 walk — a bounds index is clamped against
 * the backing's byte length with Python slice semantics (negatives from the end).
 * The read-only probes / hash / eq / materializer are shared with strview (both
 * are `(base, off, len)` over the same `ZapStr` storage), so only these four
 * byte-indexed entry points are bytes-specific. */

/* Clamp a (possibly negative) byte index into `[0, bytelen]`, Python-style. */
static int64_t zap_bv_clamp(int64_t idx, int64_t bytelen) {
    if (idx < 0) {
        idx += bytelen;
    }
    if (idx < 0) {
        return 0;
    }
    return idx > bytelen ? bytelen : idx;
}

int64_t zap_bytesview_off(void *bp, int64_t lo, int64_t has_lo) {
    const ZapStr *s = (const ZapStr *)bp;
    return has_lo ? zap_bv_clamp(lo, s->len) : 0;
}

int64_t zap_bytesview_len(void *bp, int64_t off, int64_t hi, int64_t has_hi) {
    const ZapStr *s = (const ZapStr *)bp;
    int64_t end = has_hi ? zap_bv_clamp(hi, s->len) : s->len;
    return end > off ? end - off : 0;
}

/* Sub-view within the parent byte window [voff, voff+vlen). */
int64_t zap_bytesview_sub_off(void *bp, int64_t voff, int64_t vlen, int64_t lo, int64_t has_lo) {
    (void)bp;
    return voff + (has_lo ? zap_bv_clamp(lo, vlen) : 0);
}

int64_t zap_bytesview_sub_len(void *bp, int64_t voff, int64_t vlen, int64_t noff, int64_t hi,
                              int64_t has_hi) {
    (void)bp;
    int64_t end = has_hi ? voff + zap_bv_clamp(hi, vlen) : voff + vlen;
    return end > noff ? end - noff : 0;
}

/* find(sub): codepoint index of the first match, or -1. */
int64_t zap_str_find(void *sp, void *subp) {
    const ZapStr *s = (const ZapStr *)sp;
    int64_t off = zap_str_find_bytes(s, (const ZapStr *)subp);
    if (off < 0) {
        return -1;
    }
    int64_t k = 0;
    int64_t j = 0;
    const unsigned char *d = (const unsigned char *)s->data;
    while (j < off) {
        j += zap_utf8_width(d[j]);
        k++;
    }
    return k;
}

/* index(sub): like find, but faults when `sub` is absent (Python's ValueError;
 * the typed exception lands with the exception runtime). */
int64_t zap_str_index_of(void *sp, void *subp) {
    int64_t r = zap_str_find(sp, subp);
    if (r < 0) {
        fprintf(stderr, "zap: substring not found\n");
        abort();
    }
    return r;
}

/* replace(old, new): replace every non-overlapping occurrence. An empty `old`
 * returns the string unchanged (Python inserts `new` between codepoints; left
 * as a follow-up). */
void *zap_str_replace(void *sp, void *oldp, void *newp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *o = (const ZapStr *)oldp;
    const ZapStr *nw = (const ZapStr *)newp;
    if (o->len == 0) {
        return zap_str_new(s->data, s->len);
    }
    int64_t count = 0;
    for (int64_t i = 0; i + o->len <= s->len;) {
        if (memcmp(s->data + i, o->data, (size_t)o->len) == 0) {
            count++;
            i += o->len;
        } else {
            i++;
        }
    }
    int64_t outlen = s->len + count * (nw->len - o->len);
    char *buf = (char *)zap_buf((size_t)(outlen > 0 ? outlen : 1));
    int64_t w = 0;
    for (int64_t i = 0; i < s->len;) {
        if (i + o->len <= s->len && memcmp(s->data + i, o->data, (size_t)o->len) == 0) {
            memcpy(buf + w, nw->data, (size_t)nw->len);
            w += nw->len;
            i += o->len;
        } else {
            buf[w++] = s->data[i++];
        }
    }
    return zap_str_adopt(buf, outlen);
}

/* split(sep) -> list[str] on a non-empty separator. An empty separator yields
 * a one-element list with the whole string (Python raises ValueError; left as
 * a follow-up). */
void *zap_str_split(void *sp, void *sepp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *sep = (const ZapStr *)sepp;
    void *out = zap_list_new();
    if (sep->len == 0) {
        zap_list_push_ptr(out, zap_str_new(s->data, s->len));
        return out;
    }
    int64_t start = 0;
    for (int64_t i = 0; i + sep->len <= s->len;) {
        if (memcmp(s->data + i, sep->data, (size_t)sep->len) == 0) {
            zap_list_push_ptr(out, zap_str_new(s->data + start, i - start));
            i += sep->len;
            start = i;
        } else {
            i++;
        }
    }
    zap_list_push_ptr(out, zap_str_new(s->data + start, s->len - start));
    return out;
}

/* sep.join(parts): concatenate `parts` (a list[str]) with `sep` between them. */
void *zap_str_join(void *sepp, void *listp) {
    const ZapStr *sep = (const ZapStr *)sepp;
    int64_t n = zap_list_len(listp);
    if (n <= 0) {
        return zap_str_new("", 0);
    }
    int64_t total = 0;
    for (int64_t i = 0; i < n; i++) {
        const ZapStr *e = (const ZapStr *)zap_list_get_ptr(listp, i);
        total += e->len;
        if (i > 0) {
            total += sep->len;
        }
    }
    char *buf = (char *)zap_buf((size_t)(total > 0 ? total : 1));
    int64_t w = 0;
    for (int64_t i = 0; i < n; i++) {
        if (i > 0) {
            memcpy(buf + w, sep->data, (size_t)sep->len);
            w += sep->len;
        }
        const ZapStr *e = (const ZapStr *)zap_list_get_ptr(listp, i);
        memcpy(buf + w, e->data, (size_t)e->len);
        w += e->len;
    }
    return zap_str_adopt(buf, total);
}

/* encode() -> list[int]: the UTF-8 byte values of `s` (0..255). The reachable
 * byte-level view (SigV4 percent-encoding) until the full `bytes` type lands. */
void *zap_str_encode(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    void *out = zap_list_new();
    for (int64_t i = 0; i < s->len; i++) {
        zap_list_push_i64(out, (int64_t)(unsigned char)s->data[i]);
    }
    return out;
}


/* ===========================================================================
 * statistics — descriptive statistics over scalar lists (#980, D-0243, epic
 * #966). The ergonomic scalar-list surface Python's `statistics` provides;
 * built on plain `ZapList` iteration + libm (`sqrt`/`exp`/`log`), NOT on the
 * numeric-array reductions (#505).
 *
 * Element surface (D-0243): every function takes a `list[float64]` OR a
 * `list[int64]`. An `is_float` flag (1 = words are f64 bits, 0 = int64 words)
 * selects how each element word is read as a `double`; results are always
 * `float64` (`multimode`/`quantiles` return `list[float64]`). Empty / invalid
 * input raises `ValueError` (Python's `StatisticsError` is a `ValueError`
 * subclass; Zap's exception model is flat, so `ValueError` is the equivalent).
 * ======================================================================== */

/* Raise the StatisticsError-equivalent (ValueError) with a fixed message. */
static void zap_stats_err(const char *m) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}

/* Read the i-th element word of `l` as a double (int64 word, or f64 bits). */
static double zap_stats_at(ZapList *l, int64_t i, int64_t is_float) {
    return is_float ? zap_f64_from_bits(l->data[i]) : (double)l->data[i];
}

/* Materialize the list into a freshly malloc'd `double[len]` (caller frees).
 * An empty list yields a 1-slot buffer so the pointer is never NULL. OOM
 * raises a catchable RuntimeError. */
static double *zap_stats_copy(ZapList *l, int64_t is_float, int64_t *n_out) {
    int64_t n = l->len;
    size_t slots = (size_t)(n > 0 ? n : 1);
    double *buf = (double *)malloc(slots * sizeof(double));
    if (buf == NULL) {
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                        zap_str_new("statistics: out of memory", 26));
    }
    for (int64_t i = 0; i < n; i++) {
        buf[i] = zap_stats_at(l, i, is_float);
    }
    *n_out = n;
    return buf;
}

/* Neumaier (improved Kahan) compensated summation — accurate for the internal
 * mean / sum-of-squares without the full fsum partial-set machinery. */
static double zap_stats_neumaier(const double *a, int64_t n) {
    double sum = 0.0;
    double comp = 0.0; /* running compensation for lost low-order bits */
    for (int64_t i = 0; i < n; i++) {
        double x = a[i];
        double t = sum + x;
        if (fabs(sum) >= fabs(x)) {
            comp += (sum - t) + x;
        } else {
            comp += (x - t) + sum;
        }
        sum = t;
    }
    return sum + comp;
}

/* Shewchuk exact summation over a double buffer (same algorithm as
 * `zap_math_fsum`): the result is the correctly-rounded sum. Used by `fmean`
 * so it is exact where the plain `mean` (Neumaier) may differ by a ULP. */
static double zap_stats_fsum(const double *a, int64_t n) {
    double stackbuf[64];
    double *partials = stackbuf;
    int cap = 64;
    int np = 0;
    int on_heap = 0;
    for (int64_t idx = 0; idx < n; idx++) {
        double x = a[idx];
        int i = 0;
        for (int j = 0; j < np; j++) {
            double y = partials[j];
            if (fabs(x) < fabs(y)) {
                double t = x;
                x = y;
                y = t;
            }
            double hi = x + y;
            double lo = y - (hi - x);
            if (lo != 0.0) {
                partials[i++] = lo;
            }
            x = hi;
        }
        np = i;
        if (np + 1 > cap) {
            int ncap = cap * 2;
            double *nb = (double *)malloc((size_t)ncap * sizeof(double));
            if (nb == NULL) {
                if (on_heap) {
                    free(partials);
                }
                zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                                zap_str_new("statistics: out of memory", 26));
            }
            memcpy(nb, partials, (size_t)np * sizeof(double));
            if (on_heap) {
                free(partials);
            }
            partials = nb;
            cap = ncap;
            on_heap = 1;
        }
        partials[np++] = x;
    }
    double total = 0.0;
    for (int j = 0; j < np; j++) {
        total += partials[j];
    }
    if (on_heap) {
        free(partials);
    }
    return total;
}

/* qsort comparator over doubles (ascending). NaN is ordered as equal to
 * everything, matching the "garbage in, garbage out" contract Python keeps for
 * NaN-containing data. */
static int zap_stats_dcmp(const void *pa, const void *pb) {
    double a = *(const double *)pa;
    double b = *(const double *)pb;
    if (a < b) {
        return -1;
    }
    if (a > b) {
        return 1;
    }
    return 0;
}

/* --- means ---------------------------------------------------------------- */

/* mean(xs): arithmetic mean via Neumaier compensated summation. Empty raises. */
double zap_stats_mean(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.mean(): no data points");
    }
    int64_t n;
    double *buf = zap_stats_copy(l, is_float, &n);
    double s = zap_stats_neumaier(buf, n);
    free(buf);
    return s / (double)n;
}

/* fmean(xs): float mean via Shewchuk-exact summation (exactly-rounded sum). */
double zap_stats_fmean(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.fmean(): no data points");
    }
    int64_t n;
    double *buf = zap_stats_copy(l, is_float, &n);
    double s = zap_stats_fsum(buf, n);
    free(buf);
    return s / (double)n;
}

/* geometric_mean(xs): exp(mean(log(x))). Requires every value > 0 (a value
 * <= 0 raises, as in Python). Empty raises. */
double zap_stats_geometric_mean(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.geometric_mean(): no data points");
    }
    int64_t n = l->len;
    double acc = 0.0; /* sum of logs (Neumaier inline) */
    double comp = 0.0;
    for (int64_t i = 0; i < n; i++) {
        double x = zap_stats_at(l, i, is_float);
        if (!(x > 0.0)) {
            zap_stats_err("statistics.geometric_mean(): data must be all positive");
        }
        double lx = log(x);
        double t = acc + lx;
        if (fabs(acc) >= fabs(lx)) {
            comp += (acc - t) + lx;
        } else {
            comp += (lx - t) + acc;
        }
        acc = t;
    }
    return exp((acc + comp) / (double)n);
}

/* harmonic_mean(xs): n / sum(1/x). Requires every value > 0 (D-0243: stricter
 * than Python, which maps a zero to a 0.0 result — here a non-positive value
 * raises, matching geometric_mean). Empty raises. */
double zap_stats_harmonic_mean(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.harmonic_mean(): no data points");
    }
    int64_t n = l->len;
    double acc = 0.0; /* sum of reciprocals (Neumaier inline) */
    double comp = 0.0;
    for (int64_t i = 0; i < n; i++) {
        double x = zap_stats_at(l, i, is_float);
        if (!(x > 0.0)) {
            zap_stats_err("statistics.harmonic_mean(): data must be all positive");
        }
        double r = 1.0 / x;
        double t = acc + r;
        if (fabs(acc) >= fabs(r)) {
            comp += (acc - t) + r;
        } else {
            comp += (r - t) + acc;
        }
        acc = t;
    }
    return (double)n / (acc + comp);
}

/* --- order statistics ----------------------------------------------------- */

/* median(xs): middle value (odd n), or the mean of the two middles (even n). */
double zap_stats_median(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.median(): no data points");
    }
    int64_t n;
    double *buf = zap_stats_copy(l, is_float, &n);
    qsort(buf, (size_t)n, sizeof(double), zap_stats_dcmp);
    double r;
    if (n % 2 == 1) {
        r = buf[n / 2];
    } else {
        r = (buf[n / 2 - 1] + buf[n / 2]) / 2.0;
    }
    free(buf);
    return r;
}

/* median_low(xs): the lower of the two middles for even n (else the middle). */
double zap_stats_median_low(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.median_low(): no data points");
    }
    int64_t n;
    double *buf = zap_stats_copy(l, is_float, &n);
    qsort(buf, (size_t)n, sizeof(double), zap_stats_dcmp);
    double r = (n % 2 == 1) ? buf[n / 2] : buf[n / 2 - 1];
    free(buf);
    return r;
}

/* median_high(xs): the higher of the two middles for even n (else the middle). */
double zap_stats_median_high(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.median_high(): no data points");
    }
    int64_t n;
    double *buf = zap_stats_copy(l, is_float, &n);
    qsort(buf, (size_t)n, sizeof(double), zap_stats_dcmp);
    double r = buf[n / 2];
    free(buf);
    return r;
}

/* mode(xs): the first value with the maximal count, in first-appearance order
 * (Python's `mode` returns the first mode encountered). Empty raises. O(n^2)
 * over distinct values — fine for the scalar-list use the module targets. */
double zap_stats_mode(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    int64_t n = l->len;
    if (n < 1) {
        zap_stats_err("statistics.mode(): no data points");
    }
    double best_val = zap_stats_at(l, 0, is_float);
    int64_t best_count = 0;
    for (int64_t i = 0; i < n; i++) {
        double vi = zap_stats_at(l, i, is_float);
        int is_first = 1;
        for (int64_t k = 0; k < i; k++) {
            if (zap_stats_at(l, k, is_float) == vi) {
                is_first = 0;
                break;
            }
        }
        if (!is_first) {
            continue; /* count each distinct value once, at its first index */
        }
        int64_t count = 0;
        for (int64_t k = 0; k < n; k++) {
            if (zap_stats_at(l, k, is_float) == vi) {
                count++;
            }
        }
        if (count > best_count) { /* strict: ties keep the earlier-seen value */
            best_count = count;
            best_val = vi;
        }
    }
    return best_val;
}

/* multimode(xs): every value tied for the maximal count, in first-appearance
 * order. Empty input yields an empty list (Python parity; does NOT raise). */
void *zap_stats_multimode(void *lp, int64_t is_float) {
    ZapList *l = (ZapList *)lp;
    int64_t n = l->len;
    void *out = zap_list_new();
    if (n < 1) {
        return out;
    }
    /* first pass: the maximal count over distinct values */
    int64_t best_count = 0;
    for (int64_t i = 0; i < n; i++) {
        double vi = zap_stats_at(l, i, is_float);
        int is_first = 1;
        for (int64_t k = 0; k < i; k++) {
            if (zap_stats_at(l, k, is_float) == vi) {
                is_first = 0;
                break;
            }
        }
        if (!is_first) {
            continue;
        }
        int64_t count = 0;
        for (int64_t k = 0; k < n; k++) {
            if (zap_stats_at(l, k, is_float) == vi) {
                count++;
            }
        }
        if (count > best_count) {
            best_count = count;
        }
    }
    /* second pass: collect first-occurrence values whose count == best_count */
    for (int64_t i = 0; i < n; i++) {
        double vi = zap_stats_at(l, i, is_float);
        int is_first = 1;
        for (int64_t k = 0; k < i; k++) {
            if (zap_stats_at(l, k, is_float) == vi) {
                is_first = 0;
                break;
            }
        }
        if (!is_first) {
            continue;
        }
        int64_t count = 0;
        for (int64_t k = 0; k < n; k++) {
            if (zap_stats_at(l, k, is_float) == vi) {
                count++;
            }
        }
        if (count == best_count) {
            zap_list_push_f64(out, vi);
        }
    }
    return out;
}

/* --- spread --------------------------------------------------------------- */

/* Corrected sum of squared deviations from `c` (the mean when `has_c` is 0).
 * The `- sd*sd/n` term removes the rounding bias in the deviations, matching
 * CPython's `_ss`. Fills `*n_out` with the element count. */
static double zap_stats_ss(ZapList *l, int64_t is_float, int64_t has_c, double c,
                           int64_t *n_out) {
    int64_t n;
    double *buf = zap_stats_copy(l, is_float, &n);
    double center = has_c ? c : (zap_stats_neumaier(buf, n) / (double)n);
    double *dev = (double *)malloc((size_t)(n > 0 ? n : 1) * sizeof(double));
    double *sq = (double *)malloc((size_t)(n > 0 ? n : 1) * sizeof(double));
    if (dev == NULL || sq == NULL) {
        free(buf);
        free(dev);
        free(sq);
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                        zap_str_new("statistics: out of memory", 26));
    }
    for (int64_t i = 0; i < n; i++) {
        double d = buf[i] - center;
        dev[i] = d;
        sq[i] = d * d;
    }
    double sxx = zap_stats_neumaier(sq, n);
    double sd = zap_stats_neumaier(dev, n);
    sxx -= sd * sd / (double)n;
    if (sxx < 0.0) {
        sxx = 0.0; /* a tiny negative from rounding is clamped to zero */
    }
    free(buf);
    free(dev);
    free(sq);
    *n_out = n;
    return sxx;
}

/* variance(xs, xbar?): sample variance (÷ n-1). Needs >= 2 points. */
double zap_stats_variance(void *lp, int64_t is_float, int64_t has_xbar, double xbar) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 2) {
        zap_stats_err("statistics.variance(): at least two data points required");
    }
    int64_t n;
    double sxx = zap_stats_ss(l, is_float, has_xbar, xbar, &n);
    return sxx / (double)(n - 1);
}

/* pvariance(xs, mu?): population variance (÷ n). Needs >= 1 point. */
double zap_stats_pvariance(void *lp, int64_t is_float, int64_t has_mu, double mu) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.pvariance(): no data points");
    }
    int64_t n;
    double sxx = zap_stats_ss(l, is_float, has_mu, mu, &n);
    return sxx / (double)n;
}

/* stdev(xs, xbar?): sample standard deviation. Needs >= 2 points. */
double zap_stats_stdev(void *lp, int64_t is_float, int64_t has_xbar, double xbar) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 2) {
        zap_stats_err("statistics.stdev(): at least two data points required");
    }
    int64_t n;
    double sxx = zap_stats_ss(l, is_float, has_xbar, xbar, &n);
    return sqrt(sxx / (double)(n - 1));
}

/* pstdev(xs, mu?): population standard deviation. Needs >= 1 point. */
double zap_stats_pstdev(void *lp, int64_t is_float, int64_t has_mu, double mu) {
    ZapList *l = (ZapList *)lp;
    if (l->len < 1) {
        zap_stats_err("statistics.pstdev(): no data points");
    }
    int64_t n;
    double sxx = zap_stats_ss(l, is_float, has_mu, mu, &n);
    return sqrt(sxx / (double)n);
}

/* --- quantiles ------------------------------------------------------------ */

/* quantiles(xs, n, method): the n-1 cut points dividing the sorted data into n
 * equal groups. `method` is "exclusive" (default) or "inclusive" (CPython's two
 * methods). Needs >= 2 points and n >= 1; an unknown method raises. Returns a
 * `list[float64]` of length n-1 (empty when n == 1). */
void *zap_stats_quantiles(void *lp, int64_t is_float, int64_t nq, void *method_sp) {
    ZapList *l = (ZapList *)lp;
    int64_t ld = l->len;
    if (ld < 2) {
        zap_stats_err("statistics.quantiles(): must have at least two data points");
    }
    if (nq < 1) {
        zap_stats_err("statistics.quantiles(): n must be at least 1");
    }
    ZapStr *ms = (ZapStr *)method_sp;
    int inclusive;
    if (ms->len == 9 && memcmp(ms->data, "exclusive", 9) == 0) {
        inclusive = 0;
    } else if (ms->len == 9 && memcmp(ms->data, "inclusive", 9) == 0) {
        inclusive = 1;
    } else {
        zap_stats_err("statistics.quantiles(): method must be 'exclusive' or 'inclusive'");
        return NULL; /* unreachable */
    }
    int64_t n;
    double *data = zap_stats_copy(l, is_float, &n);
    qsort(data, (size_t)n, sizeof(double), zap_stats_dcmp);
    void *out = zap_list_new();
    if (inclusive) {
        int64_t m = ld - 1;
        for (int64_t i = 1; i < nq; i++) {
            int64_t j = (i * m) / nq;
            int64_t delta = (i * m) - j * nq;
            double interp =
                (data[j] * (double)(nq - delta) + data[j + 1] * (double)delta) / (double)nq;
            zap_list_push_f64(out, interp);
        }
    } else {
        int64_t m = ld + 1;
        for (int64_t i = 1; i < nq; i++) {
            int64_t j = (i * m) / nq;
            if (j < 1) {
                j = 1;
            } else if (j > ld - 1) {
                j = ld - 1;
            }
            int64_t delta = (i * m) - j * nq;
            double interp =
                (data[j - 1] * (double)(nq - delta) + data[j] * (double)delta) / (double)nq;
            zap_list_push_f64(out, interp);
        }
    }
    free(data);
    return out;
}

/* --- relationships (two lists) -------------------------------------------- */

/* Shared setup for covariance/correlation/linear_regression: validate equal,
 * >= 2 lengths and materialize both lists. Caller frees *bx and *by. */
static int64_t zap_stats_pair(ZapList *x, int64_t fx, ZapList *y, int64_t fy,
                              const char *who, double **bx, double **by) {
    if (x->len != y->len) {
        if (strcmp(who, "covariance") == 0) {
            zap_stats_err("statistics.covariance(): both inputs must be the same length");
        } else if (strcmp(who, "correlation") == 0) {
            zap_stats_err("statistics.correlation(): both inputs must be the same length");
        } else {
            zap_stats_err("statistics.linear_regression(): both inputs must be the same length");
        }
    }
    if (x->len < 2) {
        if (strcmp(who, "covariance") == 0) {
            zap_stats_err("statistics.covariance(): at least two data points required");
        } else if (strcmp(who, "correlation") == 0) {
            zap_stats_err("statistics.correlation(): at least two data points required");
        } else {
            zap_stats_err("statistics.linear_regression(): at least two data points required");
        }
    }
    int64_t n;
    *bx = zap_stats_copy(x, fx, &n);
    int64_t n2;
    *by = zap_stats_copy(y, fy, &n2);
    return n;
}

/* covariance(xs, ys): sample covariance, sum((xi-xbar)(yi-ybar)) / (n-1). */
double zap_stats_covariance(void *xp, int64_t fx, void *yp, int64_t fy) {
    ZapList *x = (ZapList *)xp;
    ZapList *y = (ZapList *)yp;
    double *bx;
    double *by;
    int64_t n = zap_stats_pair(x, fx, y, fy, "covariance", &bx, &by);
    double xbar = zap_stats_neumaier(bx, n) / (double)n;
    double ybar = zap_stats_neumaier(by, n) / (double)n;
    double *prod = (double *)malloc((size_t)n * sizeof(double));
    if (prod == NULL) {
        free(bx);
        free(by);
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                        zap_str_new("statistics: out of memory", 26));
    }
    for (int64_t i = 0; i < n; i++) {
        prod[i] = (bx[i] - xbar) * (by[i] - ybar);
    }
    double sxy = zap_stats_neumaier(prod, n);
    free(bx);
    free(by);
    free(prod);
    return sxy / (double)(n - 1);
}

/* correlation(xs, ys): Pearson's correlation coefficient in [-1, 1]. A constant
 * input (zero variance) raises, as in Python. */
double zap_stats_correlation(void *xp, int64_t fx, void *yp, int64_t fy) {
    ZapList *x = (ZapList *)xp;
    ZapList *y = (ZapList *)yp;
    double *bx;
    double *by;
    int64_t n = zap_stats_pair(x, fx, y, fy, "correlation", &bx, &by);
    double xbar = zap_stats_neumaier(bx, n) / (double)n;
    double ybar = zap_stats_neumaier(by, n) / (double)n;
    double *pxy = (double *)malloc((size_t)n * sizeof(double));
    double *pxx = (double *)malloc((size_t)n * sizeof(double));
    double *pyy = (double *)malloc((size_t)n * sizeof(double));
    if (pxy == NULL || pxx == NULL || pyy == NULL) {
        free(bx);
        free(by);
        free(pxy);
        free(pxx);
        free(pyy);
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                        zap_str_new("statistics: out of memory", 26));
    }
    for (int64_t i = 0; i < n; i++) {
        double dx = bx[i] - xbar;
        double dy = by[i] - ybar;
        pxy[i] = dx * dy;
        pxx[i] = dx * dx;
        pyy[i] = dy * dy;
    }
    double sxy = zap_stats_neumaier(pxy, n);
    double sxx = zap_stats_neumaier(pxx, n);
    double syy = zap_stats_neumaier(pyy, n);
    free(bx);
    free(by);
    free(pxy);
    free(pxx);
    free(pyy);
    double denom = sqrt(sxx * syy);
    if (denom == 0.0) {
        zap_stats_err("statistics.correlation(): at least one of the inputs is constant");
    }
    double r = sxy / denom;
    if (r > 1.0) {
        r = 1.0;
    } else if (r < -1.0) {
        r = -1.0;
    }
    return r;
}

/* Compute the least-squares line y = slope*x + intercept. With `proportional`
 * the line passes through the origin (intercept 0, slope = sum(xy)/sum(xx)).
 * A vertical / constant-x fit (zero denominator) raises. */
static void zap_stats_linreg(ZapList *x, int64_t fx, ZapList *y, int64_t fy,
                             int64_t proportional, double *slope_out,
                             double *intercept_out) {
    double *bx;
    double *by;
    int64_t n = zap_stats_pair(x, fx, y, fy, "linear_regression", &bx, &by);
    double sxy;
    double sxx;
    double xbar = 0.0;
    double ybar = 0.0;
    double *pxy = (double *)malloc((size_t)n * sizeof(double));
    double *pxx = (double *)malloc((size_t)n * sizeof(double));
    if (pxy == NULL || pxx == NULL) {
        free(bx);
        free(by);
        free(pxy);
        free(pxx);
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR,
                        zap_str_new("statistics: out of memory", 26));
    }
    if (proportional) {
        for (int64_t i = 0; i < n; i++) {
            pxy[i] = bx[i] * by[i];
            pxx[i] = bx[i] * bx[i];
        }
    } else {
        xbar = zap_stats_neumaier(bx, n) / (double)n;
        ybar = zap_stats_neumaier(by, n) / (double)n;
        for (int64_t i = 0; i < n; i++) {
            double dx = bx[i] - xbar;
            pxy[i] = dx * (by[i] - ybar);
            pxx[i] = dx * dx;
        }
    }
    sxy = zap_stats_neumaier(pxy, n);
    sxx = zap_stats_neumaier(pxx, n);
    free(bx);
    free(by);
    free(pxy);
    free(pxx);
    if (sxx == 0.0) {
        zap_stats_err("statistics.linear_regression(): x is constant");
    }
    double slope = sxy / sxx;
    *slope_out = slope;
    *intercept_out = proportional ? 0.0 : (ybar - slope * xbar);
}

/* linear_regression slope / intercept — two entry points assembled into the
 * (slope, intercept) tuple in lowering (the modf/frexp build pattern). Each
 * recomputes the fit; acceptable for the scalar-list sizes this targets. */
double zap_stats_linreg_slope(void *xp, int64_t fx, void *yp, int64_t fy,
                              int64_t proportional) {
    double slope;
    double intercept;
    zap_stats_linreg((ZapList *)xp, fx, (ZapList *)yp, fy, proportional, &slope,
                     &intercept);
    return slope;
}
double zap_stats_linreg_intercept(void *xp, int64_t fx, void *yp, int64_t fy,
                                  int64_t proportional) {
    double slope;
    double intercept;
    zap_stats_linreg((ZapList *)xp, fx, (ZapList *)yp, fy, proportional, &slope,
                     &intercept);
    return intercept;
}

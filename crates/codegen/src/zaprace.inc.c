/* Runtime data-race detector — access instrumentation hooks (#677, epic #665,
 * D-0114; epic #661). Slice 1: NO-OP STUBS.
 *
 * Under `--race`, AST->ZIR lowering emits a call to one of these at every
 * instrumented shared-memory access — the ZAP5125 sharing-lint set (#664): a
 * read or write of a MUTABLE, non-`@shareable` module global (scalar or
 * reference). A `const` global, a `@shareable` cell, an access in the
 * single-threaded module-init pre-pass, and a name shadowed by a local emit no
 * call (see `FnLower::emit_race_access`).
 *
 * Arguments (the lowering ABI, fixed here so slice 2 need not change codegen):
 *   slot     - address of the global's data-segment slot being accessed (the
 *              conflict location the happens-before engine keys on);
 *   name_id  - the global's slot index, a stable per-global id (so a report can
 *              name the global);
 *   line,col - the access site's source byte range (span start/end), which the
 *              engine resolves to a human-readable source location.
 *
 * These are REAL exported entry points, not macros: codegen emits a
 * `call __zap_race_read` / `call __zap_race_write` at each site, so the symbols
 * must resolve at link time (exactly like `zap_gc_write_barrier`, #663). They
 * are collector-independent — the detector tracks happens-before over Zap
 * coroutines, not GC state — so there is a single definition for both the
 * conservative Boehm and the precise Immix builds.
 *
 * The happens-before / vector-clock engine that turns these into real
 * conflict-tracking (record the access against the current coroutine's clock,
 * flag a concurrent conflicting access, report and exit non-zero) is the next
 * slice (#678). Until then every build links these inert bodies, so a `--race`
 * build runs identically to a non-`--race` run — only slower by the (empty)
 * calls — which this slice's goldens assert. */

void __zap_race_read(void *slot, int64_t name_id, int64_t line, int64_t col) {
    (void)slot;
    (void)name_id;
    (void)line;
    (void)col;
}

void __zap_race_write(void *slot, int64_t name_id, int64_t line, int64_t col) {
    (void)slot;
    (void)name_id;
    (void)line;
    (void)col;
}

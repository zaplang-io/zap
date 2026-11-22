//! Specialization dedup (#1255, HQ5 — the mono `#671` global-recv model).
//!
//! A comptime generator (a `comptime for`-unrolled codec body, #1251) is
//! specialized per concrete **`(type, backend)`**, lazily, for each one actually
//! **used** — *not* per call-site. The same specialization reached from many
//! call sites is emitted **once**; the same type specialized for two distinct
//! backends yields two instantiations. This cache records which `(type,
//! backend)` bodies have been emitted so the generator driver (mono) emits each
//! once and reuses the symbol thereafter.
//!
//! It models mono's existing generic-instantiation dedup (#671) for the comptime
//! generator layer; the actual symbol emission is wired when the generator lands
//! (E3). Keeping it here lets the dedup discipline be host-tested now.

use std::collections::HashSet;

/// Records emitted `(type, backend)` specializations so each is emitted once.
#[derive(Debug, Default, Clone)]
pub struct SpecializationCache {
    seen: HashSet<(String, String)>,
}

impl SpecializationCache {
    /// An empty cache.
    pub fn new() -> SpecializationCache {
        SpecializationCache::default()
    }

    /// Record a `(type, backend)` specialization. Returns `true` the **first**
    /// time the pair is seen — the caller should emit the specialized body — and
    /// `false` on every later request for the same pair (dedup: reuse the
    /// already-emitted symbol). This is what makes emission per-used, not
    /// per-call-site.
    pub fn record(&mut self, type_name: &str, backend: &str) -> bool {
        self.seen
            .insert((type_name.to_string(), backend.to_string()))
    }

    /// Has this `(type, backend)` already been emitted?
    pub fn contains(&self, type_name: &str, backend: &str) -> bool {
        self.seen
            .contains(&(type_name.to_string(), backend.to_string()))
    }

    /// The number of distinct `(type, backend)` instantiations emitted.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// `true` when nothing has been emitted yet.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

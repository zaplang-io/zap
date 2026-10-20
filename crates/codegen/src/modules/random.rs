//! `random` module — runtime-symbol declarations (ticket #98, D-0056).
//!
//! The xoshiro256** generator: `zap_random_seed(n)` reseeds from an integer;
//! `zap_random_f64()` -> `[0, 1)`; `zap_random_uniform(a, b)` -> `[a, b]`;
//! `zap_random_randint(a, b)` -> inclusive integer range. `zap_random_list_index
//! (list)` returns a random valid index (aborts on an empty list) for `choice`;
//! `zap_random_shuffle(list)` permutes a list's element words in place. The
//! generator autoseeds from OS entropy on first use. See `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let f = cg.ctx.f64_type();
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_random_seed", &[i.into()], None);
    cg.declare_rt("zap_random_f64", &[], Some(f.into()));
    cg.declare_rt("zap_random_uniform", &[f.into(), f.into()], Some(f.into()));
    cg.declare_rt("zap_random_randint", &[i.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_random_list_index", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_random_shuffle", &[p.into()], None);
}

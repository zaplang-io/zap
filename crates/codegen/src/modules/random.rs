//! `random` module — runtime-symbol declarations (ticket #98, D-0056; NumPy
//! `Generator` #930/#931, D-0189).
//!
//! The pre-existing module-level xoshiro256** generator: `zap_random_seed(n)`
//! reseeds; `zap_random_f64()` -> `[0, 1)`; `zap_random_uniform(a, b)`;
//! `zap_random_randint(a, b)`; `zap_random_list_index(list)` / `zap_random_shuffle
//! (list)` back Python `random.choice`/`shuffle`.
//!
//! The NumPy-compatible bit-exact PCG64 `Generator` surface (D-0189) adds an
//! opaque handle (`zap_random_default_rng*` -> a `Ptr`) and, per distribution, a
//! scalar entry point plus an array-fill entry point (the fill writes into a
//! pre-shaped result array). `choice`/`shuffle`/`permutation` round out the
//! sampling surface. See `zaprandom.inc.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let f = cg.ctx.f64_type();
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());

    // --- pre-existing xoshiro surface (D-0056) ---
    cg.declare_rt("zap_random_seed", &[i.into()], None);
    cg.declare_rt("zap_random_f64", &[], Some(f.into()));
    cg.declare_rt("zap_random_uniform", &[f.into(), f.into()], Some(f.into()));
    cg.declare_rt("zap_random_randint", &[i.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_random_list_index", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_random_shuffle", &[p.into()], None);

    // --- NumPy PCG64 Generator (D-0189) ---
    // Construct a generator handle (opaque GC pointer).
    cg.declare_rt("zap_random_default_rng", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_random_default_rng_entropy", &[], Some(p.into()));

    // Scalar draws: (generator, params...) -> scalar.
    cg.declare_rt("zap_random_gen_random", &[p.into()], Some(f.into()));
    cg.declare_rt(
        "zap_random_gen_uniform",
        &[p.into(), f.into(), f.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_integers",
        &[p.into(), i.into(), i.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_random_gen_standard_normal",
        &[p.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_normal",
        &[p.into(), f.into(), f.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_standard_exponential",
        &[p.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_exponential",
        &[p.into(), f.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_standard_gamma",
        &[p.into(), f.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_gamma",
        &[p.into(), f.into(), f.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_beta",
        &[p.into(), f.into(), f.into()],
        Some(f.into()),
    );
    cg.declare_rt(
        "zap_random_gen_poisson",
        &[p.into(), f.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_random_gen_binomial",
        &[p.into(), i.into(), f.into()],
        Some(i.into()),
    );

    // Array fills: (generator, result_array, params...) -> void. The array is
    // pre-allocated and shaped by codegen; the fill writes `count` draws.
    cg.declare_rt("zap_random_gen_random_fill", &[p.into(), p.into()], None);
    cg.declare_rt(
        "zap_random_gen_uniform_fill",
        &[p.into(), p.into(), f.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_integers_fill",
        &[p.into(), p.into(), i.into(), i.into(), i.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_standard_normal_fill",
        &[p.into(), p.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_normal_fill",
        &[p.into(), p.into(), f.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_standard_exponential_fill",
        &[p.into(), p.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_exponential_fill",
        &[p.into(), p.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_standard_gamma_fill",
        &[p.into(), p.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_gamma_fill",
        &[p.into(), p.into(), f.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_beta_fill",
        &[p.into(), p.into(), f.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_poisson_fill",
        &[p.into(), p.into(), f.into()],
        None,
    );
    cg.declare_rt(
        "zap_random_gen_binomial_fill",
        &[p.into(), p.into(), i.into(), f.into()],
        None,
    );

    // Sampling: choice (fill an i64 index array), shuffle (in place),
    // permutation (int -> shuffled arange / array -> shuffled copy).
    cg.declare_rt(
        "zap_random_gen_choice_fill",
        &[p.into(), p.into(), i.into(), i.into(), p.into(), i.into()],
        None,
    );
    cg.declare_rt("zap_random_gen_shuffle", &[p.into(), p.into()], None);
    cg.declare_rt(
        "zap_random_gen_permutation_n",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_random_gen_permutation",
        &[p.into(), p.into()],
        Some(p.into()),
    );
}

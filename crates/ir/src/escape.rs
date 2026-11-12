//! Escape analysis (#839, design discussion #849): lower non-escaping, leaf /
//! pointer-free / finalizer-free heap allocations to the stack.
//!
//! This is a conservative ZIR→ZIR pass run from [`crate::lower::lower_module_opts`]
//! after lowering and before codegen, so it applies uniformly to the AOT path,
//! the REPL JIT, and `zap ir`. It only ever turns a **heap** allocation into a
//! **stack** one; it never changes control flow or observable semantics.
//!
//! ## What it promotes (v1 scope, maintainer-set)
//! A `StructNew` or f-string `StrBuild` whose produced value **provably does not
//! escape its function** AND whose representation is **leaf / pointer-free**
//! (holds no outgoing heap pointers) AND **finalizer-free**:
//! - `StructNew{sid}` → [`Inst::StackStruct`] when every field of `sid` is a
//!   scalar (`!Ty::is_ptr`) and the struct is within the size caps.
//! - `StrBuild` → `StrBuild{stack:true, cap:STR_STACK_CAP}` (strings are
//!   pointer-free atomic objects; the runtime builds into an inline stack buffer
//!   with a heap spill when the length exceeds the cap).
//!
//! Pointer-holding structs are deferred to a v2 (they would need the precise
//! static-root scanner, #663/#672, extended to scan a stack object's interior
//! heap fields). Anything with a finalizer (#92 — today only `Ty::Py`, itself a
//! pointer) stays on the heap.
//!
//! ## Soundness
//! The escape lattice is **monotone**: a value starts non-escaping and is only
//! ever raised to *escaping*; any allocation site, callee, or alias the pass does
//! not understand resolves to *escaping* (heap). A wrong "does not escape" is a
//! use-after-free, so every ambiguity fails safe to the heap. A stack slot is
//! never relocated by moving Immix and holds no outgoing heap pointers, so it
//! needs no GC-root tracking and there is no missed root — see discussion #849 §3.

use crate::ir::{Func, Inst, Module, Term, Ty, Value};
use std::collections::HashMap;

/// Max byte size of a single stack-promoted struct (tunable; fails safe to heap
/// above it). Deliberately well under Go's 64 KB implicit-allocation cap because
/// Zap coroutines run on fixed OS-thread stacks rather than growable goroutine
/// stacks (discussion #849 §7).
const MAX_STACK_ALLOC: u64 = 256;

/// Max total stack bytes this pass may add to one function's frame, summed over
/// **distinct promoted sites** (not dynamic executions — a promoted site inside a
/// loop is one entry-hoisted `alloca`, reused each iteration, #91). Past the
/// budget, further eligible sites stay on the heap.
const MAX_FRAME_BUDGET: u64 = 4096;

/// Inline stack-buffer capacity for a promoted f-string (bound `K`, discussion
/// #849). Go uses 32 B (`tmpStringBufSize`) for the analogous small-string stack
/// buffer; maintainer chose 64 B for more headroom on short keys/lines. A string
/// longer than this spills to a heap `ZapStr` at run time.
const STR_STACK_CAP: u32 = 64;

/// On-frame footprint charged for a promoted f-string: the `{i64 len; char[cap]}`
/// inline buffer (one-object `ZapStr` layout, #838).
const STR_SLOT_SIZE: u64 = 8 + STR_STACK_CAP as u64;

pub fn run(m: &mut Module) {
    // `struct_is_leaf` / `struct_size` only read `m.structs`, which the pass never
    // mutates, so snapshot the bits we need to satisfy the borrow checker while we
    // mutate `m.funcs` in place.
    // Promote only genuine **reference structs** (D-0023) whose fields are all
    // scalars. A `value struct` / tuple (`is_value` / `inline`) is already a
    // by-value aggregate — not a heap `StructNew` the way a reference struct is —
    // and an interface-box cell relies on its heap identity, so both are out of
    // v1 scope (discussion #849). Excluding them also keeps value-semantics,
    // tuple, and interface-boxing lowering byte-for-byte unchanged.
    let leaf: Vec<bool> = m
        .structs
        .iter()
        .map(|s| !s.is_value && !s.inline && s.fields.iter().all(|f| !f.is_ptr()))
        .collect();
    let size: Vec<u64> = m.structs.iter().map(struct_size).collect();
    for f in &mut m.funcs {
        run_fn(f, &leaf, &size);
    }
}

/// Conservative byte size of a struct layout, for the per-allocation cap. Padding
/// is ignored and each field rounded up, so the estimate only ever over-counts —
/// safe for a cap whose job is to reject *large* allocations.
fn struct_size(s: &crate::ir::StructLayout) -> u64 {
    if let Some(p) = &s.packed {
        return ty_size(p.backing);
    }
    // Round the sum up to an 8-byte multiple (worst-case alignment).
    let raw: u64 = s.fields.iter().map(|t| ty_size(*t)).sum();
    raw.div_ceil(8) * 8
}

fn ty_size(t: Ty) -> u64 {
    match t {
        Ty::I1 | Ty::I8 | Ty::U8 => 1,
        Ty::I16 | Ty::U16 => 2,
        Ty::I32 | Ty::U32 | Ty::Char | Ty::F32 => 4,
        Ty::I64 | Ty::U64 | Ty::Enum(_) | Ty::F64 => 8,
        Ty::Void => 0,
        // Any pointer-bearing field disqualifies a struct from being a leaf, so
        // this branch is unreached for a promoted struct; 8 is the pointer width.
        _ => 8,
    }
}

/// A runtime callee **proven not to retain, alias, or return** any pointer
/// argument it is given, so a pointer passed here does not escape through the
/// call. This is a positive allowlist of pure readers/probes — every other callee
/// (all user functions, every closure/interface dispatch, every unlisted runtime
/// function) conservatively retains, so its pointer arguments escape. Keep this
/// list tight: only add a function after confirming in `zaprt.c` that it neither
/// stores the argument into a heap object nor hands it back.
fn call_is_non_retaining(func: &str) -> bool {
    const NON_RETAINING: &[&str] = &[
        "zap_dict_get",      // probe: returns the value, never the key
        "zap_dict_contains", // membership probe
        "zap_set_contains",  // membership probe
        "zap_list_get",      // index-based read
        "zap_str_len",       // reads the length header
        "zap_str_concat",    // allocates a fresh copy; retains neither input
        "zap_str_eq",        // byte compare
        "zap_str_hash",      // hashes bytes
        "zap_fmt_",          // formatters copy into a fresh string
        "zap_render",        // render-to-string copies
    ];
    NON_RETAINING.iter().any(|p| func.starts_with(p))
}

fn run_fn(f: &mut Func, leaf: &[bool], size: &[u64]) {
    let n = f.value_types.len();
    if n == 0 {
        return;
    }

    // --- Clean local slots -------------------------------------------------
    // An `Alloca` whose pointer is used ONLY as the `ptr` of `Load`/`Store`
    // (its address never leaks) models a mutable local variable as a cell: a
    // value stored into it does not escape merely by being stored; it escapes
    // only if a value *loaded* back out of it escapes. Any other use of the
    // alloca pointer (passed to a call, returned, field-set, selected, cast,
    // captured, …) makes the slot "address-captured" and we fall back to the
    // conservative rule (a store into it escapes the stored value).
    let mut is_alloca = vec![false; n];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Alloca { dest, .. } = i {
                is_alloca[*dest as usize] = true;
            }
        }
    }
    let mut clean = is_alloca.clone();
    // Demote any alloca used outside a Load/Store `ptr` position.
    for b in &f.blocks {
        for i in &b.insts {
            visit_non_slot_operands(i, &mut |v| {
                if (v as usize) < n && is_alloca[v as usize] {
                    clean[v as usize] = false;
                }
            });
        }
        if let Term::Ret(Some(v)) = &b.term {
            if (*v as usize) < n && is_alloca[*v as usize] {
                clean[*v as usize] = false;
            }
        }
    }

    // For each clean slot: the values stored into it, and the values loaded out.
    let mut slot_stores: HashMap<Value, Vec<Value>> = HashMap::new();
    let mut slot_loads: HashMap<Value, Vec<Value>> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Store { ptr, val } if (*ptr as usize) < n && clean[*ptr as usize] => {
                    slot_stores.entry(*ptr).or_default().push(*val);
                }
                Inst::Load { dest, ptr } if (*ptr as usize) < n && clean[*ptr as usize] => {
                    slot_loads.entry(*ptr).or_default().push(*dest);
                }
                _ => {}
            }
        }
    }

    // --- Escape fixpoint ---------------------------------------------------
    let mut escaped = vec![false; n];
    let set = |escaped: &mut [bool], changed: &mut bool, v: Value| {
        let idx = v as usize;
        if idx < escaped.len() && !escaped[idx] {
            escaped[idx] = true;
            *changed = true;
        }
    };
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    // A pointer argument escapes through a retaining callee. A
                    // dynamic callee (closure / interface dispatch) is always
                    // opaque, hence retaining.
                    Inst::Call { func, args, .. } | Inst::Invoke { func, args, .. } => {
                        if !call_is_non_retaining(func) {
                            for a in args {
                                set(&mut escaped, &mut changed, *a);
                            }
                        }
                    }
                    Inst::CallClosure { args, .. }
                    | Inst::IfaceCall { args, .. }
                    | Inst::IfaceInvoke { args, .. } => {
                        for a in args {
                            set(&mut escaped, &mut changed, *a);
                        }
                    }
                    // A spawned argument is packaged into a GC closure handed to
                    // another coroutine — always escapes.
                    Inst::Spawn { args, .. } => {
                        for a in args {
                            set(&mut escaped, &mut changed, *a);
                        }
                    }
                    // Captured by a (conservatively escaping) closure record.
                    Inst::ClosureMake { captures, .. } => {
                        for c in captures {
                            set(&mut escaped, &mut changed, *c);
                        }
                    }
                    // Stored into a struct field => reachable through the struct.
                    Inst::FieldSet { val, .. } => set(&mut escaped, &mut changed, *val),
                    // A store into a non-clean (address-captured) slot, a global
                    // slot, or any other pointer escapes the stored value. Stores
                    // into a clean local slot are handled by the slot edges below.
                    Inst::Store { ptr, val } => {
                        let clean_slot = (*ptr as usize) < n && clean[*ptr as usize];
                        if !clean_slot {
                            set(&mut escaped, &mut changed, *val);
                        }
                    }
                    // Alias edges: if the result escapes, so does the source.
                    Inst::Select { dest, a, b: bb, .. } => {
                        if escaped[*dest as usize] {
                            set(&mut escaped, &mut changed, *a);
                            set(&mut escaped, &mut changed, *bb);
                        }
                    }
                    Inst::Cast { dest, val, .. } => {
                        if escaped[*dest as usize] {
                            set(&mut escaped, &mut changed, *val);
                        }
                    }
                    // Boxing a value behind an interface fat pointer: treat the
                    // boxed `data` as always escaping. An interface value's data
                    // pointer flows through dynamic dispatch / may be stored in the
                    // fat pointer that outlives this frame, and reasoning about its
                    // escape precisely is out of v1 scope — conservatively heap.
                    Inst::IfaceMake { data, .. } => set(&mut escaped, &mut changed, *data),
                    _ => {}
                }
            }
            if let Term::Ret(Some(v)) = &b.term {
                set(&mut escaped, &mut changed, *v);
            }
        }
        // Clean-slot edge: a stored value escapes iff some value loaded from the
        // same slot escapes.
        for (slot, stores) in &slot_stores {
            let loads_escape = slot_loads
                .get(slot)
                .is_some_and(|ls| ls.iter().any(|d| escaped[*d as usize]));
            if loads_escape {
                for sv in stores {
                    set(&mut escaped, &mut changed, *sv);
                }
            }
        }
        if !changed {
            break;
        }
    }

    // --- Promote -----------------------------------------------------------
    // Rewrite each non-escaping, eligible site, charging a shared per-function
    // frame budget (counted over distinct static sites).
    let mut budget = MAX_FRAME_BUDGET;
    for b in &mut f.blocks {
        for i in &mut b.insts {
            match i {
                Inst::StructNew { dest, sid } => {
                    let d = *dest;
                    let s = *sid;
                    if !escaped[d as usize]
                        && leaf[s as usize]
                        && size[s as usize] <= MAX_STACK_ALLOC
                        && size[s as usize] <= budget
                    {
                        budget -= size[s as usize];
                        *i = Inst::StackStruct { dest: d, sid: s };
                    }
                }
                Inst::StrBuild {
                    dest, stack, cap, ..
                } if !*stack && !escaped[*dest as usize] && STR_SLOT_SIZE <= budget => {
                    budget -= STR_SLOT_SIZE;
                    *stack = true;
                    *cap = STR_STACK_CAP;
                }
                _ => {}
            }
        }
    }
}

/// Visit every operand value of `i` **except** a `Load`/`Store`'s `ptr` field.
/// Used only to detect address-captured allocas: the `Load`/`Store` `ptr`
/// positions are the legitimate use of a local-variable slot and are excluded,
/// so any alloca reported here appears in a position that leaks its address and
/// must be demoted from "clean". The match is **exhaustive** (no wildcard) so a
/// future `Inst` variant forces a conscious decision rather than silently being
/// treated as slot-safe (which would be unsound).
fn visit_non_slot_operands(i: &Inst, f: &mut dyn FnMut(Value)) {
    match i {
        Inst::IConst { .. }
        | Inst::FConst { .. }
        | Inst::BConst { .. }
        | Inst::NullConst { .. }
        | Inst::StrConst { .. }
        | Inst::StructNew { .. }
        | Inst::StackStruct { .. }
        | Inst::Alloca { .. }
        | Inst::FuncAddr { .. }
        | Inst::GlobalAddr { .. }
        | Inst::LandingPad { .. }
        // A `Load`'s only operand is the excluded `ptr`.
        | Inst::Load { .. } => {}
        Inst::IsNull { val, .. } | Inst::Un { val, .. } | Inst::Cast { val, .. } => f(*val),
        Inst::Bin { lhs, rhs, .. } | Inst::ICmp { lhs, rhs, .. } | Inst::FCmp { lhs, rhs, .. } => {
            f(*lhs);
            f(*rhs);
        }
        // A `Store`'s `ptr` is excluded; its `val` is a real (non-slot) operand.
        Inst::Store { val, .. } => f(*val),
        Inst::Select { cond, a, b, .. } => {
            f(*cond);
            f(*a);
            f(*b);
        }
        Inst::Call { args, .. } | Inst::Invoke { args, .. } | Inst::Spawn { args, .. } => {
            for a in args {
                f(*a);
            }
        }
        Inst::Print { args } => {
            for a in args {
                f(*a);
            }
        }
        Inst::StrBuild { parts, .. } => {
            for p in parts {
                f(*p);
            }
        }
        Inst::FieldGet { base, .. } => f(*base),
        Inst::FieldSet { base, val, .. } => {
            f(*base);
            f(*val);
        }
        Inst::ArrayBinOp { lhs, rhs, .. } => {
            f(*lhs);
            f(*rhs);
        }
        Inst::ClosureMake { captures, .. } => {
            for c in captures {
                f(*c);
            }
        }
        Inst::CallClosure { callee, args, .. } => {
            f(*callee);
            for a in args {
                f(*a);
            }
        }
        Inst::IfaceMake { data, .. } => f(*data),
        Inst::IfaceCall { iface, args, .. } | Inst::IfaceInvoke { iface, args, .. } => {
            f(*iface);
            for a in args {
                f(*a);
            }
        }
        Inst::IfaceRender { iface, .. } | Inst::IfaceRenderInvoke { iface, .. } => f(*iface),
        // Atomic ops operate on an `Atomic[T]` box `base` (and value operands) —
        // none is a local-variable slot pointer, so visiting them demotes any
        // alloca used there (conservative / heap-safe).
        Inst::AtomicLoad { base, .. } => f(*base),
        Inst::AtomicStore { base, val, .. } => {
            f(*base);
            f(*val);
        }
        Inst::AtomicRmw { base, val, .. } => {
            f(*base);
            f(*val);
        }
        Inst::AtomicCas {
            base,
            expected,
            desired,
            ..
        } => {
            f(*base);
            f(*expected);
            f(*desired);
        }
    }
}

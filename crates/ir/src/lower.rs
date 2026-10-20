//! AST -> ZIR lowering.
//!
//! Mirrors the vertical slice the backend supports: `int64`/`float64`/`bool`,
//! functions (incl. recursion), `let`/assign/aug-assign, `if`/`while`/
//! `do-while`/`for`-over-`range`, `return`, calls, arithmetic/comparison, and
//! `print`. Type-directed decisions (int vs float ops, widening casts,
//! truthiness) happen here, so the ZIR -> LLVM step is mechanical. Unsupported
//! constructs produce a [`LowerError`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use zapc_syntax::ast::{
    self, BinOp as AstBin, BindTarget, Expr, PatArg, Pattern, Stmt, UnOp as AstUn,
};

use crate::ir::*;

/// Built-in stdlib module registry (ticket #121): per-module lowering lives in
/// `lower/modules/<name>.rs`, dispatched by a data table instead of a hand-
/// written `match (module, method)` block, so new modules are conflict-free
/// parallel-addable. See [`modules`] for the seam contract.
mod modules;

/// Interns anonymous tuple layouts (D-0049). A tuple is a by-value aggregate laid
/// out like an anonymous struct (positional order), so it reuses the struct
/// machinery: each distinct element-type sequence gets one [`Ty::Struct`] id,
/// allocated after the named structs (`base = number of named structs`). The
/// interned layouts are appended to [`Module::structs`] once lowering finishes,
/// so their ids line up with `base + index`.
#[derive(Default)]
struct TupleInterner {
    base: u32,
    layouts: Vec<Vec<Ty>>,
    map: HashMap<Vec<Ty>, u32>,
}

impl TupleInterner {
    fn new(base: u32) -> TupleInterner {
        TupleInterner {
            base,
            layouts: Vec::new(),
            map: HashMap::new(),
        }
    }

    /// The struct id for a tuple with these element types (deduplicated).
    fn intern(&mut self, elems: Vec<Ty>) -> u32 {
        if let Some(&sid) = self.map.get(&elems) {
            return sid;
        }
        let sid = self.base + self.layouts.len() as u32;
        self.map.insert(elems.clone(), sid);
        self.layouts.push(elems);
        sid
    }

    /// Whether `sid` names a tuple layout (vs a user struct / tier-3 enum).
    fn is_tuple(&self, sid: u32) -> bool {
        sid >= self.base && ((sid - self.base) as usize) < self.layouts.len()
    }

    /// The element types of tuple `sid`, if it is one.
    fn fields(&self, sid: u32) -> Option<Vec<Ty>> {
        if sid < self.base {
            return None;
        }
        self.layouts.get((sid - self.base) as usize).cloned()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LowerError {
    pub message: String,
}

/// One data-carrying variant of a tier-3 enum (D-0046), resolved for lowering.
#[derive(Clone, Debug)]
struct VariantLow {
    name: String,
    /// The discriminant stored in the tag slot (declaration order).
    tag: i64,
    /// Payload fields: `(name, struct-field index in the tagged struct, type)`.
    /// The index points into the tagged struct, whose field 0 is the tag.
    fields: Vec<(String, u32, Ty)>,
}

/// A lowered enum (D-0018 tier-1 / D-0046 tier-3).
///
/// A **tier-1** enum (`is_sum == false`) is an unboxed backing integer (`i64`):
/// members carry explicit or auto-incremented discriminants. A **tier-3** enum
/// (`is_sum == true`) is a GC-allocated **tagged struct** `{ tag:i32, payload…
/// }`: `sid` indexes [`Module::structs`]. Reusing the struct machinery means a
/// recursive variant field (whose type is the enum itself) is already a pointer
/// to a heap enum value, which breaks the cycle for free (D-0046).
#[derive(Clone, Debug)]
struct EnumLow {
    is_sum: bool,
    /// Tagged-struct id (tier-3 only).
    sid: Option<u32>,
    /// Members in declaration order: `(name, discriminant)` (tier-1).
    members: Vec<(String, i64)>,
    /// Variants in declaration order (tier-3).
    variants: Vec<VariantLow>,
}

impl EnumLow {
    fn member_value(&self, name: &str) -> Option<i64> {
        self.members
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
    }
    fn variant(&self, name: &str) -> Option<&VariantLow> {
        self.variants.iter().find(|v| v.name == name)
    }
}

/// The information `resolve_ty` needs to map a named type to a ZIR type: struct
/// / tier-3-enum ids, plus the enum registry (so a tier-1 enum resolves to its
/// backing integer and a tier-3 enum to its tagged struct).
struct TyEnv<'a> {
    ids: &'a HashMap<String, u32>,
    enums: &'a HashMap<String, EnumLow>,
    /// Structural interfaces (D-0016): interface name -> the synthetic inline
    /// `{ptr data, ptr itable}` struct id (`fat_sid`) that an interface value has
    /// as its ZIR type. An interface-typed annotation resolves to this.
    ifaces: &'a HashMap<String, u32>,
    /// Interner for anonymous tuple layouts (D-0049), shared so every
    /// `(T, U, ...)` type resolves to the same struct id across the module.
    tuples: &'a RefCell<TupleInterner>,
}

/// Lower a type-checked module to ZIR.
/// Struct layouts/ids collected before lowering function bodies.
struct StructCtx {
    ids: HashMap<String, u32>,
    names: Vec<String>,
    fields: Vec<Vec<(String, Ty)>>,
    /// Struct ids that are `value struct`s (value types, D-0051). A value struct
    /// has value semantics: copied on assign/pass/return (no shared identity).
    value_ids: HashSet<u32>,
    /// Registered enums (tier-1 and tier-3), keyed by type name.
    enums: HashMap<String, EnumLow>,
    /// Python import aliases (ticket #27, D-0012): the in-scope name
    /// (`import python "numpy" as np` → `np`) mapped to the CPython module name
    /// (`numpy`). A reference to one of these names lowers to a `zap_py_import`.
    py: HashMap<String, String>,
    /// Interned anonymous tuple layouts (D-0049), appended to [`Module::structs`]
    /// after lowering.
    tuples: RefCell<TupleInterner>,
    /// Structural interfaces (D-0016): interface name -> `fat_sid` (its synthetic
    /// inline `{ptr data, ptr itable}` struct layout id). An interface value's
    /// ZIR type is `Ty::Struct(fat_sid)`.
    ifaces: HashMap<String, u32>,
    /// Interface metadata, indexed by interface id (`iid`); `iid` is the position
    /// in this vector. Used to map a `fat_sid` back to its interface for dynamic
    /// dispatch and to build the per-method dispatch-table slots.
    interfaces: Vec<InterfaceLow>,
    /// Reverse map `fat_sid -> iid`, so a method call on an interface-typed
    /// receiver resolves to the owning interface.
    fat_to_iid: HashMap<u32, u32>,
    /// Generator functions (I#67, D-0026/D-0048), keyed by source function name.
    /// A call `gen(args)` constructs the function's synthetic state struct
    /// instead of calling a function; see [`GeneratorInfo`].
    generators: HashMap<String, GeneratorInfo>,
}

/// A compiler-generated generator (I#67, D-0026/D-0048). A free `def` whose body
/// contains `yield` is lowered to a heap **state struct** that structurally
/// implements the `Iterator` protocol (D-0048): `__iter__(self) -> self` and the
/// comma-ok `__next__(self) -> (T, bool)`. Calling the generator constructs an
/// instance of this struct (capturing the arguments); each `__next__` resumes
/// the body to the next `yield` (or signals exhaustion).
#[derive(Clone)]
struct GeneratorInfo {
    /// Source function name (also the key in [`StructCtx::generators`]).
    name: String,
    /// The synthetic state-struct id (indexes `Module::structs`).
    sid: u32,
    /// Element type `T` — the join of the `yield` operand types.
    elem: Ty,
    /// The `(T, bool)` tuple layout id that `__next__` returns.
    next_tuple_sid: u32,
    /// The state-struct fields that back the generator's live values, in field
    /// order: `(name, type)`. The first `n_params` are the generator's
    /// parameters (captured at construction); the rest are body locals. A var at
    /// position `p` lives in field `p + 1` — field `0` is the `$state`
    /// discriminant (`i64`), set to `0` at construction, to the resume id at each
    /// `yield`, and to `-1` at exhaustion.
    vars: Vec<(String, Ty)>,
    /// How many leading `vars` are the generator's parameters.
    n_params: usize,
}

/// Field index of generator var at position `p` (slot `0` is `$state`).
fn gen_var_field(p: usize) -> u32 {
    (p + 1) as u32
}

/// Lowered interface metadata (D-0016). `fat_sid` is the synthetic inline
/// `{ptr data, ptr itable}` struct layout; `methods` is the dispatch-table slot
/// order (interface declaration order).
#[derive(Clone)]
struct InterfaceLow {
    name: String,
    fat_sid: u32,
    methods: Vec<IfaceMethodLow>,
}

/// One interface method slot (D-0016): its name (used to resolve a call and to
/// name the implementor's `Struct__method` in the itable) and its ZIR signature
/// (excluding `self`), used to coerce arguments and to type the dispatch.
#[derive(Clone)]
struct IfaceMethodLow {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
}

impl StructCtx {
    /// Index and type of `name` in struct `sid`.
    fn field(&self, sid: u32, name: &str) -> Option<(u32, Ty)> {
        self.fields[sid as usize]
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n == name)
            .map(|(i, (_, t))| (i as u32, *t))
    }

    fn tyenv(&self) -> TyEnv<'_> {
        TyEnv {
            ids: &self.ids,
            enums: &self.enums,
            ifaces: &self.ifaces,
            tuples: &self.tuples,
        }
    }

    /// If `sid` is an interface's synthetic fat-pointer layout, its interface id.
    fn iid_of_fat_sid(&self, sid: u32) -> Option<u32> {
        self.fat_to_iid.get(&sid).copied()
    }

    /// Is `sid` a `value struct` (value type, D-0051)?
    fn is_value_struct(&self, sid: u32) -> bool {
        self.value_ids.contains(&sid)
    }

    /// Whether `sid` uses the physical inline / register / sret layout (D-0051):
    /// a value struct or a tuple, all of whose fields are non-aggregate (no field
    /// is itself a value struct or tuple). This is the single source of truth for
    /// [`StructLayout::inline`]; lowering consults it to decide where the
    /// by-value calling convention already supplies the value-semantics copy, so
    /// an explicit copy can be dropped. See [`StructLayout::inline`] for why a
    /// nested value-aggregate field keeps the heap representation.
    fn inline_eligible(&self, sid: u32) -> bool {
        let is_tuple = self.tuples.borrow().is_tuple(sid);
        if !(self.is_value_struct(sid) || is_tuple) {
            return false;
        }
        let blocking = |fty: &Ty| -> bool {
            matches!(fty, Ty::Struct(fsid)
                if self.is_value_struct(*fsid) || self.tuples.borrow().is_tuple(*fsid))
        };
        if is_tuple {
            match self.tuples.borrow().fields(sid) {
                Some(fields) => !fields.iter().any(&blocking),
                None => false,
            }
        } else {
            !self.fields[sid as usize].iter().any(|(_, f)| blocking(f))
        }
    }
}

/// An expression whose value is an *existing object* (an lvalue read), so a
/// value struct obtained from it must be copied to preserve value semantics
/// (D-0051). A freshly produced value — a constructor call, function/method
/// call, or operator result — is already uniquely owned and needs no copy.
fn is_lvalue_read(e: &ast::Expr) -> bool {
    matches!(
        e,
        ast::Expr::Name(..) | ast::Expr::Attr { .. } | ast::Expr::Index { .. }
    )
}

/// The in-place augmented-assignment dunder for an operator (D-0047): `a OP= b`
/// calls `a.__iOP__(b)` when the LHS struct type defines it, mutating in place
/// and avoiding the fresh allocation the binary `__OP__` form needs. Only the
/// arithmetic / bitwise / shift operators that can appear in an augmented
/// assignment have one; everything else returns `None` (and falls back to the
/// derive-from-binary form).
fn inplace_op_dunder(op: AstBin) -> Option<&'static str> {
    use AstBin::*;
    Some(match op {
        Add => "__iadd__",
        Sub => "__isub__",
        Mul => "__imul__",
        Div => "__itruediv__",
        FloorDiv => "__ifloordiv__",
        Mod => "__imod__",
        Pow => "__ipow__",
        BitAnd => "__iand__",
        BitOr => "__ior__",
        BitXor => "__ixor__",
        Shl => "__ilshift__",
        Shr => "__irshift__",
        _ => return None,
    })
}

pub fn lower_module(module: &ast::Module) -> (Module, Vec<LowerError>) {
    // Prepend the built-in prelude (`Signal` enum, `MemStats` struct) so the
    // signals surface lowers through the ordinary enum/struct paths. Must match
    // the set sema sees (both call the same helper), ticket #77.
    let module = &zapc_syntax::parser::with_prelude(module);
    let mut errors = Vec::new();

    // Collect struct ids and tier-3-enum tagged-struct ids (a tier-3 enum is
    // lowered as a struct, D-0046), then resolve field types. Multiple passes so
    // fields may reference types declared later, and so a recursive enum variant
    // (whose field type is the enum itself) resolves to a pointer.
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let mut value_ids: HashSet<u32> = HashSet::new();
    for item in &module.items {
        match item {
            ast::Item::Struct(s) => {
                let sid = names.len() as u32;
                ids.insert(s.name.clone(), sid);
                names.push(s.name.clone());
                // Record the value-vs-reference kind (D-0051); a `value struct`
                // gets value semantics at lowering.
                if s.is_value {
                    value_ids.insert(sid);
                }
            }
            // A data-carrying enum needs a struct id reserved up front so its
            // own (possibly recursive) variant fields resolve to a pointer.
            ast::Item::Enum(e) if e.variants.iter().any(|v| !v.fields.is_empty()) => {
                ids.insert(e.name.clone(), names.len() as u32);
                names.push(e.name.clone());
            }
            // A generator (a free `def` with `yield`, I#67) gets a synthetic
            // state-struct id reserved up front — its layout is filled after the
            // function signatures are known (its live locals need type inference).
            ast::Item::Function(f)
                if zapc_syntax::genanalysis::is_generator(f) && f.name != "main" =>
            {
                let name = zapc_syntax::genanalysis::gen_state_name(&f.name);
                ids.insert(name.clone(), names.len() as u32);
                names.push(name);
            }
            _ => {}
        }
    }

    // Interfaces (D-0016): each gets a synthetic inline `{ptr data, ptr itable}`
    // struct layout (`fat_sid`) — the ZIR type of an interface value is
    // `Ty::Struct(fat_sid)`. Reserve the ids up front (like structs/enums) so an
    // interface-typed field/parameter resolves, and so these ids are claimed
    // BEFORE the tuple interner captures `names.len()` as its base. The layout
    // name is prefixed with `$iface:` so it never collides with a user type.
    let mut ifaces: HashMap<String, u32> = HashMap::new();
    for item in &module.items {
        if let ast::Item::Interface(i) = item {
            let fat_sid = names.len() as u32;
            ifaces.insert(i.name.clone(), fat_sid);
            names.push(format!("$iface:{}", i.name));
        }
    }

    // Anonymous tuple layouts (D-0049) are interned after the named structs, so
    // their struct ids start at `names.len()`. The interner is shared across the
    // field/signature pre-passes and all body lowering, then its layouts are
    // appended to `Module::structs`.
    let tuples = RefCell::new(TupleInterner::new(names.len() as u32));

    // Build the enum registry. For tier-1 enums, compute member discriminants
    // (explicit, else previous + 1 from 0). For tier-3 enums, lay out the tagged
    // struct `{ tag:i32, <flattened payload fields> }` and record each variant's
    // tag and its fields' struct indices.
    let mut enums: HashMap<String, EnumLow> = HashMap::new();
    {
        // A borrow of `ids`/`enums` is needed to resolve payload field types,
        // but enum entries are inserted as they are built, so recursive and
        // forward references resolve against `ids` directly.
        for item in &module.items {
            if let ast::Item::Enum(e) = item {
                let is_sum = e.variants.iter().any(|v| !v.fields.is_empty());
                if !is_sum {
                    let mut members = Vec::new();
                    let mut next = 0i64;
                    for v in &e.variants {
                        let value = v.value.as_ref().and_then(const_int_value).unwrap_or(next);
                        members.push((v.name.clone(), value));
                        next = value + 1;
                    }
                    enums.insert(
                        e.name.clone(),
                        EnumLow {
                            is_sum: false,
                            sid: None,
                            members,
                            variants: Vec::new(),
                        },
                    );
                } else {
                    let sid = ids[&e.name];
                    let mut variants = Vec::new();
                    // Struct field 0 is the tag; payload fields follow, flattened
                    // across variants (each variant's fields occupy their own
                    // slots — a space-optimal overlapping union is a follow-up).
                    let mut next_idx = 1u32;
                    for (tag, v) in e.variants.iter().enumerate() {
                        let mut vfields = Vec::new();
                        for f in &v.fields {
                            let ft = resolve_ty(
                                &f.ty,
                                &TyEnv {
                                    ids: &ids,
                                    enums: &enums,
                                    ifaces: &ifaces,
                                    tuples: &tuples,
                                },
                            )
                            .unwrap_or(Ty::Ptr);
                            vfields.push((f.name.clone(), next_idx, ft));
                            next_idx += 1;
                        }
                        variants.push(VariantLow {
                            name: v.name.clone(),
                            tag: tag as i64,
                            fields: vfields,
                        });
                    }
                    enums.insert(
                        e.name.clone(),
                        EnumLow {
                            is_sum: true,
                            sid: Some(sid),
                            members: Vec::new(),
                            variants,
                        },
                    );
                }
            }
        }
    }

    let mut fields: Vec<Vec<(String, Ty)>> = vec![Vec::new(); names.len()];
    {
        let tyenv = TyEnv {
            ids: &ids,
            enums: &enums,
            ifaces: &ifaces,
            tuples: &tuples,
        };
        for item in &module.items {
            if let ast::Item::Struct(s) = item {
                let sid = ids[&s.name];
                for fld in &s.fields {
                    let ft = resolve_ty(&fld.ty, &tyenv).unwrap_or(Ty::Ptr);
                    fields[sid as usize].push((fld.name.clone(), ft));
                }
            }
        }
        // Fill in each tier-3 enum's tagged-struct layout: tag + payload slots.
        for e in enums.values() {
            if let (true, Some(sid)) = (e.is_sum, e.sid) {
                let slot = &mut fields[sid as usize];
                slot.push(("$tag".to_string(), Ty::I32));
                for v in &e.variants {
                    for (fname, _idx, ft) in &v.fields {
                        slot.push((format!("{}${}", v.name, fname), *ft));
                    }
                }
            }
        }
        // Lay out each interface's synthetic fat-pointer struct as
        // `{ ptr data, ptr itable }` (D-0016, design/0006 §2.4).
        for &fat_sid in ifaces.values() {
            fields[fat_sid as usize] = vec![
                ("$data".to_string(), Ty::Ptr),
                ("$itable".to_string(), Ty::Ptr),
            ];
        }
    }

    // Build the interface metadata (D-0016): per interface, resolve each method's
    // ZIR signature (excluding `self`) in declaration order — the dispatch-table
    // slot order. `iid` is the index into `interfaces`.
    let mut interfaces: Vec<InterfaceLow> = Vec::new();
    let mut fat_to_iid: HashMap<u32, u32> = HashMap::new();
    {
        let tyenv = TyEnv {
            ids: &ids,
            enums: &enums,
            ifaces: &ifaces,
            tuples: &tuples,
        };
        for item in &module.items {
            if let ast::Item::Interface(i) = item {
                let fat_sid = ifaces[&i.name];
                let iid = interfaces.len() as u32;
                fat_to_iid.insert(fat_sid, iid);
                let methods = i
                    .methods
                    .iter()
                    .map(|m| {
                        // Parameter types excluding the leading `self`.
                        let params = m
                            .params
                            .iter()
                            .filter(|p| p.name != "self")
                            .map(|p| {
                                p.ty.as_ref()
                                    .map(|t| resolve_ty(t, &tyenv).unwrap_or(Ty::Ptr))
                                    .unwrap_or(Ty::Ptr)
                            })
                            .collect();
                        let ret = m
                            .ret
                            .as_ref()
                            .map(|t| resolve_ty(t, &tyenv).unwrap_or(Ty::Void))
                            .unwrap_or(Ty::Void);
                        IfaceMethodLow {
                            name: m.name.clone(),
                            params,
                            ret,
                        }
                    })
                    .collect();
                interfaces.push(InterfaceLow {
                    name: i.name.clone(),
                    fat_sid,
                    methods,
                });
            }
        }
    }
    // Python imports (ticket #27, D-0012): map each imported module's in-scope
    // name to its CPython module name. The alias defaults to the module string
    // when `as` is omitted.
    let mut py: HashMap<String, String> = HashMap::new();
    for item in &module.items {
        if let ast::Item::Import(ast::Import::Python { module, alias, .. }) = item {
            let name = alias.clone().unwrap_or_else(|| module.clone());
            py.insert(name, module.clone());
        }
    }

    let mut ctx = StructCtx {
        ids,
        names,
        fields,
        value_ids,
        enums,
        py,
        tuples,
        ifaces,
        interfaces,
        fat_to_iid,
        generators: HashMap::new(),
    };

    // Signatures: top-level functions and struct methods (mangled `S__m`).
    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    for item in &module.items {
        match item {
            ast::Item::Function(f) => {
                let params = f.params.iter().map(|p| param_ty(p, None, &ctx)).collect();
                // A generator's call returns its synthetic iterator state struct
                // (I#67), not the declared/`void` type; the body is compiled into
                // the struct's `__next__`, not a function named `f`.
                let ret = if zapc_syntax::genanalysis::is_generator(f) && f.name != "main" {
                    Ty::Struct(ctx.ids[&zapc_syntax::genanalysis::gen_state_name(&f.name)])
                } else {
                    f.ret
                        .as_ref()
                        .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Void))
                        .unwrap_or(Ty::Void)
                };
                sigs.insert(f.name.clone(), (params, ret));
            }
            ast::Item::Struct(s) => {
                let sid = ctx.ids[&s.name];
                for m in &s.methods {
                    let params = m
                        .params
                        .iter()
                        .map(|p| param_ty(p, Some(sid), &ctx))
                        .collect();
                    let ret = m
                        .ret
                        .as_ref()
                        .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Void))
                        .unwrap_or(Ty::Void);
                    sigs.insert(format!("{}__{}", s.name, m.name), (params, ret));
                }
            }
            _ => {}
        }
    }

    // Decorators & annotations (ticket #16, D-0036). Register the signatures of
    // the methods `@derive(...)` will generate, so user calls (`u.eq(v)`) resolve
    // during body lowering, and collect the runtime-reflection metadata.
    let deco = DecoratorPlan::collect(module, &ctx, &mut errors);
    for (sid, traits) in &deco.derived {
        for t in traits {
            for (mname, psig, ret) in t.signatures(*sid) {
                sigs.insert(
                    format!("{}__{}", ctx.names[*sid as usize], mname),
                    (psig, ret),
                );
            }
        }
    }

    // Generators (I#67): discover each generator's live locals + element type,
    // lay out its state struct, and register the synthetic `__iter__`/`__next__`
    // signatures — all before body lowering, so a `for x in gen()` and the
    // emitted methods resolve through the ordinary struct/method machinery.
    let mut gen_map: HashMap<String, GeneratorInfo> = HashMap::new();
    for item in &module.items {
        let ast::Item::Function(f) = item else {
            continue;
        };
        if !(zapc_syntax::genanalysis::is_generator(f) && f.name != "main") {
            continue;
        }
        let (locals, elem) = match discover_generator(f, &sigs, &ctx) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let sname = zapc_syntax::genanalysis::gen_state_name(&f.name);
        let sid = ctx.ids[&sname];
        // State-struct vars: the parameters (captured at construction) then the
        // discovered body locals; field index is `1 + position` (`$state` is 0).
        let param_tys = sigs[&f.name].0.clone();
        let mut vars: Vec<(String, Ty)> = f
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), param_tys[i]))
            .collect();
        let n_params = vars.len();
        vars.extend(locals);
        let next_tuple_sid = ctx.tuples.borrow_mut().intern(vec![elem, Ty::I1]);
        // Fill the state struct's layout: `$state` discriminant + the var fields.
        let mut layout: Vec<(String, Ty)> = vec![("$state".to_string(), Ty::I64)];
        layout.extend(vars.iter().cloned());
        ctx.fields[sid as usize] = layout;
        // Register the iterator dunders so the for-loop/comprehension driver and
        // `setup_iter`/`next_form` find them.
        sigs.insert(
            format!("{sname}____iter__"),
            (vec![Ty::Struct(sid)], Ty::Struct(sid)),
        );
        sigs.insert(
            format!("{sname}____next__"),
            (vec![Ty::Struct(sid)], Ty::Struct(next_tuple_sid)),
        );
        gen_map.insert(
            f.name.clone(),
            GeneratorInfo {
                name: f.name.clone(),
                sid,
                elem,
                next_tuple_sid,
                vars,
                n_params,
            },
        );
    }
    ctx.generators = gen_map;

    let mut out = Module::default();
    for sid in 0..ctx.names.len() {
        out.structs.push(StructLayout {
            name: ctx.names[sid].clone(),
            fields: ctx.fields[sid].iter().map(|(_, t)| *t).collect(),
            is_value: ctx.is_value_struct(sid as u32),
            inline: false, // set in the post-pass below, once all layouts exist
        });
    }
    for item in &module.items {
        match item {
            // A generator is lowered to its state struct's `__iter__`/`__next__`
            // (I#67), not a function named `f`.
            ast::Item::Function(f) if ctx.generators.contains_key(&f.name) => {
                let gi = ctx.generators[&f.name].clone();
                out.funcs.push(lower_generator_iter(&gi, &sigs, &ctx));
                match lower_generator_next(f, &gi, &sigs, &ctx) {
                    Ok(func) => out.funcs.push(func),
                    Err(e) => errors.push(e),
                }
            }
            ast::Item::Function(f) => {
                match lower_fn(
                    f,
                    f.name.clone(),
                    None,
                    &sigs,
                    &ctx,
                    &deco.reflect_meta,
                    &deco.on_signal_meta,
                ) {
                    Ok(func) => out.funcs.push(func),
                    Err(e) => errors.push(e),
                }
            }
            ast::Item::Struct(s) => {
                let sid = ctx.ids[&s.name];
                for m in &s.methods {
                    let name = format!("{}__{}", s.name, m.name);
                    match lower_fn(m, name, Some(sid), &sigs, &ctx, &[], &[]) {
                        Ok(func) => out.funcs.push(func),
                        Err(e) => errors.push(e),
                    }
                }
            }
            _ => {}
        }
    }

    // Generate the bodies of the derived methods (compile-time expansion of the
    // built-in `@derive(...)` decorator, kept isolated from expression lowering).
    // `@derive(Json)` and `@derive(Yaml)` share the DOM decode core
    // (`{Struct}__from_json_value`); when a struct derives both, it is emitted by
    // each, so dedup generated functions by name (keep the first).
    let mut gen_names: HashSet<String> = HashSet::new();
    for (sid, traits) in &deco.derived {
        for t in traits {
            match t.generate(*sid, &sigs, &ctx) {
                Ok(funcs) => {
                    for f in funcs {
                        if gen_names.insert(f.name.clone()) {
                            out.funcs.push(f);
                        }
                    }
                }
                Err(e) => errors.push(e),
            }
        }
    }

    // Append the interned tuple layouts (D-0049) after the named structs. Their
    // ids were handed out as `base + index` with `base == out.structs.len()`
    // here, so they line up positionally. A tuple is laid out exactly like an
    // anonymous struct, so codegen needs no tuple-specific path.
    for elems in ctx.tuples.borrow().layouts.iter() {
        out.structs.push(StructLayout {
            name: "(tuple)".to_string(),
            fields: elems.clone(),
            is_value: false,
            inline: false, // set in the post-pass below
        });
    }

    // Physical inline-layout pass (D-0051). A value struct or a tuple is a
    // value-semantic aggregate with no shared identity, so it can be laid out
    // inline and passed/returned by value (register/sret) instead of as a heap
    // pointer — but only when none of its fields is *itself* a value aggregate
    // (see `StructCtx::inline_eligible`, the shared predicate).
    for sid in 0..out.structs.len() {
        out.structs[sid].inline = ctx.inline_eligible(sid as u32);
    }

    // Interface fat pointers are passed/returned by value exactly like an inline
    // value aggregate (D-0016, design/0006 §2.4: "the one reference type wider
    // than a single word"), so force their synthetic `{ptr, ptr}` layout inline.
    // Emit the interface table so codegen can build dispatch itables.
    for i in &ctx.interfaces {
        out.structs[i.fat_sid as usize].inline = true;
        out.interfaces.push(InterfaceLayout {
            name: i.name.clone(),
            fat_sid: i.fat_sid,
            methods: i
                .methods
                .iter()
                .map(|m| IfaceMethod {
                    name: m.name.clone(),
                    params: m.params.clone(),
                    ret: m.ret,
                })
                .collect(),
        });
    }

    (out, errors)
}

fn resolve_ty(t: &ast::Type, env: &TyEnv) -> Result<Ty, LowerError> {
    match t {
        ast::Type::Named { name, args, .. } => match name.as_str() {
            "int64" | "i64" => Ok(Ty::I64),
            "int32" | "i32" => Ok(Ty::I32),
            "float64" | "f64" => Ok(Ty::F64),
            "float32" | "f32" => Ok(Ty::F32),
            // `uint8`/`u8` is currently only an `array[T]` dtype (ticket #49),
            // not a general scalar type; it resolves to `i8` so `dtype_of`
            // recognizes it in a type-annotation `array[uint8]`.
            "uint8" | "u8" => Ok(Ty::I8),
            "bool" => Ok(Ty::I1),
            "char" => Ok(Ty::I32),
            "str" | "bytes" => Ok(Ty::Ptr),
            "pyobject" => Ok(Ty::Py),
            "list" => {
                let e = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::List(e))
            }
            "array" => {
                let d = match args.first() {
                    Some(a) => dtype_of(a, env)?,
                    None => Dtype::F64,
                };
                Ok(Ty::Array(d))
            }
            "dict" => {
                let k = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                let v = match args.get(1) {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Dict(k, v))
            }
            "set" => {
                let e = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Set(e))
            }
            "heap" => {
                let e = match args.first() {
                    Some(a) => heap_elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Heap(e))
            }
            "channel" => {
                // A channel's element uses the same uniform-word representation
                // as a collection element (ticket #25, D-0031): the element kind
                // picks the typed send/recv runtime entry point.
                let e = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Chan(e))
            }
            other => {
                // A tier-1 enum is its unboxed backing integer; a tier-3 enum is
                // its tagged struct; a struct is its layout (D-0018/D-0046).
                if let Some(e) = env.enums.get(other) {
                    if e.is_sum {
                        return Ok(Ty::Struct(e.sid.expect("tier-3 enum has a struct id")));
                    }
                    return Ok(Ty::I64);
                }
                // An interface value is its synthetic `{ptr data, ptr itable}`
                // fat-pointer layout (D-0016).
                if let Some(fat_sid) = env.ifaces.get(other) {
                    return Ok(Ty::Struct(*fat_sid));
                }
                match env.ids.get(other) {
                    Some(id) => Ok(Ty::Struct(*id)),
                    None => Err(err(format!(
                        "type '{other}' is not supported by codegen yet"
                    ))),
                }
            }
        },
        // A tuple type `(T, U, ...)` is an anonymous struct laid out positionally
        // (D-0049); it interns to a struct id reused by the struct machinery.
        ast::Type::Tuple { elems, .. } => {
            let tys = elems
                .iter()
                .map(|e| resolve_ty(e, env))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Ty::Struct(env.tuples.borrow_mut().intern(tys)))
        }
        _ => Err(err("only named scalar types are supported by codegen yet")),
    }
}

/// The runtime representation of a collection element/key/value given its
/// source type.
fn elem_of(t: &ast::Type, env: &TyEnv) -> Result<Elem, LowerError> {
    elem_kind(resolve_ty(t, env)?)
}

/// The element kind of a `heap[T]` element type (D-0057). A heap orders its
/// elements, so — unlike a general collection — only the *comparable* element
/// kinds are supported in this increment: integers/bools/chars (compared as
/// signed `i64`) and strings (compared by `ZapStr` bytes). User-struct elements
/// (ordered via a comparator / `__cmp__`) and floats are a clean follow-up.
fn heap_elem_of(t: &ast::Type, env: &TyEnv) -> Result<Elem, LowerError> {
    match resolve_ty(t, env)? {
        Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => Ok(Elem::I64),
        Ty::Ptr => Ok(Elem::Ptr),
        other => Err(err(format!(
            "heap[{other}] is not supported yet (only integer and str elements)"
        ))),
    }
}

/// The heap element kind named by a construction subscript like `heap[int64]`,
/// where the element type appears as a bare name in expression position.
fn heap_elem_from_name(name: &str) -> Result<Elem, LowerError> {
    match name {
        "int64" | "i64" | "int32" | "i32" | "bool" | "char" => Ok(Elem::I64),
        "str" => Ok(Elem::Ptr),
        other => Err(err(format!(
            "heap[{other}] is not supported yet (only integer and str elements)"
        ))),
    }
}

/// The array dtype for a `array[T]` element type (tickets #24, #49). Supported
/// dtypes: `f64`, `i64`, `uint8` (image data), and `bool` (masks). Other
/// numeric dtypes (`f32`, `i32`) are deferred.
fn dtype_of(t: &ast::Type, env: &TyEnv) -> Result<Dtype, LowerError> {
    match resolve_ty(t, env)? {
        Ty::F64 => Ok(Dtype::F64),
        Ty::I64 => Ok(Dtype::I64),
        Ty::I8 => Ok(Dtype::Uint8),
        Ty::I1 => Ok(Dtype::Bool),
        other => Err(err(format!(
            "array[{other}] is not supported yet (only array[f64], array[i64], array[uint8], array[bool])"
        ))),
    }
}

/// The array dtype named by a construction subscript like `array[f64]`, where
/// the dtype appears as a bare name in expression position.
fn dtype_from_name(name: &str) -> Result<Dtype, LowerError> {
    match name {
        "float64" | "f64" => Ok(Dtype::F64),
        "int64" | "i64" => Ok(Dtype::I64),
        "uint8" | "u8" => Ok(Dtype::Uint8),
        "bool" => Ok(Dtype::Bool),
        other => Err(err(format!(
            "array[{other}] is not supported yet (only array[f64], array[i64], array[uint8], array[bool])"
        ))),
    }
}

/// The channel element kind named by a construction subscript like
/// `channel[int64]`, where the element type appears as a bare name in
/// expression position (ticket #25, D-0031). Integers/bools become an `i64`
/// word; reference types (`str`, structs, collections) become a pointer word.
fn chan_elem_from_name(name: &str) -> Result<Elem, LowerError> {
    match name {
        "int64" | "i64" | "int32" | "i32" | "bool" | "char" => Ok(Elem::I64),
        "float64" | "f64" | "float32" | "f32" => Err(err(
            "channels of floating-point elements are not supported yet",
        )),
        // `str`, `bytes`, struct, and nested-collection elements are references.
        _ => Ok(Elem::Ptr),
    }
}

/// Classify a ZIR type into the element word kind collections store. Elements
/// are uniform 8-byte words (RUNTIME-ABI §"Collection layout"): integers/bools
/// go in as `i64`, reference types as pointers. Floats need a separate word
/// path and are deferred.
fn elem_kind(t: Ty) -> Result<Elem, LowerError> {
    match t {
        Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => Ok(Elem::I64),
        // A struct element carries its id so a struct *key* hashes/compares via
        // its own `__hash__`/`__eq__` (operator overloading, D-0047).
        Ty::Struct(sid) => Ok(Elem::Struct(sid)),
        Ty::Ptr
        | Ty::List(_)
        | Ty::Dict(..)
        | Ty::Set(_)
        | Ty::Heap(_)
        | Ty::Array(_)
        | Ty::Chan(_)
        | Ty::Match
        | Ty::Py => Ok(Elem::Ptr),
        Ty::F32 | Ty::F64 => Err(err(
            "collections of floating-point elements are not supported yet",
        )),
        Ty::Void => Err(err("a collection element cannot be void")),
    }
}

fn param_ty(p: &ast::Param, self_sid: Option<u32>, ctx: &StructCtx) -> Ty {
    if p.name == "self" {
        if let Some(sid) = self_sid {
            return Ty::Struct(sid);
        }
    }
    p.ty.as_ref()
        .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Ptr))
        .unwrap_or(Ty::Ptr)
}

fn err(m: impl Into<String>) -> LowerError {
    LowerError { message: m.into() }
}

// ----------------------------------------------------------------------------
// Decorators & annotations (ticket #16, D-0036)
//
// This is the item-level compile-time expansion of the built-in decorator set.
// The design doc locks `@cache`, `@derive(Eq, Ord, Json, Repr, Hash)`,
// `@default`, `@validate`, `@range`, `@deprecated`, and `@inject`, plus runtime
// reflection over annotations. This increment implements:
//   * `@derive(Eq)` / `@derive(Repr)` — generate a structural-equality method
//     and a debug-string method directly as ZIR (zero runtime cost).
//   * runtime reflection — struct-level annotations/decorators are recorded in a
//     registry at startup and queried with `reflect_has` / `reflect_names`.
// Everything else in the locked set is validated by sema and reported here as a
// clean "not implemented yet" lowering error (never silently ignored). The
// user-defined compile-time macro API stays deferred (OPEN-QUESTIONS.md).
// ----------------------------------------------------------------------------

/// The built-in behavioral decorators whose code generation is deferred. Sema
/// accepts them (they are part of the locked set); lowering reports them.
const DEFERRED_BUILTINS: &[&str] = &[
    "cache",
    "default",
    "validate",
    "range",
    "deprecated",
    "inject",
];

/// A trait requested via `@derive(...)`.
#[derive(Clone, Copy, PartialEq)]
enum DerivedTrait {
    Eq,
    Repr,
    Ord,
    Json,
    Csv,
    Yaml,
    Hash,
}

impl DerivedTrait {
    fn parse(name: &str) -> Option<DerivedTrait> {
        match name {
            "Eq" => Some(DerivedTrait::Eq),
            "Repr" => Some(DerivedTrait::Repr),
            "Ord" => Some(DerivedTrait::Ord),
            "Json" => Some(DerivedTrait::Json),
            "Csv" => Some(DerivedTrait::Csv),
            "Yaml" => Some(DerivedTrait::Yaml),
            "Hash" => Some(DerivedTrait::Hash),
            _ => None,
        }
    }

    /// The dunder method the generated code is reachable through and that the
    /// corresponding operator desugars to (D-0047): `Eq` → `__eq__` (`==`/`!=`),
    /// `Ord` → `__cmp__` (`< <= > >=`), `Hash` → `__hash__` (dict/set keys),
    /// `Repr` → `__repr__` (`print` / `str`).
    fn method_name(self) -> &'static str {
        match self {
            DerivedTrait::Eq => "__eq__",
            DerivedTrait::Repr => "__repr__",
            DerivedTrait::Ord => "__cmp__",
            DerivedTrait::Json => "to_json",
            DerivedTrait::Csv => "to_csv",
            DerivedTrait::Yaml => "to_yaml",
            DerivedTrait::Hash => "__hash__",
        }
    }

    /// The ZIR signature of the generated method, if this trait is implemented.
    /// `__cmp__` returns an `i64` three-way result (−1 / 0 / +1); `__hash__`
    /// returns the combined `i64` hash.
    fn signatures(self, sid: u32) -> Vec<(&'static str, Vec<Ty>, Ty)> {
        match self {
            DerivedTrait::Eq => vec![("__eq__", vec![Ty::Struct(sid), Ty::Struct(sid)], Ty::I1)],
            DerivedTrait::Repr => vec![("__repr__", vec![Ty::Struct(sid)], Ty::Ptr)],
            DerivedTrait::Ord => vec![("__cmp__", vec![Ty::Struct(sid), Ty::Struct(sid)], Ty::I64)],
            DerivedTrait::Hash => vec![("__hash__", vec![Ty::Struct(sid)], Ty::I64)],
            // Compile-time JSON codec (D-0059, I#100): the encode method
            // `to_json(self) -> str` and the decode associated function
            // `from_json(str) -> Self`.
            DerivedTrait::Json => vec![
                ("to_json", vec![Ty::Struct(sid)], Ty::Ptr),
                ("from_json", vec![Ty::Ptr], Ty::Struct(sid)),
            ],
            // Compile-time CSV record codec (D-0059 §4b, I#132): the encode
            // method `to_csv(self) -> str` and the decode associated function
            // `from_csv(str) -> Self`.
            DerivedTrait::Csv => vec![
                ("to_csv", vec![Ty::Struct(sid)], Ty::Ptr),
                ("from_csv", vec![Ty::Ptr], Ty::Struct(sid)),
            ],
            // Compile-time YAML codec (D-0063, I#149): the encode method
            // `to_yaml(self) -> str` and the decode associated function
            // `from_yaml(str) -> Self`. Reuses the shared DOM decode core.
            DerivedTrait::Yaml => vec![
                ("to_yaml", vec![Ty::Struct(sid)], Ty::Ptr),
                ("from_yaml", vec![Ty::Ptr], Ty::Struct(sid)),
            ],
        }
    }

    /// Generate the method body as a ZIR function, or `Ok(None)` if the trait
    /// produces no standalone function. Deferred traits return an error.
    fn generate<'a>(
        self,
        sid: u32,
        sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
        ctx: &'a StructCtx,
    ) -> Result<Vec<Func>, LowerError> {
        Ok(match self {
            DerivedTrait::Eq => vec![gen_eq_method(sid, sigs, ctx)?],
            DerivedTrait::Repr => vec![gen_repr_method(sid, sigs, ctx)?],
            DerivedTrait::Ord => vec![gen_cmp_method(sid, sigs, ctx)?],
            DerivedTrait::Hash => vec![gen_hash_method(sid, sigs, ctx)?],
            DerivedTrait::Json => vec![
                gen_json_to_method(sid, sigs, ctx)?,
                gen_json_from_method(sid, sigs, ctx)?,
                gen_json_from_value_method(sid, sigs, ctx)?,
            ],
            DerivedTrait::Csv => vec![
                gen_csv_to_method(sid, sigs, ctx)?,
                gen_csv_from_method(sid, sigs, ctx)?,
            ],
            // `from_yaml` parses to the shared `JsonValue` DOM and reuses the
            // same `from_json_value` decode core (generated here too, deduped
            // against `@derive(Json)` by name when a struct derives both).
            DerivedTrait::Yaml => vec![
                gen_yaml_to_method(sid, sigs, ctx)?,
                gen_yaml_from_method(sid, sigs, ctx)?,
                gen_json_from_value_method(sid, sigs, ctx)?,
            ],
        })
    }

    fn name(self) -> &'static str {
        match self {
            DerivedTrait::Eq => "Eq",
            DerivedTrait::Repr => "Repr",
            DerivedTrait::Ord => "Ord",
            DerivedTrait::Json => "Json",
            DerivedTrait::Csv => "Csv",
            DerivedTrait::Yaml => "Yaml",
            DerivedTrait::Hash => "Hash",
        }
    }
}

/// The result of scanning a module's decorators/annotations.
struct DecoratorPlan {
    /// Per struct (by id), the traits to derive.
    derived: Vec<(u32, Vec<DerivedTrait>)>,
    /// `(type_name, decorator_or_annotation_name)` pairs for runtime reflection.
    reflect_meta: Vec<(String, String)>,
    /// `@on_signal(Signal.X)` handlers (ticket #77): `(signal_number, fn_name)`
    /// pairs, registered at the top of `main` via `zap_signal_on`.
    on_signal_meta: Vec<(i64, String)>,
}

impl DecoratorPlan {
    fn collect(
        module: &ast::Module,
        ctx: &StructCtx,
        errors: &mut Vec<LowerError>,
    ) -> DecoratorPlan {
        // Names declared as metadata (annotations) or behavioral decorators.
        // These are what the reflection registry records; the built-in
        // compile-time decorators (`@derive` etc.) leave no runtime trace.
        let mut meta_names: HashSet<String> = HashSet::new();
        for item in &module.items {
            match item {
                ast::Item::Annotation(a) => {
                    meta_names.insert(a.name.clone());
                }
                ast::Item::Decorator(d) => {
                    meta_names.insert(d.name.clone());
                }
                _ => {}
            }
        }

        let mut derived: Vec<(u32, Vec<DerivedTrait>)> = Vec::new();
        let mut reflect_meta: Vec<(String, String)> = Vec::new();
        let mut on_signal_meta: Vec<(i64, String)> = Vec::new();

        for item in &module.items {
            match item {
                ast::Item::Struct(s) => {
                    let sid = ctx.ids[&s.name];
                    let mut traits: Vec<DerivedTrait> = Vec::new();
                    for d in &s.decorators {
                        if d.name == "derive" {
                            for a in &d.args {
                                if let ast::Expr::Name(t, _) = &a.value {
                                    match DerivedTrait::parse(t) {
                                        Some(tr) => {
                                            // A user method of the same name wins;
                                            // deriving it too would duplicate it.
                                            if s.methods.iter().any(|m| m.name == tr.method_name())
                                            {
                                                errors.push(err(format!(
                                                    "struct '{}' both defines method '{}' and \
                                                     derives {} (remove one)",
                                                    s.name,
                                                    tr.method_name(),
                                                    tr.name()
                                                )));
                                            } else if !traits.contains(&tr) {
                                                traits.push(tr);
                                            }
                                        }
                                        None => errors.push(err(format!(
                                            "unknown derivable trait '{t}' on struct '{}'",
                                            s.name
                                        ))),
                                    }
                                }
                            }
                        } else if DEFERRED_BUILTINS.contains(&d.name.as_str()) {
                            errors.push(err(format!(
                                "built-in decorator '@{}' is not implemented yet (deferred)",
                                d.name
                            )));
                        } else if meta_names.contains(&d.name) {
                            reflect_meta.push((s.name.clone(), d.name.clone()));
                        }
                        // Unknown names are already rejected by sema.
                    }
                    if !traits.is_empty() {
                        derived.push((sid, traits));
                    }
                    // Field / method decorators are deferred this increment.
                    for fld in &s.fields {
                        if !fld.decorators.is_empty() {
                            errors.push(err(format!(
                                "field decorators are not implemented yet (deferred): \
                                 '@{}' on field '{}'",
                                fld.decorators[0].name, fld.name
                            )));
                        }
                    }
                    for m in &s.methods {
                        defer_fn_decorators(m, errors);
                    }
                }
                ast::Item::Function(f) => {
                    // `@on_signal(Signal.X)` (ticket #77): register `f` as a
                    // handler for signal X; a detached receiver goroutine runs it
                    // on each delivery. Other function decorators remain deferred.
                    for d in &f.decorators {
                        if d.name == "on_signal" {
                            if !f.params.is_empty() {
                                errors.push(err(format!(
                                    "@on_signal handler '{}' must take no parameters",
                                    f.name
                                )));
                            }
                            match on_signal_number(d, ctx) {
                                Ok(signo) => on_signal_meta.push((signo, f.name.clone())),
                                Err(e) => errors.push(e),
                            }
                        } else {
                            errors.push(err(format!(
                                "function decorators are not implemented yet (deferred): \
                                 '@{}' on '{}'",
                                d.name, f.name
                            )));
                        }
                    }
                    for p in &f.params {
                        if !p.decorators.is_empty() {
                            errors.push(err(format!(
                                "parameter decorators are not implemented yet (deferred): \
                                 '@{}' on '{}'",
                                p.decorators[0].name, p.name
                            )));
                        }
                    }
                }
                ast::Item::Enum(e) if !e.decorators.is_empty() => {
                    errors.push(err(format!(
                        "enum decorators are not implemented yet (deferred): '@{}'",
                        e.decorators[0].name
                    )));
                }
                _ => {}
            }
        }

        DecoratorPlan {
            derived,
            reflect_meta,
            on_signal_meta,
        }
    }
}

/// Resolve the single `Signal.X` argument of an `@on_signal(...)` decorator to
/// its OS signal number (ticket #77). The argument must be a `Signal` enum
/// member (a tier-1 discriminant that IS the signal number). Sema has already
/// validated the shape; this mirrors it for lowering and errors defensively.
fn on_signal_number(d: &ast::Decorator, ctx: &StructCtx) -> Result<i64, LowerError> {
    if d.args.len() != 1 {
        return Err(err(
            "@on_signal(Signal.X) takes exactly one Signal argument".to_string(),
        ));
    }
    if let Expr::Attr { base, name, .. } = &d.args[0].value {
        if let Expr::Name(tyname, _) = base.as_ref() {
            if tyname == "Signal" {
                if let Some(v) = ctx.enums.get("Signal").and_then(|e| e.member_value(name)) {
                    return Ok(v);
                }
            }
        }
    }
    Err(err(
        "@on_signal expects a Signal member, e.g. @on_signal(Signal.SIGINT)".to_string(),
    ))
}

/// Report function/parameter decorators as deferred (not implemented yet).
fn defer_fn_decorators(f: &ast::Function, errors: &mut Vec<LowerError>) {
    if !f.decorators.is_empty() {
        errors.push(err(format!(
            "function decorators are not implemented yet (deferred): '@{}' on '{}'",
            f.decorators[0].name, f.name
        )));
    }
    for p in &f.params {
        if !p.decorators.is_empty() {
            errors.push(err(format!(
                "parameter decorators are not implemented yet (deferred): '@{}' on '{}'",
                p.decorators[0].name, p.name
            )));
        }
    }
}

/// `@derive(Eq)` — a structural-equality method `Struct____eq__(self, other) ->
/// bool` (the `__eq__` dunder, D-0047) that compares every field (ints/bools by
/// value, floats by ordered compare, strings by content) and ANDs the results.
fn gen_eq_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}__{}", ctx.names[sid as usize], "__eq__");
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(sid), Ty::Struct(sid)],
        Ty::I1,
        false,
        true, // self_param: a derived method's receiver is by-reference
        sigs,
        ctx,
    );
    // Params: value 0 = self, value 1 = other (used directly as struct bases).
    let mut acc = b.iconst(Ty::I1, 1);
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let idx = idx as u32;
        let a = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: a,
            base: 0,
            sid,
            idx,
        });
        let c = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: c,
            base: 1,
            sid,
            idx,
        });
        let bit = gen_field_eq(&mut b, a, c, *fty)?;
        let next = b.new_value(Ty::I1);
        b.emit(Inst::Bin {
            dest: next,
            op: BinOp::And,
            lhs: acc,
            rhs: bit,
        });
        acc = next;
    }
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// Compare one field of a derived `eq`, returning an `i1`.
fn gen_field_eq(b: &mut FnLower, a: Value, c: Value, fty: Ty) -> Result<Value, LowerError> {
    let bit = b.new_value(Ty::I1);
    match fty {
        Ty::Ptr => {
            // Strings: content equality via the runtime (returns 1/0 as i64).
            let word = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(word),
                func: "zap_str_eq".to_string(),
                args: vec![a, c],
            });
            let zero = b.iconst(Ty::I64, 0);
            b.emit(Inst::ICmp {
                dest: bit,
                pred: CmpPred::Ne,
                lhs: word,
                rhs: zero,
            });
        }
        Ty::F32 | Ty::F64 => {
            b.emit(Inst::FCmp {
                dest: bit,
                pred: CmpPred::Eq,
                lhs: a,
                rhs: c,
            });
        }
        t if t.int_width().is_some() => {
            b.emit(Inst::ICmp {
                dest: bit,
                pred: CmpPred::Eq,
                lhs: a,
                rhs: c,
            });
        }
        other => {
            return Err(err(format!(
                "@derive(Eq) does not support field type {other} yet \
                 (only scalars and str)"
            )));
        }
    }
    Ok(bit)
}

/// `@derive(Repr)` — a debug-string method `Struct____repr__(self) -> str` (the
/// `__repr__` dunder, D-0047) producing `Name(f0=v0, f1=v1, ...)`.
fn gen_repr_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__{}", "__repr__");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    let mut acc = b.strconst(&format!("{sname}("));
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        if idx > 0 {
            let sep = b.strconst(", ");
            acc = gen_concat(&mut b, acc, sep);
        }
        let label = b.strconst(&format!("{fname}="));
        acc = gen_concat(&mut b, acc, label);
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        let vstr = match *fty {
            Ty::Ptr => fv,
            t if t.int_width().is_some() => {
                let i64v = b.coerce(fv, t, Ty::I64);
                let s = b.new_value(Ty::Ptr);
                b.emit(Inst::Call {
                    dest: Some(s),
                    func: "zap_i64_to_str".to_string(),
                    args: vec![i64v],
                });
                s
            }
            other => {
                return Err(err(format!(
                    "@derive(Repr) does not support field type {other} yet \
                     (only integers, bools, and str)"
                )));
            }
        };
        acc = gen_concat(&mut b, acc, vstr);
    }
    let close = b.strconst(")");
    acc = gen_concat(&mut b, acc, close);
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// `zap_str_concat(a, b)` as a ZIR call returning the new string.
fn gen_concat(b: &mut FnLower, a: Value, c: Value) -> Value {
    let d = b.new_value(Ty::Ptr);
    b.emit(Inst::Call {
        dest: Some(d),
        func: "zap_str_concat".to_string(),
        args: vec![a, c],
    });
    d
}

/// Format-neutral classification of a derived struct field's type — the shared
/// "derive-walk core" (D-0059 §4b). The `@derive(Json)` encode/decode codegen
/// dispatches on these kinds, and `@derive(Csv)` (PR3) reuses the same
/// classifier, each mapping a kind to its own wire form. A field type with no
/// kind is not yet supported by a derived codec.
enum DeriveFieldKind {
    /// A boolean (`i1`). Classified before `Int` (an `i1` has an integer width).
    Bool,
    /// A float field; carries the exact width (`F32`/`F64`).
    Float(Ty),
    /// An integer field; carries the exact width.
    Int(Ty),
    /// A `str` (reference string, lowered to `Ptr`).
    Str,
    /// A nested struct field; carries its struct id (must itself derive the
    /// codec).
    Nested(u32),
}

/// Classify a derived field's type for the shared codec walk, or `None` if no
/// derived codec supports it yet (collections, enums, tuples, …).
fn derive_field_kind(fty: Ty) -> Option<DeriveFieldKind> {
    match fty {
        Ty::I1 => Some(DeriveFieldKind::Bool),
        Ty::F32 | Ty::F64 => Some(DeriveFieldKind::Float(fty)),
        t if t.int_width().is_some() => Some(DeriveFieldKind::Int(t)),
        Ty::Ptr => Some(DeriveFieldKind::Str),
        Ty::Struct(s) => Some(DeriveFieldKind::Nested(s)),
        _ => None,
    }
}

/// `@derive(Json)` encode direction (D-0059, I#100): a `Struct__to_json(self)
/// -> str` method producing the struct's compact JSON object,
/// `{"f0":v0,"f1":v1,...}`. Field names are identifiers (no escaping needed);
/// values are encoded per field type. Reuses the DOM runtime helpers
/// (`zap_json_escape` for strings, `zap_i64_to_str`/`zap_f64_to_str` for
/// numbers, `zap_json_bool_str` for booleans); a nested struct field recurses
/// through its own derived `to_json` (so nested records must also
/// `@derive(Json)`). Collections / enum fields are a follow-up slice. This is
/// the string-building encoder; the `__to_json_value__` DOM bridge and the
/// `from_json` decode direction land with the shared derive-walk core.
fn gen_json_to_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__to_json");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    let mut acc = b.strconst("{");
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        if idx > 0 {
            let sep = b.strconst(",");
            acc = gen_concat(&mut b, acc, sep);
        }
        // A field name is a Zap identifier, so it needs no JSON escaping.
        let label = b.strconst(&format!("\"{fname}\":"));
        acc = gen_concat(&mut b, acc, label);
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        let vstr = gen_json_encode_field(&mut b, fv, *fty, ctx)?;
        acc = gen_concat(&mut b, acc, vstr);
    }
    let close = b.strconst("}");
    acc = gen_concat(&mut b, acc, close);
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// Encode one `@derive(Json)` field value to its JSON text.
fn gen_json_encode_field(
    b: &mut FnLower,
    fv: Value,
    fty: Ty,
    ctx: &StructCtx,
) -> Result<Value, LowerError> {
    let call1 = |b: &mut FnLower, func: &str, arg: Value| -> Value {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => {
            let w = b.coerce(fv, Ty::I1, Ty::I64);
            call1(b, "zap_json_bool_str", w)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.coerce(fv, w, Ty::F64);
            call1(b, "zap_f64_to_str", f)
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.coerce(fv, w, Ty::I64);
            call1(b, "zap_i64_to_str", i)
        }
        // A `str` field — escape + quote.
        Some(DeriveFieldKind::Str) => call1(b, "zap_json_escape", fv),
        // A nested struct field — recurse through its own derived encoder.
        Some(DeriveFieldKind::Nested(fsid)) => {
            let fname = ctx.names[fsid as usize].clone();
            call1(b, &format!("{fname}__to_json"), fv)
        }
        None => {
            return Err(err(format!(
                "@derive(Json) does not support field type {fty} yet \
                 (scalars, str, and nested @derive(Json) structs are; \
                 collections and enums are a follow-up)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Json)` decode direction (D-0059, I#100): a `Struct__from_json(s:
/// str) -> Struct` associated function: parse `s` to the DOM (`__json_loads`),
/// then hand off to the recursive `Struct__from_json_value` core.
fn gen_json_from_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_json");
    let jv_sid = json_value_sid(ctx)?;
    // One parameter: the source string (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let jv = b.new_value(Ty::Struct(jv_sid));
    b.emit(Inst::Call {
        dest: Some(jv),
        func: "__json_loads".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::Call {
        dest: Some(dest),
        func: format!("{sname}__from_json_value"),
        args: vec![jv],
    });
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// `@derive(Json)` decode core (D-0059, I#100): `Struct__from_json_value(v:
/// JsonValue) -> Struct`, populating each field from the object by name via the
/// prelude typed accessors (`__json_field` / `__json_as_*`). A nested struct
/// field recurses through the nested type's own `from_json_value`, so nested
/// records decode (they must also `@derive(Json)`). Strict typing is lenient for
/// v1 (a missing/mismatched field yields the accessor's default); strict-error
/// semantics are a follow-up.
fn gen_json_from_value_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_json_value");
    let jv_sid = json_value_sid(ctx)?;
    // One parameter: the object JsonValue (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(jv_sid)],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::StructNew { dest, sid });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        // jf = __json_field(v, "fname")
        let key = b.strconst(fname);
        let jf = b.new_value(Ty::Struct(jv_sid));
        b.emit(Inst::Call {
            dest: Some(jf),
            func: "__json_field".to_string(),
            args: vec![0, key],
        });
        let val = gen_json_decode_field(&mut b, jf, *fty, ctx)?;
        b.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// The prelude `JsonValue` enum's struct id (its tagged-struct layout).
fn json_value_sid(ctx: &StructCtx) -> Result<u32, LowerError> {
    ctx.ids
        .get("JsonValue")
        .copied()
        .ok_or_else(|| err("JsonValue unavailable (built-in prelude missing)"))
}

/// Decode one field: convert its `JsonValue` (`jf`) to the field type `fty` via
/// the prelude typed accessors, recursing into a nested struct's
/// `from_json_value`.
fn gen_json_decode_field(
    b: &mut FnLower,
    jf: Value,
    fty: Ty,
    ctx: &StructCtx,
) -> Result<Value, LowerError> {
    let call1 = |b: &mut FnLower, func: &str, ret: Ty, arg: Value| -> Value {
        let d = b.new_value(ret);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => call1(b, "__json_as_bool", Ty::I1, jf),
        Some(DeriveFieldKind::Float(Ty::F32)) => {
            let f = call1(b, "__json_as_float", Ty::F64, jf);
            b.coerce(f, Ty::F64, Ty::F32)
        }
        Some(DeriveFieldKind::Float(_)) => call1(b, "__json_as_float", Ty::F64, jf),
        Some(DeriveFieldKind::Int(Ty::I64)) => call1(b, "__json_as_int", Ty::I64, jf),
        Some(DeriveFieldKind::Int(w)) => {
            let i = call1(b, "__json_as_int", Ty::I64, jf);
            b.coerce(i, Ty::I64, w)
        }
        Some(DeriveFieldKind::Str) => call1(b, "__json_as_str", Ty::Ptr, jf),
        // A nested struct field — recurse through its own decode core.
        Some(DeriveFieldKind::Nested(fsid)) => {
            let fname = ctx.names[fsid as usize].clone();
            let d = b.new_value(Ty::Struct(fsid));
            b.emit(Inst::Call {
                dest: Some(d),
                func: format!("{fname}__from_json_value"),
                args: vec![jf],
            });
            d
        }
        None => {
            return Err(err(format!(
                "@derive(Json) from_json does not support field type {fty} yet \
                 (scalars, str, and nested @derive(Json) structs are; \
                 collections and enums are a follow-up)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Yaml)` encode direction (D-0063, I#149): a `Struct__to_yaml(self)
/// -> str` method producing a YAML block mapping, one `fname: value` line per
/// field (declaration order). Scalar values reuse the json inline formatters
/// (strings JSON-escaped+quoted, which is valid YAML); a nested struct field
/// recurses through its own derived `to_yaml`, re-indented by two spaces via
/// `zap_yaml_indent`, so nested records must also `@derive(Yaml)`.
fn gen_yaml_to_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__to_yaml");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    let mut acc = b.strconst("");
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        if let Some(DeriveFieldKind::Nested(fsid)) = derive_field_kind(*fty) {
            // `fname:\n` then the child's YAML, indented two spaces.
            let label = b.strconst(&format!("{fname}:\n"));
            acc = gen_concat(&mut b, acc, label);
            let child_name = ctx.names[fsid as usize].clone();
            let child = b.new_value(Ty::Ptr);
            b.emit(Inst::Call {
                dest: Some(child),
                func: format!("{child_name}__to_yaml"),
                args: vec![fv],
            });
            let pad = b.strconst("  ");
            let indented = b.new_value(Ty::Ptr);
            b.emit(Inst::Call {
                dest: Some(indented),
                func: "zap_yaml_indent".to_string(),
                args: vec![child, pad],
            });
            acc = gen_concat(&mut b, acc, indented);
        } else {
            // `fname: <scalar>\n`
            let label = b.strconst(&format!("{fname}: "));
            acc = gen_concat(&mut b, acc, label);
            let vstr = gen_yaml_encode_scalar(&mut b, fv, *fty)?;
            acc = gen_concat(&mut b, acc, vstr);
            let nl = b.strconst("\n");
            acc = gen_concat(&mut b, acc, nl);
        }
    }
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// Encode one scalar `@derive(Yaml)` field value to its inline YAML text
/// (reusing the json formatters; JSON scalars are valid YAML). Nested structs
/// are handled by the caller; other field types are rejected.
fn gen_yaml_encode_scalar(b: &mut FnLower, fv: Value, fty: Ty) -> Result<Value, LowerError> {
    let call1 = |b: &mut FnLower, func: &str, arg: Value| -> Value {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => {
            let w = b.coerce(fv, Ty::I1, Ty::I64);
            call1(b, "zap_json_bool_str", w)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.coerce(fv, w, Ty::F64);
            call1(b, "zap_f64_to_str", f)
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.coerce(fv, w, Ty::I64);
            call1(b, "zap_i64_to_str", i)
        }
        Some(DeriveFieldKind::Str) => call1(b, "zap_json_escape", fv),
        Some(DeriveFieldKind::Nested(_)) | None => {
            return Err(err(format!(
                "@derive(Yaml) does not support field type {fty} yet \
                 (scalars, str, and nested @derive(Yaml) structs are; \
                 collections and enums are a follow-up)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Yaml)` decode direction (D-0063, I#149): a `Struct__from_yaml(s:
/// str) -> Struct` associated function. YAML 1.2 is a JSON superset, so this
/// parses `s` to the shared `JsonValue` DOM (`__yaml_loads`, which transpiles
/// the YAML subset to JSON and reuses `__json_loads`), then hands off to the
/// same recursive `Struct__from_json_value` decode core as `@derive(Json)`.
fn gen_yaml_from_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_yaml");
    let jv_sid = json_value_sid(ctx)?;
    // One parameter: the source string (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let jv = b.new_value(Ty::Struct(jv_sid));
    b.emit(Inst::Call {
        dest: Some(jv),
        func: "__yaml_loads".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::Call {
        dest: Some(dest),
        func: format!("{sname}__from_json_value"),
        args: vec![jv],
    });
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// `@derive(Csv)` encode (D-0059 §4b, I#132): `Struct__to_csv(self) -> str`
/// producing one CSV record — each field formatted and CSV-quoted as needed,
/// joined with commas (declaration order = column order). Reuses the shared
/// `DeriveFieldKind` walk; nested structs / collections do not map to a flat
/// record and are rejected.
fn gen_csv_to_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__to_csv");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    // parts = [] (list of column strings)
    let parts = b.new_value(Ty::List(Elem::Ptr));
    b.emit(Inst::Call {
        dest: Some(parts),
        func: "zap_list_new".to_string(),
        args: vec![],
    });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        let col = gen_csv_encode_field(&mut b, fv, *fty)?;
        b.emit(Inst::Call {
            dest: None,
            func: "zap_list_push_ptr".to_string(),
            args: vec![parts, col],
        });
    }
    let d = b.new_value(Ty::Ptr);
    b.emit(Inst::Call {
        dest: Some(d),
        func: "zap_csv_join".to_string(),
        args: vec![parts],
    });
    b.set_term(Term::Ret(Some(d)));
    Ok(b.func)
}

/// Encode one `@derive(Csv)` field to its CSV column text.
fn gen_csv_encode_field(b: &mut FnLower, fv: Value, fty: Ty) -> Result<Value, LowerError> {
    let call1 = |b: &mut FnLower, func: &str, arg: Value| -> Value {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    let r = match derive_field_kind(fty) {
        // Python csv convention: booleans render as True / False.
        Some(DeriveFieldKind::Bool) => {
            let w = b.coerce(fv, Ty::I1, Ty::I64);
            call1(b, "zap_bool_to_str", w)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.coerce(fv, w, Ty::F64);
            call1(b, "zap_f64_to_str", f)
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.coerce(fv, w, Ty::I64);
            call1(b, "zap_i64_to_str", i)
        }
        Some(DeriveFieldKind::Str) => call1(b, "zap_csv_quote", fv),
        Some(DeriveFieldKind::Nested(_)) | None => {
            return Err(err(format!(
                "@derive(Csv) does not support field type {fty} \
                 (a CSV record is flat: scalars and str only; nested structs, \
                 collections, and enums do not map to columns)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Csv)` decode (D-0059 §4b, I#132): `Struct__from_csv(s: str) ->
/// Struct`, splitting one CSV record into columns and parsing each to its
/// field's scalar type. Lenient for v1 (unparseable column → 0 / 0.0 / false).
fn gen_csv_from_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_csv");
    // One parameter: the record line (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    // cols = zap_csv_split(line)
    let cols = b.new_value(Ty::List(Elem::Ptr));
    b.emit(Inst::Call {
        dest: Some(cols),
        func: "zap_csv_split".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::StructNew { dest, sid });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let iidx = b.iconst(Ty::I64, idx as i64);
        let col = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(col),
            func: "zap_list_get_ptr".to_string(),
            args: vec![cols, iidx],
        });
        let val = gen_csv_decode_field(&mut b, col, *fty)?;
        b.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// Decode one `@derive(Csv)` column string `col` to the field type `fty`.
fn gen_csv_decode_field(b: &mut FnLower, col: Value, fty: Ty) -> Result<Value, LowerError> {
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => {
            let w = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(w),
                func: "zap_str_to_bool".to_string(),
                args: vec![col],
            });
            b.coerce(w, Ty::I64, Ty::I1)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.new_value(Ty::F64);
            b.emit(Inst::Call {
                dest: Some(f),
                func: "zap_str_to_f64".to_string(),
                args: vec![col],
            });
            if w == Ty::F64 {
                f
            } else {
                b.coerce(f, Ty::F64, w)
            }
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(i),
                func: "zap_str_to_i64".to_string(),
                args: vec![col],
            });
            if w == Ty::I64 {
                i
            } else {
                b.coerce(i, Ty::I64, w)
            }
        }
        // The column is already the string value.
        Some(DeriveFieldKind::Str) => col,
        Some(DeriveFieldKind::Nested(_)) | None => {
            return Err(err(format!(
                "@derive(Csv) does not support field type {fty} \
                 (a CSV record is flat: scalars and str only; nested structs, \
                 collections, and enums do not map to columns)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Ord)` — a three-way comparison `Struct____cmp__(self, other) -> i64`
/// (the `__cmp__` dunder, D-0047) returning −1 / 0 / +1 lexicographically by
/// field. The four relational operators (`< <= > >=`) derive from it. Fields are
/// compared in declaration order; the first non-equal field decides the result.
fn gen_cmp_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}__{}", ctx.names[sid as usize], "__cmp__");
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(sid), Ty::Struct(sid)],
        Ty::I64,
        false,
        true, // self_param: a derived method's receiver is by-reference
        sigs,
        ctx,
    );
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let idx = idx as u32;
        let a = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: a,
            base: 0,
            sid,
            idx,
        });
        let c = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: c,
            base: 1,
            sid,
            idx,
        });
        let cmp = gen_field_cmp(&mut b, a, c, *fty)?;
        // `if cmp != 0: return cmp` — otherwise fall through to the next field.
        let zero = b.iconst(Ty::I64, 0);
        let ne = b.new_value(Ty::I1);
        b.emit(Inst::ICmp {
            dest: ne,
            pred: CmpPred::Ne,
            lhs: cmp,
            rhs: zero,
        });
        let ret_bb = b.new_block();
        let next_bb = b.new_block();
        b.set_term(Term::BrIf {
            cond: ne,
            then_bb: ret_bb,
            else_bb: next_bb,
        });
        b.position(ret_bb);
        b.set_term(Term::Ret(Some(cmp)));
        b.position(next_bb);
    }
    // All fields equal.
    let zero = b.iconst(Ty::I64, 0);
    b.set_term(Term::Ret(Some(zero)));
    Ok(b.func)
}

/// Three-way compare one field, returning an `i64` of −1 / 0 / +1.
fn gen_field_cmp(b: &mut FnLower, a: Value, c: Value, fty: Ty) -> Result<Value, LowerError> {
    match fty {
        Ty::Ptr => {
            // Strings: lexicographic byte comparison via the runtime.
            let d = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(d),
                func: "zap_str_cmp".to_string(),
                args: vec![a, c],
            });
            Ok(d)
        }
        Ty::F32 | Ty::F64 => {
            let gt = b.new_value(Ty::I1);
            b.emit(Inst::FCmp {
                dest: gt,
                pred: CmpPred::Gt,
                lhs: a,
                rhs: c,
            });
            let lt = b.new_value(Ty::I1);
            b.emit(Inst::FCmp {
                dest: lt,
                pred: CmpPred::Lt,
                lhs: a,
                rhs: c,
            });
            Ok(gen_three_way(b, gt, lt))
        }
        t if t.int_width().is_some() => {
            // Compare at i64 so mixed-width fields share a type (D-0019).
            let ai = b.coerce(a, t, Ty::I64);
            let ci = b.coerce(c, t, Ty::I64);
            let gt = b.new_value(Ty::I1);
            b.emit(Inst::ICmp {
                dest: gt,
                pred: CmpPred::Gt,
                lhs: ai,
                rhs: ci,
            });
            let lt = b.new_value(Ty::I1);
            b.emit(Inst::ICmp {
                dest: lt,
                pred: CmpPred::Lt,
                lhs: ai,
                rhs: ci,
            });
            Ok(gen_three_way(b, gt, lt))
        }
        other => Err(err(format!(
            "@derive(Ord) does not support field type {other} yet \
             (only scalars and str)"
        ))),
    }
}

/// Fold a `greater-than` and `less-than` bit into an `i64` of +1 / −1 / 0
/// without integer extension pitfalls (an `i1` sign-extends to −1).
fn gen_three_way(b: &mut FnLower, gt: Value, lt: Value) -> Value {
    let neg1 = b.iconst(Ty::I64, -1);
    let zero = b.iconst(Ty::I64, 0);
    let one = b.iconst(Ty::I64, 1);
    let tmp = b.new_value(Ty::I64);
    b.emit(Inst::Select {
        dest: tmp,
        cond: lt,
        a: neg1,
        b: zero,
    });
    let d = b.new_value(Ty::I64);
    b.emit(Inst::Select {
        dest: d,
        cond: gt,
        a: one,
        b: tmp,
    });
    d
}

/// `@derive(Hash)` — a hash method `Struct____hash__(self) -> i64` (the
/// `__hash__` dunder, D-0047) combining every field's hash. The same fields feed
/// `__eq__` and `__hash__`, so the invariant `a == b ⟹ hash(a) == hash(b)` holds
/// (D-0047). Float fields are a documented follow-up (bit-hashing would break the
/// invariant against `__eq__`'s value compare), so they are rejected cleanly.
fn gen_hash_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}__{}", ctx.names[sid as usize], "__hash__");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::I64, false, true, sigs, ctx);
    // Seed; each field hash is mixed in with `zap_hash_combine`.
    let mut acc = b.iconst(Ty::I64, 0);
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        let h = match *fty {
            Ty::Ptr => {
                let d = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_hash".to_string(),
                    args: vec![fv],
                });
                d
            }
            t if t.int_width().is_some() => {
                let i = b.coerce(fv, t, Ty::I64);
                let d = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_hash".to_string(),
                    args: vec![i],
                });
                d
            }
            other => {
                return Err(err(format!(
                    "@derive(Hash) does not support field type {other} yet \
                     (only integers, bools, and str)"
                )));
            }
        };
        let next = b.new_value(Ty::I64);
        b.emit(Inst::Call {
            dest: Some(next),
            func: "zap_hash_combine".to_string(),
            args: vec![acc, h],
        });
        acc = next;
    }
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

fn lower_fn(
    f: &ast::Function,
    zir_name: String,
    self_sid: Option<u32>,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
    reflect_meta: &[(String, String)],
    on_signal_meta: &[(i64, String)],
) -> Result<Func, LowerError> {
    let (params, ret) = sigs.get(&zir_name).cloned().unwrap();
    let is_main = zir_name == "main";
    let mut b = FnLower::new(
        zir_name,
        params.clone(),
        ret,
        is_main,
        self_sid.is_some(),
        sigs,
        ctx,
    );

    // Parameters: values 0..n are the incoming args; copy each into a slot.
    for (i, p) in f.params.iter().enumerate() {
        let pty = params[i];
        let slot = b.alloca(pty);
        b.store(slot, i as Value);
        b.declare(&p.name, slot, pty);
    }

    if is_main {
        // Reflection bootstrap (D-0036): populate the runtime metadata registry
        // once, at the top of `main`, so `reflect_has`/`reflect_names` queries
        // anywhere in the program see every annotated/decorated type. These are
        // plain (non-throwing) runtime calls emitted before the user body.
        b.emit_reflect_registrations(reflect_meta);
        // Signal subsystem bootstrap (ticket #77, D-0058): start the dedicated
        // signal thread + self-pipe and install the default diagnostic handlers
        // (SIGQUIT -> stacks/runtime dump, SIGUSR1 -> GC + mem stats), unless the
        // runtime is built -DZAP_NO_DEFAULT_SIGNALS. A no-op there; otherwise the
        // handlers are live for the whole program (user notify/@on_signal wins).
        b.emit(Inst::Call {
            dest: None,
            func: "zap_signal_init".to_string(),
            args: vec![],
        });
        // `@on_signal(Signal.X)` handler registrations (ticket #77): each spawns
        // a detached receiver goroutine over `signal.notify(X)` that invokes the
        // decorated function on every delivery. Emitted at the top of `main`.
        b.emit_on_signal_registrations(on_signal_meta);
        // `main` installs an implicit top-level landing pad so an exception that
        // propagates out of the program is caught and reported (message + exit 1)
        // rather than ending in a raw abort (RUNTIME-ABI §"Exception / unwinding
        // ABI": "unwinds to the top frame and aborts"). Every call in `main`
        // therefore `invoke`s to this pad.
        let lp = b.new_block();
        b.landing_pads.push(lp);
        for st in &f.body.stmts {
            b.stmt(st)?;
        }
        b.landing_pads.pop();
        if !b.terminated() {
            b.set_term(Term::Ret(None));
        }
        if b.used_lps.contains(&lp) {
            b.position(lp);
            let exc = b.new_value(Ty::Ptr);
            b.emit(Inst::LandingPad { dest: exc });
            b.emit(Inst::Call {
                dest: None,
                func: "zap_uncaught".to_string(),
                args: vec![exc],
            });
            b.set_term(Term::Unreachable);
        }
    } else {
        for st in &f.body.stmts {
            b.stmt(st)?;
        }
        if !b.terminated() {
            if ret == Ty::Void {
                b.set_term(Term::Ret(None));
            } else {
                b.set_term(Term::Unreachable);
            }
        }
    }
    Ok(b.func)
}

/// Discover a generator's live locals and element type `T` (I#67), by lowering
/// its body once in **discovery** mode and throwing the result away. Returns the
/// body's `let`-bound locals as `(name, type)` in first-seen order (the state
/// struct's local fields) and the element type (the join of the `yield` operand
/// types). The generator's parameters are handled by the caller (they are the
/// leading state-struct fields).
fn discover_generator(
    f: &ast::Function,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<(Vec<(String, Ty)>, Ty), LowerError> {
    let (params, _ret) = sigs.get(&f.name).cloned().unwrap();
    let mut b = FnLower::new(
        f.name.clone(),
        params.clone(),
        Ty::Void,
        false,
        false,
        sigs,
        ctx,
    );
    b.gen_discover = Some(Vec::new());
    for (i, p) in f.params.iter().enumerate() {
        let pty = params[i];
        let slot = b.alloca(pty);
        b.store(slot, i as Value);
        b.declare(&p.name, slot, pty);
    }
    for st in &f.body.stmts {
        b.stmt(st)?;
    }
    let locals = b.gen_discover.take().unwrap();
    let elem = join_yield_tys(&b.gen_elems)?;
    Ok((locals, elem))
}

/// The element type `T` of a generator: every `yield` must produce the same ZIR
/// type in v1 (I#67). Detection guarantees at least one `yield`.
fn join_yield_tys(tys: &[Ty]) -> Result<Ty, LowerError> {
    let mut it = tys.iter().copied();
    let first = it
        .next()
        .ok_or_else(|| err("internal: generator has no 'yield'"))?;
    for t in it {
        if t != first {
            return Err(err(
                "a generator must yield values of a single type in this version (I#67 v1)",
            ));
        }
    }
    Ok(first)
}

/// Emit a generator's `__iter__` (I#67, D-0048): an iterator is its own iterable,
/// so it returns `self`.
fn lower_generator_iter(
    gi: &GeneratorInfo,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Func {
    let sname = zapc_syntax::genanalysis::gen_state_name(&gi.name);
    let mangled = format!("{sname}____iter__");
    let mut b = FnLower::new(
        mangled,
        vec![Ty::Struct(gi.sid)],
        Ty::Struct(gi.sid),
        false,
        true,
        sigs,
        ctx,
    );
    b.set_term(Term::Ret(Some(0))); // `self` is value 0
    b.func
}

/// Emit a generator's comma-ok `__next__` (I#67, D-0048) as a state machine.
///
/// Shape (ZIR is a flat CFG with no `switch`, so dispatch is a `BrIf` ladder):
/// ```text
/// entry:   <hoisted allocas>                 ; one per live local / parameter
///          slot_i = FieldGet self, field_i   ; load persisted values
///          state  = FieldGet self, 0
///          if state == 0 -> body_start
///          if state == 1 -> resume_1
///          ...
///          else          -> done
/// body:    ... run to the first `yield` ...
/// resume_k:... continue after the k-th `yield` ...
/// done:    FieldSet self, 0, -1              ; latch exhausted
///          return (zeroed, false)
/// ```
/// Each `yield v` (lowered in [`FnLower::lower_yield_stmt`]) stores every live
/// slot back to its field, sets `$state`, and returns `(v, true)` — so the next
/// call's entry dispatch resumes just past it.
fn lower_generator_next(
    f: &ast::Function,
    gi: &GeneratorInfo,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = zapc_syntax::genanalysis::gen_state_name(&gi.name);
    let mangled = format!("{sname}____next__");
    let ret = Ty::Struct(gi.next_tuple_sid);
    let mut b = FnLower::new(
        mangled,
        vec![Ty::Struct(gi.sid)],
        ret,
        false,
        true,
        sigs,
        ctx,
    );
    let entry = 0; // block 0, created by `FnLower::new`
    let done = b.new_block();
    let body_start = b.new_block();

    let mut field_idx = HashMap::new();
    for (p, (name, _ty)) in gi.vars.iter().enumerate() {
        field_idx.insert(name.clone(), gen_var_field(p));
    }
    b.gen_entry = Some(GenEntry {
        self_val: 0,
        sid: gi.sid,
        entry_block: entry,
        done_block: done,
        next_tuple_sid: gi.next_tuple_sid,
        elem: gi.elem,
        field_idx,
        slots: HashMap::new(),
        order: Vec::new(),
        next_state: 1,
        resume: Vec::new(),
    });

    // Pre-create the per-call slots for the parameters (declared in the root
    // scope) so the body can reference them and they are field-loaded at entry.
    for p in 0..gi.n_params {
        let (name, ty) = gi.vars[p].clone();
        b.gen_entry_slot(&name, ty);
    }

    // Lower the body; `yield`s split it into resume blocks and `let`s create
    // their hoisted slots lazily (see `decl_slot`).
    b.position(body_start);
    for st in &f.body.stmts {
        b.stmt(st)?;
    }
    // Falling off the end exhausts the generator.
    b.ensure_br(done);

    // The exhaustion block: latch `$state = -1` and return `(zeroed, false)`.
    b.position(done);
    let neg1 = b.iconst(Ty::I64, -1);
    b.emit(Inst::FieldSet {
        base: 0,
        sid: gi.sid,
        idx: 0,
        val: neg1,
    });
    let tup = b.new_value(Ty::Struct(gi.next_tuple_sid));
    b.emit(Inst::StructNew {
        dest: tup,
        sid: gi.next_tuple_sid,
    });
    let no = b.bconst(false);
    b.emit(Inst::FieldSet {
        base: tup,
        sid: gi.next_tuple_sid,
        idx: 1,
        val: no,
    });
    b.set_term(Term::Ret(Some(tup)));

    // Fill the entry block: load every field into its slot, load `$state`, and
    // branch to the start / the right resume point / `done`.
    b.position(entry);
    let order = b.gen_entry.as_ref().unwrap().order.clone();
    for name in &order {
        let (slot, ty) = b.gen_entry.as_ref().unwrap().slots[name];
        let idx = b.gen_entry.as_ref().unwrap().field_idx[name];
        let v = b.new_value(ty);
        b.emit(Inst::FieldGet {
            dest: v,
            base: 0,
            sid: gi.sid,
            idx,
        });
        b.store(slot, v);
    }
    let state = b.new_value(Ty::I64);
    b.emit(Inst::FieldGet {
        dest: state,
        base: 0,
        sid: gi.sid,
        idx: 0,
    });
    let mut arms: Vec<(i64, BlockId)> = vec![(0, body_start)];
    arms.extend(b.gen_entry.as_ref().unwrap().resume.iter().copied());
    let mut cur_cmp = entry;
    let n = arms.len();
    for (i, (sval, target)) in arms.into_iter().enumerate() {
        b.position(cur_cmp);
        let kc = b.iconst(Ty::I64, sval);
        let cond = b.new_value(Ty::I1);
        b.emit(Inst::ICmp {
            dest: cond,
            pred: CmpPred::Eq,
            lhs: state,
            rhs: kc,
        });
        if i + 1 < n {
            let next_cmp = b.new_block();
            b.set_term(Term::BrIf {
                cond,
                then_bb: target,
                else_bb: next_cmp,
            });
            cur_cmp = next_cmp;
        } else {
            b.set_term(Term::BrIf {
                cond,
                then_bb: target,
                else_bb: done,
            });
        }
    }
    b.gen_entry = None;
    Ok(b.func)
}

/// The form of an iterator's `__next__` (D-0048), selected by its return
/// signature.
enum NextForm {
    /// Comma-ok `-> (T, bool)`: `tuple_sid` is the `(T, bool)` layout, `elem` is
    /// `T`. The preferred, exception-free form.
    CommaOk { tuple_sid: u32, elem: Ty },
    /// Python `-> T` that raises `StopIteration` at exhaustion; `elem` is `T`.
    StopIter { elem: Ty },
}

struct FnLower<'a> {
    func: Func,
    cur: BlockId,
    scopes: Vec<HashMap<String, (Value, Ty)>>,
    loops: Vec<(BlockId, BlockId)>, // (continue, break)
    term_set: HashSet<BlockId>,     // blocks with a real terminator
    /// Stack of active landing-pad blocks (innermost `try` last). A call emitted
    /// while this is non-empty becomes an [`Inst::Invoke`] whose unwind edge goes
    /// to the top entry, so a raised exception transfers to the nearest handler.
    landing_pads: Vec<BlockId>,
    /// Landing-pad blocks actually targeted by an [`Inst::Invoke`]. A `try` whose
    /// body emitted no throwing call cannot raise, so its handler (and the
    /// `landingpad` that LLVM requires an `invoke` predecessor for) is omitted.
    used_lps: HashSet<BlockId>,
    /// Stack of active `scope:` handles (innermost last), each the `ptr` value
    /// returned by `zap_scope_enter`. A `spawn` lowered while this is non-empty is
    /// owned by the top scope ([`Owner::Scope`]); a `spawn` at the root is
    /// [`Owner::Root`] (D-0044, structured concurrency).
    scope_stack: Vec<Value>,
    /// Counter handing out distinct flag names for `with expect_raises(...)`
    /// desugaring (D-0054), so nested `with`s do not collide.
    with_counter: u32,
    /// Set while **discovering** a generator's live locals (I#67): each named
    /// binding is recorded here, first-seen first, as `(name, type)`. These
    /// become the state struct's fields. `None` outside generator discovery.
    gen_discover: Option<Vec<(String, Ty)>>,
    /// `yield` operand types seen during generator **discovery** (I#67); their
    /// join is the element type `T`. Empty otherwise.
    gen_elems: Vec<Ty>,
    /// Set while **emitting** a generator's `__next__` state machine (I#67).
    /// Carries the per-call working slots and the resume-point table.
    gen_entry: Option<GenEntry>,
    sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &'a StructCtx,
}

/// State threaded through the lowering of a generator's `__next__` (I#67). The
/// generator's live values live in the state struct's fields; each `__next__`
/// call loads them into per-call stack slots (allocas hoisted into the entry
/// block so they dominate every resume point), runs to the next `yield` (which
/// stores the slots back to the fields and returns `(value, true)`), and on
/// fall-off / bare `return` returns `(zeroed, false)`.
struct GenEntry {
    /// The `self` state-struct value (parameter 0 of `__next__`).
    self_val: Value,
    /// The state-struct id (for `FieldGet`/`FieldSet` on `self`).
    sid: u32,
    /// The entry block (block 0): hoisted allocas + field→slot loads + dispatch.
    entry_block: BlockId,
    /// The shared exhaustion block: sets `$state = -1` and returns `(_, false)`.
    done_block: BlockId,
    /// The `(T, bool)` tuple layout id returned by `__next__`.
    next_tuple_sid: u32,
    /// Element type `T` — the `yield` operand type (for coercing each operand).
    elem: Ty,
    /// Generator var name → its state-struct field index.
    field_idx: HashMap<String, u32>,
    /// Generator var name → `(per-call slot, type)`, in `order`.
    slots: HashMap<String, (Value, Ty)>,
    /// Var names in the order their slots were created (stable field-load/sync).
    order: Vec<String>,
    /// Next resume-state id to hand out (`0` is the start; `1..` are post-yield).
    next_state: i64,
    /// `(state id, resume block)` for every `yield`, used to build dispatch.
    resume: Vec<(i64, BlockId)>,
}

impl<'a> FnLower<'a> {
    fn new(
        name: String,
        params: Vec<Ty>,
        ret: Ty,
        is_main: bool,
        self_param: bool,
        sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
        ctx: &'a StructCtx,
    ) -> FnLower<'a> {
        let value_types = params.clone();
        let func = Func {
            name,
            params,
            ret,
            is_main,
            self_param,
            value_types,
            blocks: vec![Block {
                id: 0,
                insts: Vec::new(),
                term: Term::Unreachable,
            }],
        };
        FnLower {
            func,
            cur: 0,
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            term_set: HashSet::new(),
            landing_pads: Vec::new(),
            used_lps: HashSet::new(),
            scope_stack: Vec::new(),
            with_counter: 0,
            gen_discover: None,
            gen_elems: Vec::new(),
            gen_entry: None,
            sigs,
            ctx,
        }
    }

    /// Emit a call that may raise. Inside a `try` (a landing pad is active) this
    /// becomes an [`Inst::Invoke`] whose unwind edge targets the nearest handler;
    /// otherwise a plain [`Inst::Call`] (the exception, if any, unwinds through
    /// this frame to a caller's handler or to the top-level). The caller sets the
    /// terminator (e.g. `Unreachable` after `zap_panic`).
    fn throwing_call(&mut self, dest: Option<Value>, func: &str, args: Vec<Value>) {
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::Invoke {
                dest,
                func: func.to_string(),
                args,
                unwind: lp,
            });
        } else {
            self.emit(Inst::Call {
                dest,
                func: func.to_string(),
                args,
            });
        }
    }

    // --- builder primitives ---

    fn new_value(&mut self, ty: Ty) -> Value {
        let v = self.func.value_types.len() as Value;
        self.func.value_types.push(ty);
        v
    }

    fn new_block(&mut self) -> BlockId {
        let id = self.func.blocks.len() as BlockId;
        self.func.blocks.push(Block {
            id,
            insts: Vec::new(),
            term: Term::Unreachable,
        });
        id
    }

    fn position(&mut self, b: BlockId) {
        self.cur = b;
    }

    fn emit(&mut self, inst: Inst) {
        self.func.blocks[self.cur as usize].insts.push(inst);
    }

    fn set_term(&mut self, t: Term) {
        self.func.blocks[self.cur as usize].term = t;
        self.term_set.insert(self.cur);
    }

    fn terminated(&self) -> bool {
        self.term_set.contains(&self.cur)
    }

    fn ensure_br(&mut self, dest: BlockId) {
        if !self.terminated() {
            self.set_term(Term::Br(dest));
        }
    }

    // scopes
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, ptr: Value, ty: Ty) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (ptr, ty));
    }
    fn lookup(&self, name: &str) -> Option<(Value, Ty)> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Allocate and declare the stack slot for a named local binding (`let`,
    /// `auto`), returning the slot to store the initializer into.
    ///
    /// Outside a generator this is just `alloca` + `declare`. While **discovering**
    /// a generator's locals (I#67) it also records `(name, type)` so the name
    /// becomes a state-struct field. While **emitting** a generator's `__next__`
    /// it reuses the per-call slot created once in the entry block (so the binding
    /// persists across `yield`s via its field, and the alloca dominates every
    /// resume point); the first binding of each name creates that hoisted slot.
    fn decl_slot(&mut self, name: &str, ty: Ty) -> Value {
        if self.gen_discover.is_some() {
            if let Some(list) = self.gen_discover.as_mut() {
                if !list.iter().any(|(n, _)| n == name) {
                    list.push((name.to_string(), ty));
                }
            }
            let slot = self.alloca(ty);
            self.declare(name, slot, ty);
            return slot;
        }
        if self.gen_entry.is_some() {
            if let Some(&(slot, _)) = self.gen_entry.as_ref().unwrap().slots.get(name) {
                // Rebind the name to the existing hoisted slot (idempotent).
                self.declare(name, slot, ty);
                return slot;
            }
            return self.gen_entry_slot(name, ty);
        }
        let slot = self.alloca(ty);
        self.declare(name, slot, ty);
        slot
    }

    /// Create the per-call working slot for generator var `name` in the entry
    /// block (hoisted so it dominates every resume point), declare it, and record
    /// it for the entry field→slot load and the per-`yield` slot→field sync.
    fn gen_entry_slot(&mut self, name: &str, ty: Ty) -> Value {
        let entry = self.gen_entry.as_ref().unwrap().entry_block;
        let d = self.new_value(Ty::Ptr);
        self.func.blocks[entry as usize].insts.push(Inst::Alloca {
            dest: d,
            pointee: ty,
        });
        self.declare(name, d, ty);
        let ge = self.gen_entry.as_mut().unwrap();
        ge.slots.insert(name.to_string(), (d, ty));
        ge.order.push(name.to_string());
        d
    }

    // instruction helpers
    fn alloca(&mut self, pointee: Ty) -> Value {
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Alloca { dest: d, pointee });
        d
    }
    fn store(&mut self, ptr: Value, val: Value) {
        self.emit(Inst::Store { ptr, val });
    }
    fn load(&mut self, ptr: Value, ty: Ty) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::Load { dest: d, ptr });
        d
    }

    // --- value structs (D-0051) ---

    /// If `v : ty` is a `value struct` obtained from an existing object
    /// (`src` is an lvalue read), copy it so value semantics hold: the result is
    /// an independent instance whose later mutation does not escape to `src`
    /// (and vice-versa). A freshly produced value (constructor/call/operator
    /// result) is already uniquely owned, so it is returned unchanged. Reference
    /// structs (D-0023) are always returned unchanged — they share identity.
    /// This is also the *boxing* copy used when a value struct is stored into a
    /// collection or another aggregate (D-0051): the stored copy is independent.
    fn copy_if_value(&mut self, v: Value, ty: Ty, src: &ast::Expr) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.is_value_struct(sid) && is_lvalue_read(src) {
                return self.copy_value_struct(v, sid);
            }
        }
        v
    }

    /// Like [`copy_if_value`], but for a value crossing a **by-value calling
    /// boundary** — a function/method argument, a `return`, or a `spawn`
    /// argument. For an [`inline-eligible`](StructCtx::inline_eligible) aggregate
    /// (a flat value struct or tuple) no explicit copy is emitted: the
    /// register/sret ABI passes/returns it **by value**, which is itself an
    /// independent copy (D-0051), so a prior copy would be redundant and would
    /// reintroduce the heap allocation the inline layout removes. Non-inline
    /// value structs (those with a nested value-aggregate field, still a heap
    /// pointer) fall back to [`copy_if_value`]'s deep copy, preserving the
    /// observable value semantics unchanged.
    fn copy_for_pass(&mut self, v: Value, ty: Ty, src: &ast::Expr) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.inline_eligible(sid) {
                return v;
            }
        }
        self.copy_if_value(v, ty, src)
    }

    /// Box a value struct for storage in a collection or another aggregate
    /// (D-0051): a value struct stored in a list/dict/set is *boxed* — copied to
    /// the heap — so the stored element is independent of the original and later
    /// mutation of either does not affect the other. Unlike [`copy_if_value`],
    /// this copies unconditionally (the collection always owns its own instance),
    /// matching D-0051's "stored in a collection … is boxed (copied to the
    /// heap)". Non-value types pass through unchanged.
    fn box_if_value(&mut self, v: Value, ty: Ty) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.is_value_struct(sid) {
                return self.copy_value_struct(v, sid);
            }
        }
        v
    }

    /// A field-wise copy of value struct `sid` at `src` into a fresh instance.
    /// Nested value-struct fields are copied recursively (deep copy, so a nested
    /// value aggregate is independent too); reference-typed fields — including
    /// reference structs, strings, and collections — are shared (the pointer is
    /// copied), matching D-0023 reference semantics and "a value struct holding
    /// references is still GC-scanned".
    fn copy_value_struct(&mut self, src: Value, sid: u32) -> Value {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        let fields = self.ctx.fields[sid as usize].clone();
        for (i, (_, fty)) in fields.iter().enumerate() {
            let idx = i as u32;
            let fv = self.new_value(*fty);
            self.emit(Inst::FieldGet {
                dest: fv,
                base: src,
                sid,
                idx,
            });
            let fv = match *fty {
                Ty::Struct(fsid) if self.ctx.is_value_struct(fsid) => {
                    self.copy_value_struct(fv, fsid)
                }
                _ => fv,
            };
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: fv,
            });
        }
        dest
    }
    fn iconst(&mut self, ty: Ty, val: i64) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::IConst { dest: d, val });
        d
    }
    fn fconst(&mut self, ty: Ty, val: f64) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::FConst { dest: d, val });
        d
    }
    fn bconst(&mut self, val: bool) -> Value {
        let d = self.new_value(Ty::I1);
        self.emit(Inst::BConst { dest: d, val });
        d
    }

    // --- decorators & reflection (ticket #16, D-0036) ---
    //
    // This section is self-contained: it builds a string constant object, emits
    // the reflection-registry bootstrap, and lowers the reflection queries. All
    // of it is item-level/runtime-support work, isolated from expression and
    // numeric lowering.

    /// A fresh string object holding `s` (via the runtime string constructor).
    fn strconst(&mut self, s: &str) -> Value {
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::StrConst {
            dest: d,
            bytes: s.as_bytes().to_vec(),
        });
        d
    }

    /// Emit `zap_reflect_register(type, name)` for every (type, decorator/
    /// annotation) pair. Called once at the top of `main`.
    fn emit_reflect_registrations(&mut self, meta: &[(String, String)]) {
        for (ty, name) in meta {
            let tv = self.strconst(ty);
            let nv = self.strconst(name);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_reflect_register".to_string(),
                args: vec![tv, nv],
            });
        }
    }

    /// Emit `zap_signal_on(signo, &handler)` for every `@on_signal(Signal.X)`
    /// handler (ticket #77). Called once at the top of `main`. The runtime
    /// subscribes to the signal and spawns a detached goroutine that invokes the
    /// handler on each delivery (so it runs in full Zap context).
    fn emit_on_signal_registrations(&mut self, meta: &[(i64, String)]) {
        for (signo, fn_name) in meta {
            let s = self.iconst(Ty::I64, *signo);
            let fv = self.new_value(Ty::Ptr);
            self.emit(Inst::FuncAddr {
                dest: fv,
                func: fn_name.clone(),
            });
            self.emit(Inst::Call {
                dest: None,
                func: "zap_signal_on".to_string(),
                args: vec![s, fv],
            });
        }
    }

    /// Lower `reflect_has(type, name) -> bool` / `reflect_names(type) -> str`.
    fn lower_reflect_query(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let mut argvals = Vec::new();
        for a in args {
            let (v, vt) = self.expr(&a.value)?;
            if vt != Ty::Ptr {
                return Err(err(format!("{fname}() expects str arguments")));
            }
            argvals.push(v);
        }
        if fname == "reflect_names" {
            if argvals.len() != 1 {
                return Err(err("reflect_names(type) takes exactly one argument"));
            }
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_reflect_names".to_string(),
                args: argvals,
            });
            return Ok((d, Ty::Ptr));
        }
        // reflect_has: the runtime returns 1/0 as i64; normalize to a bool.
        if argvals.len() != 2 {
            return Err(err("reflect_has(type, name) takes exactly two arguments"));
        }
        let word = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(word),
            func: "zap_reflect_has".to_string(),
            args: argvals,
        });
        let zero = self.iconst(Ty::I64, 0);
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    // --- statements ---

    fn stmt(&mut self, st: &Stmt) -> Result<(), LowerError> {
        match st {
            Stmt::Let {
                ty, name, value, ..
            } => {
                let (v, vt) = self.expr(value)?;
                let declared = match ty {
                    Some(t) => resolve_ty(t, &self.ctx.tyenv())?,
                    None => vt,
                };
                let v = self.coerce(v, vt, declared);
                // Value structs are copied on binding (D-0051): `T b = a` makes
                // `b` an independent instance of the value aggregate.
                let v = self.copy_if_value(v, declared, value);
                // `decl_slot` is a plain alloca+declare outside a generator; in a
                // generator it records the local (discovery) or reuses the state
                // struct-backed per-call slot (emit), so the binding persists
                // across `yield`s (I#67).
                let slot = self.decl_slot(name, declared);
                self.store(slot, v);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                if let Expr::Name(n, _) = target {
                    let (v, vt) = self.expr(value)?;
                    let (ptr, ty) = self
                        .lookup(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let v = self.coerce(v, vt, ty);
                    // Value structs copy on assignment (D-0051).
                    let v = self.copy_if_value(v, ty, value);
                    self.store(ptr, v);
                    Ok(())
                } else if let Expr::Attr { base, name, .. } = target {
                    let (bv, bty) = self.expr(base)?;
                    let Ty::Struct(sid) = bty else {
                        return Err(err("field assignment on a non-struct value"));
                    };
                    let (idx, fty) = self
                        .ctx
                        .field(sid, name)
                        .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                    let (v, vt) = self.expr(value)?;
                    let v = self.coerce(v, vt, fty);
                    // Storing a value struct into a field copies it (D-0051), so
                    // the field holds an independent instance.
                    let v = self.copy_if_value(v, fty, value);
                    self.emit(Inst::FieldSet {
                        base: bv,
                        sid,
                        idx,
                        val: v,
                    });
                    Ok(())
                } else if let Expr::Index { base, index, .. } = target {
                    self.lower_index_set(base, index, value)
                } else {
                    Err(err(
                        "only variable, field, and index assignment are supported by codegen yet",
                    ))
                }
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                if let Expr::Name(n, _) = target {
                    let (ptr, ty) = self
                        .lookup(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let cur = self.load(ptr, ty);
                    let (rhs, rt) = self.expr(value)?;
                    // In-place augmented assignment (D-0047): if the LHS is a user
                    // struct that defines the matching in-place dunder, `a OP= b`
                    // calls `a.__iOP__(b)`, which mutates the operand in place and
                    // returns it — no fresh allocation as the binary `__OP__` form
                    // would need. Otherwise fall back to deriving from the binary
                    // operator (`a = a.__OP__(b)`). Reference structs share
                    // identity (D-0023), so the in-place mutation is the intended
                    // visible effect; the store-back is a no-op rebind for them and
                    // keeps the value-struct / scalar paths correct.
                    let (res, rty) = match (ty, inplace_op_dunder(*op)) {
                        (Ty::Struct(sid), Some(dunder))
                            if self.sigs.contains_key(&format!(
                                "{}__{}",
                                self.ctx.names[sid as usize], dunder
                            )) =>
                        {
                            self.call_dunder(sid, dunder, vec![(cur, ty), (rhs, rt)])?
                        }
                        _ => self.binop(*op, cur, ty, rhs, rt)?,
                    };
                    let res = self.coerce(res, rty, ty);
                    self.store(ptr, res);
                    Ok(())
                } else {
                    Err(err(
                        "only simple variable aug-assignment is supported by codegen yet",
                    ))
                }
            }
            Stmt::Expr(e) => {
                // A statement-position `yield` drives the generator state machine
                // (I#67); every other position is deferred (see `expr`).
                if let Expr::Yield { value, .. } = e {
                    return self.lower_yield_stmt(value.as_deref());
                }
                self.expr(e)?;
                Ok(())
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => self.lower_if(branches, else_block.as_ref()),
            Stmt::While { cond, body, .. } => self.lower_while(cond, body),
            Stmt::DoWhile { body, cond, .. } => self.lower_do_while(body, cond),
            Stmt::For {
                target, iter, body, ..
            } => self.lower_for(target, iter, body),
            // Inside a generator, `return` ends iteration (D-0048/PEP 479): jump
            // to the shared exhaustion block (sema rejects `return <value>`).
            Stmt::Return { .. } if self.gen_entry.is_some() => {
                let done = self.gen_entry.as_ref().unwrap().done_block;
                self.set_term(Term::Br(done));
                Ok(())
            }
            Stmt::Return { value, .. } => {
                match value {
                    Some(e) => {
                        let (v, vt) = self.expr(e)?;
                        let ret = self.func.ret;
                        let v = if ret == Ty::Void {
                            v
                        } else {
                            self.coerce(v, vt, ret)
                        };
                        // Returning a value struct yields an instance independent
                        // of the callee's locals (D-0051). An inline-eligible
                        // aggregate is returned by value (register/sret), which is
                        // already that copy; a non-inline value struct is deep-
                        // copied here. See `copy_for_pass`.
                        let v = self.copy_for_pass(v, ret, e);
                        self.set_term(Term::Ret(Some(v)));
                    }
                    None => self.set_term(Term::Ret(None)),
                }
                Ok(())
            }
            Stmt::Break(_) => {
                let (_, brk) = *self
                    .loops
                    .last()
                    .ok_or_else(|| err("'break' outside a loop"))?;
                self.set_term(Term::Br(brk));
                Ok(())
            }
            Stmt::Continue(_) => {
                let (cont, _) = *self
                    .loops
                    .last()
                    .ok_or_else(|| err("'continue' outside a loop"))?;
                self.set_term(Term::Br(cont));
                Ok(())
            }
            Stmt::Raise { value, .. } => self.lower_raise(value.as_ref()),
            Stmt::Assert {
                cond,
                msg,
                cond_src,
                ..
            } => self.lower_assert(cond, msg.as_ref(), cond_src),
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => self.lower_try(body, excepts, finally.as_ref()),
            Stmt::Spawn { call, detached, .. } => self.lower_spawn(call, *detached),
            Stmt::Scope { opts, body, .. } => self.lower_scope(opts, body),
            Stmt::Match {
                scrutinee, arms, ..
            } => self.lower_match(scrutinee, arms),
            // Tuple-destructuring declaration / assignment (D-0049): lower the
            // value to its tuple aggregate, then bind each target via `FieldGet`.
            Stmt::LetTuple { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                self.lower_bind_target(target, v, vt, true)
            }
            Stmt::AssignTuple { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                self.lower_bind_target(target, v, vt, false)
            }
            Stmt::With { items, body, .. } => self.lower_with(items, body),
        }
    }

    /// Bind a destructuring target to a value (D-0049). `is_decl` chooses between
    /// introducing new locals (declaration) and storing into existing lvalues
    /// (reassignment). A `Tuple` target reads each element out of the aggregate
    /// with `FieldGet`; a `*rest` rebuilds the remaining elements into a sub-tuple.
    fn lower_bind_target(
        &mut self,
        t: &BindTarget,
        val: Value,
        ty: Ty,
        is_decl: bool,
    ) -> Result<(), LowerError> {
        match t {
            BindTarget::Elem(e) => self.lower_bind_leaf(e, val, ty, is_decl),
            BindTarget::Rest(_, _) => {
                Err(err("'*rest' is only valid as an element of a tuple target"))
            }
            BindTarget::Tuple(elems, _) => {
                let Ty::Struct(sid) = ty else {
                    return Err(err("cannot destructure a non-tuple value"));
                };
                let field_tys = self
                    .ctx
                    .tuples
                    .borrow()
                    .fields(sid)
                    .ok_or_else(|| err("cannot destructure a non-tuple value"))?;
                self.lower_bind_tuple(elems, val, sid, &field_tys, is_decl)
            }
        }
    }

    /// Bind the elements of a tuple target against the aggregate's fields,
    /// honoring at most one `*rest` (which captures the remaining fields as a
    /// freshly built sub-tuple). Arity is validated in sema.
    fn lower_bind_tuple(
        &mut self,
        elems: &[BindTarget],
        base: Value,
        sid: u32,
        field_tys: &[Ty],
        is_decl: bool,
    ) -> Result<(), LowerError> {
        let rest_pos = elems.iter().position(|e| matches!(e, BindTarget::Rest(..)));
        match rest_pos {
            None => {
                if elems.len() != field_tys.len() {
                    return Err(err(format!(
                        "tuple destructuring expects {} element(s), found {}",
                        field_tys.len(),
                        elems.len()
                    )));
                }
                for (idx, e) in elems.iter().enumerate() {
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                Ok(())
            }
            Some(r) => {
                let after = elems.len() - r - 1;
                if field_tys.len() + 1 < elems.len() {
                    return Err(err("tuple destructuring with '*rest' needs more elements"));
                }
                let tail_start = field_tys.len() - after;
                // Fixed elements before the rest.
                for (idx, e) in elems[..r].iter().enumerate() {
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                // `*rest`: rebuild the middle fields into a new sub-tuple.
                let rest_tys: Vec<Ty> = field_tys[r..tail_start].to_vec();
                let rest_sid = self.ctx.tuples.borrow_mut().intern(rest_tys.clone());
                let rest_val = self.new_value(Ty::Struct(rest_sid));
                self.emit(Inst::StructNew {
                    dest: rest_val,
                    sid: rest_sid,
                });
                for (j, fty) in rest_tys.iter().enumerate() {
                    let fv = self.field_get(base, sid, (r + j) as u32, *fty);
                    self.emit(Inst::FieldSet {
                        base: rest_val,
                        sid: rest_sid,
                        idx: j as u32,
                        val: fv,
                    });
                }
                if let BindTarget::Rest(inner, _) = &elems[r] {
                    self.lower_bind_target(inner, rest_val, Ty::Struct(rest_sid), is_decl)?;
                }
                // Fixed elements after the rest.
                for (k, e) in elems[r + 1..].iter().enumerate() {
                    let idx = tail_start + k;
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                Ok(())
            }
        }
    }

    /// Emit a `FieldGet` yielding field `idx` of tuple/struct `sid`.
    fn field_get(&mut self, base: Value, sid: u32, idx: u32, fty: Ty) -> Value {
        let d = self.new_value(fty);
        self.emit(Inst::FieldGet {
            dest: d,
            base,
            sid,
            idx,
        });
        d
    }

    /// Bind one leaf of a destructuring target: a new local (declaration) or an
    /// existing lvalue (reassignment — a name or a struct field).
    fn lower_bind_leaf(
        &mut self,
        e: &Expr,
        val: Value,
        ty: Ty,
        is_decl: bool,
    ) -> Result<(), LowerError> {
        if is_decl {
            let Expr::Name(name, _) = e else {
                return Err(err("a declaration target must be a name"));
            };
            let slot = self.alloca(ty);
            self.store(slot, val);
            self.declare(name, slot, ty);
            return Ok(());
        }
        match e {
            Expr::Name(n, _) => {
                let (ptr, dt) = self
                    .lookup(n)
                    .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                let v = self.coerce(val, ty, dt);
                self.store(ptr, v);
                Ok(())
            }
            Expr::Attr { base, name, .. } => {
                let (bv, bty) = self.expr(base)?;
                let Ty::Struct(sid) = bty else {
                    return Err(err("field assignment on a non-struct value"));
                };
                let (idx, fty) = self
                    .ctx
                    .field(sid, name)
                    .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                let v = self.coerce(val, ty, fty);
                self.emit(Inst::FieldSet {
                    base: bv,
                    sid,
                    idx,
                    val: v,
                });
                Ok(())
            }
            _ => Err(err(
                "only name and field targets are supported in tuple destructuring assignment yet",
            )),
        }
    }

    /// Lower `spawn f(args)` (ticket #25, D-0013). The operand must be a call;
    /// its arguments are evaluated eagerly here, in the spawning coroutine (so
    /// argument evaluation happens-before the child — D-0034), coerced to the
    /// callee's parameter types, and handed to [`Inst::Spawn`], which codegen
    /// turns into a GC-registered thread running `f`. Fire-and-forget: the spawn
    /// yields no handle; the root scope joins all spawned threads at program exit
    /// (`zap_rt_join_all`, emitted at `main`'s return). Only a direct call to a
    /// named top-level function is supported in this increment; method-call and
    /// lambda operands (`spawn s.handle(c)`, `spawn (lambda: ...)()`) are a
    /// documented follow-up, as is the structured-concurrency `scope:` block.
    fn lower_spawn(&mut self, call: &Expr, detached: bool) -> Result<(), LowerError> {
        let Expr::Call { callee, args, .. } = call else {
            return Err(err("spawn operand must be a call expression (D-0013)"));
        };
        let Expr::Name(fname, _) = callee.as_ref() else {
            return Err(err(
                "spawn of a method or lambda call is not supported yet (only `spawn f(...)` to a named function)",
            ));
        };
        let (ptys, _ret) = self
            .sigs
            .get(fname)
            .cloned()
            .ok_or_else(|| err(format!("spawn of unknown function '{fname}'")))?;
        let mut argvals = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let (v, vt) = self.expr(&a.value)?;
            let target = *ptys.get(i).unwrap_or(&vt);
            let v = self.coerce(v, vt, target);
            // A value struct captured by a goroutine is copied (D-0051) so it
            // safely outlives the spawning frame. A non-inline value struct is
            // deep-copied here (heap). An inline-eligible aggregate is copied by
            // value into the GC-allocated spawn closure (codegen stores the
            // aggregate, not a pointer), which outlives the frame just the same,
            // so no explicit copy is emitted. See `copy_for_pass`.
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        // Ownership (D-0044): `spawn detached` opts out of all scopes; otherwise
        // the innermost enclosing `scope:` owns it, falling back to the implicit
        // root scope at the top level.
        let owner = if detached {
            Owner::Detached
        } else if let Some(&s) = self.scope_stack.last() {
            Owner::Scope(s)
        } else {
            Owner::Root
        };
        self.emit(Inst::Spawn {
            func: fname.clone(),
            args: argvals,
            owner,
        });
        Ok(())
    }

    /// Lower a `scope:` block (D-0044/D-0045, structured concurrency): a lexical
    /// region that owns every `spawn` inside it and joins — or, on failure/early
    /// exit, cancels then awaits — them at the dedent.
    ///
    /// Shape (mirrors `lower_try`'s landing-pad handling): a fresh scope handle is
    /// opened with `zap_scope_enter(on_error)`, pushed onto `scope_stack` so inner
    /// `spawn`s bind to it, and the body is lowered under a landing pad. On normal
    /// completion `zap_scope_exit` is the join barrier — it waits for all owned
    /// coroutines and re-raises their failure (a single exception, or an aggregate
    /// `ExceptionGroup` for several concurrent failures, D-0045). If the body
    /// itself raises (or leaves early by a `raise`), the landing pad calls
    /// `zap_scope_exit_cancel` to cancel and await the children, then re-raises the
    /// parent's in-flight exception (rule 4; the parent's failure wins over the
    /// children's, which are cancelled).
    fn lower_scope(&mut self, opts: &[ast::Arg], body: &ast::Block) -> Result<(), LowerError> {
        let on_error = self.scope_on_error(opts)?;
        let policy = self.iconst(Ty::I64, on_error);
        let handle = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(handle),
            func: "zap_scope_enter".to_string(),
            args: vec![policy],
        });

        // `cancel_on=[Signal.X, ...]` + optional `grace=<ms>` (ticket #77,
        // D-0058): arm this scope so a listed signal cancels its tree, with an
        // optional hard-exit deadline. The subscription is removed inside
        // `zap_scope_exit`/`_cancel` (after the join), so it stays active while
        // the scope awaits its children.
        let cancel_mask = self.scope_cancel_mask(opts)?;
        if cancel_mask != 0 {
            let grace = match self.scope_grace(opts)? {
                Some(v) => v,
                None => self.iconst(Ty::I64, 0),
            };
            let maskv = self.iconst(Ty::I64, cancel_mask);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_scope_cancel_on".to_string(),
                args: vec![handle, maskv, grace],
            });
        }

        let lp = self.new_block();
        let after = self.new_block();

        // --- owned region: a raise inside unwinds to `lp` (cancel + re-raise) ---
        self.landing_pads.push(lp);
        self.scope_stack.push(handle);
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        self.scope_stack.pop();
        self.landing_pads.pop();

        // Normal completion: the join barrier. `zap_scope_exit` awaits the owned
        // coroutines and re-raises their failure, so it is a throwing call (an
        // aggregate may unwind to an enclosing `try`).
        if !self.terminated() {
            self.throwing_call(None, "zap_scope_exit", vec![handle]);
            self.ensure_br(after);
        }

        // If the body emitted no throwing call it cannot raise, so the cancel
        // landing pad is dead (LLVM requires a `landingpad` to have an `invoke`
        // predecessor) — skip it.
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // --- cancel landing pad: the body raised; cancel + await children, then
        // re-raise the parent's exception (rule 4). ---
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        self.emit(Inst::Call {
            dest: None,
            func: "zap_scope_exit_cancel".to_string(),
            args: vec![handle],
        });
        if !self.terminated() {
            self.emit_end_catch();
            self.throwing_call(None, "zap_panic", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        self.position(after);
        Ok(())
    }

    /// The `on_error` policy constant for a `scope(...)` (D-0044): `0` = Cancel
    /// (the default — a failure cancels siblings), `1` = Wait (a failure waits for
    /// siblings instead of cancelling). Validated in sema; re-checked here so a
    /// bad option is a lowering error rather than a silent default.
    fn scope_on_error(&self, opts: &[ast::Arg]) -> Result<i64, LowerError> {
        let mut policy = 0; // OnError.Cancel
        for a in opts {
            match a.name.as_deref() {
                Some("on_error") => {
                    if let Expr::Attr { base, name, .. } = &a.value {
                        if matches!(base.as_ref(), Expr::Name(n, _) if n == "OnError") {
                            policy = match name.as_str() {
                                "Cancel" => 0,
                                "Wait" => 1,
                                _ => {
                                    return Err(err(
                                        "scope on_error expects OnError.Cancel or OnError.Wait",
                                    ))
                                }
                            };
                            continue;
                        }
                    }
                    return Err(err("scope on_error expects OnError.Cancel or OnError.Wait"));
                }
                Some("timeout") => {
                    return Err(err(
                        "scope(timeout=...) is not supported yet (the timer API is deferred)",
                    ))
                }
                // Handled separately by `scope_cancel_on` (ticket #77).
                Some("cancel_on") | Some("grace") => continue,
                Some(other) => return Err(err(format!("scope() has no option '{other}'"))),
                None => return Err(err("scope() options must be keyword arguments")),
            }
        }
        Ok(policy)
    }

    /// The `cancel_on` signal bitmask for a `scope(cancel_on=[Signal.X, ...])`
    /// (ticket #77, D-0058): bit N set for each listed signal number. Returns 0
    /// when the scope has no `cancel_on`. The `Signal` members are tier-1
    /// discriminants that ARE the OS signal numbers.
    fn scope_cancel_mask(&self, opts: &[ast::Arg]) -> Result<i64, LowerError> {
        let mut mask: i64 = 0;
        for a in opts {
            if a.name.as_deref() != Some("cancel_on") {
                continue;
            }
            let Expr::List(elems, _) = &a.value else {
                return Err(err(
                    "scope cancel_on expects a list, e.g. cancel_on=[Signal.SIGINT]",
                ));
            };
            for e in elems {
                let signo = match e {
                    Expr::Attr { base, name, .. } if matches!(base.as_ref(), Expr::Name(n, _) if n == "Signal") => {
                        self.ctx
                            .enums
                            .get("Signal")
                            .and_then(|en| en.member_value(name))
                            .ok_or_else(|| err(format!("unknown Signal member '{name}'")))?
                    }
                    _ => {
                        return Err(err(
                            "scope cancel_on expects Signal members, e.g. Signal.SIGINT",
                        ))
                    }
                };
                if (1..63).contains(&signo) {
                    mask |= 1i64 << signo;
                }
            }
        }
        Ok(mask)
    }

    /// The `grace=<ms>` hard-exit deadline expression for a `scope(...)`, lowered
    /// to an i64 value (ticket #77); `None` when absent (no deadline).
    fn scope_grace(&mut self, opts: &[ast::Arg]) -> Result<Option<Value>, LowerError> {
        for a in opts {
            if a.name.as_deref() == Some("grace") {
                let (v, vt) = self.expr(&a.value)?;
                return Ok(Some(self.coerce(v, vt, Ty::I64)));
            }
        }
        Ok(None)
    }

    /// Balance the `__cxa_begin_catch` performed on landing-pad entry. Called on
    /// each path that leaves a handler normally or re-raises.
    fn emit_end_catch(&mut self) {
        self.emit(Inst::Call {
            dest: None,
            func: "zap_eh_end_catch".to_string(),
            args: Vec::new(),
        });
    }

    /// `raise EXPR` — build the exception and begin unwinding via `zap_panic`
    /// (RUNTIME-ABI §"Exception / unwinding ABI"). For this increment the
    /// exception object is the message string itself; the typed exception
    /// hierarchy (D-0029) is deferred. `zap_panic` never returns, so the normal
    /// path is `unreachable` (inside a `try`, the invoke's unwind edge carries
    /// control to the landing pad instead).
    fn lower_raise(&mut self, value: Option<&Expr>) -> Result<(), LowerError> {
        let Some(e) = value else {
            return Err(err("bare 'raise' (re-raise) is not supported yet"));
        };
        // `raise StopIteration` / `raise StopIteration()` (the Python iterator
        // end-of-iteration signal, D-0048) lowers to a panic carrying a sentinel
        // message the iteration-protocol landing pad recognizes.
        if is_stop_iteration(e) {
            let msg = self.new_value(Ty::Ptr);
            self.emit(Inst::StrConst {
                dest: msg,
                bytes: b"StopIteration".to_vec(),
            });
            self.throwing_call(None, "zap_panic", vec![msg]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
            return Ok(());
        }
        let (v, vt) = self.expr(e)?;
        if vt != Ty::Ptr {
            return Err(err(
                "raise currently expects a str value (the exception message)",
            ));
        }
        self.throwing_call(None, "zap_panic", vec![v]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        Ok(())
    }

    /// `assert cond [, msg]` (D-0054). Always evaluated in v1: the condition is
    /// tested, and on failure a *power-assert* message — the condition's source
    /// text plus its operand values — is built and raised through the ordinary
    /// `zap_panic` path (so an enclosing `try`/`except`, and the `zap test`
    /// runner, catch it like any other exception).
    ///
    /// For a top-level comparison (`a == b`, `a < b`, …) each operand is
    /// evaluated exactly once and its value retained, so the diagnostic can show
    /// both sides without re-evaluating (no double side effects). Other
    /// conditions fall back to showing the whole expression's truthiness.
    fn lower_assert(
        &mut self,
        cond: &Expr,
        msg: Option<&Expr>,
        cond_src: &str,
    ) -> Result<(), LowerError> {
        // Decompose a comparison so both operand values can be reported.
        let decomposed = match cond {
            Expr::Binary { op, lhs, rhs, .. } if is_power_comparison(*op) => {
                let (lv, lt) = self.expr(lhs)?;
                let (rv, rt) = self.expr(rhs)?;
                let (bit, _) = self.binop(*op, lv, lt, rv, rt)?;
                Some((bit, (lv, lt), (rv, rt)))
            }
            _ => None,
        };
        let bit = match decomposed {
            Some((bit, ..)) => bit,
            None => {
                let (cv, ct) = self.expr(cond)?;
                self.truthy(cv, ct)
            }
        };

        let fail = self.new_block();
        let after = self.new_block();
        self.set_term(Term::BrIf {
            cond: bit,
            then_bb: after,
            else_bb: fail,
        });

        // --- failure path: build the message and raise it ---
        self.position(fail);
        let mut acc = self.strconst("assertion failed");
        if let Some(m) = msg {
            let (mv, mt) = self.expr(m)?;
            // A str message is used verbatim; anything else is stringified.
            let ms = if mt == Ty::Ptr {
                mv
            } else {
                self.value_to_str(mv, mt)
            };
            let sep = self.strconst(": ");
            acc = gen_concat(self, acc, sep);
            acc = gen_concat(self, acc, ms);
        }
        let srcline = self.strconst(&format!("\n  assert {cond_src}"));
        acc = gen_concat(self, acc, srcline);
        if let Some((_, (lv, lt), (rv, rt))) = decomposed {
            let ls = self.value_to_str(lv, lt);
            let rs = self.value_to_str(rv, rt);
            let lpfx = self.strconst("\n  left:  ");
            acc = gen_concat(self, acc, lpfx);
            acc = gen_concat(self, acc, ls);
            let rpfx = self.strconst("\n  right: ");
            acc = gen_concat(self, acc, rpfx);
            acc = gen_concat(self, acc, rs);
        }
        self.throwing_call(None, "zap_panic", vec![acc]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(after);
        Ok(())
    }

    /// Best-effort stringification of a value for power-assert diagnostics.
    /// Returns a `Ptr` (ZapStr). Never fails: an unsupported type renders as a
    /// `<type>` placeholder so `assert` always lowers.
    fn value_to_str(&mut self, v: Value, t: Ty) -> Value {
        match t {
            // Quote strings so empty/whitespace values are visible.
            Ty::Ptr => {
                let q = self.strconst("\"");
                let a = gen_concat(self, q, v);
                let q2 = self.strconst("\"");
                gen_concat(self, a, q2)
            }
            Ty::I1 => {
                let i64v = self.coerce(v, Ty::I1, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bool_to_str".to_string(),
                    args: vec![i64v],
                });
                d
            }
            t if t.int_width().is_some() => {
                let i64v = self.coerce(v, t, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_to_str".to_string(),
                    args: vec![i64v],
                });
                d
            }
            t if t.is_float() => {
                let f64v = self.coerce(v, t, Ty::F64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_f64_to_str".to_string(),
                    args: vec![f64v],
                });
                d
            }
            Ty::Struct(sid) => {
                // Use the struct's own `__str__`/`__repr__` when it has one;
                // otherwise fall back to a type-name placeholder (never fail).
                for dunder in ["__str__", "__repr__"] {
                    let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
                    if self.sigs.contains_key(&mangled) {
                        let d = self.new_value(Ty::Ptr);
                        self.throwing_call(Some(d), &mangled, vec![v]);
                        return d;
                    }
                }
                self.strconst(&format!("<{}>", self.ctx.names[sid as usize]))
            }
            _ => self.strconst("<value>"),
        }
    }

    /// `with items: BODY` (D-0027). Only the `with expect_raises(ErrType):`
    /// exception-assertion form is supported (D-0054): BODY must raise, or the
    /// `with` itself fails. The general context-manager protocol
    /// (`__enter__`/`__exit__`) is a documented follow-up.
    ///
    /// Desugars to the equivalent `try`/flag/`raise`:
    /// ```text
    /// int __er_N = 0
    /// try: BODY
    /// except: __er_N = 1
    /// if __er_N == 0: raise "expected <ErrType> ... but none was raised"
    /// ```
    /// so a raise inside BODY is swallowed and a missing one propagates (and is
    /// caught by an enclosing test). Typed matching is catch-all until the typed
    /// exception hierarchy (D-0029) lands — the type name is diagnostic only.
    fn lower_with(&mut self, items: &[ast::WithItem], body: &ast::Block) -> Result<(), LowerError> {
        let bad = || {
            err(
                "`with` currently supports only `with expect_raises(ErrType):` \
                 (general context managers are a follow-up, D-0027)",
            )
        };
        if items.len() != 1 {
            return Err(bad());
        }
        let Expr::Call { callee, args, .. } = &items[0].expr else {
            return Err(bad());
        };
        if !is_name(callee, "expect_raises") {
            return Err(bad());
        }
        let ty_name = args
            .first()
            .map(|a| ast::render_expr(&a.value))
            .unwrap_or_else(|| "an exception".to_string());

        // Build the desugared statements as AST and lower them, reusing the
        // existing try / if / raise lowering.
        let sp = body.span;
        let flag = format!("__er_{}", self.new_with_id());
        let miss = format!("expected {ty_name} to be raised, but the with-body returned normally");

        let decl = Stmt::Let {
            is_const: false,
            ty: None,
            name: flag.clone(),
            value: Expr::Int("0".to_string(), sp),
            span: sp,
        };
        let set_flag = Stmt::Assign {
            target: Expr::Name(flag.clone(), sp),
            value: Expr::Int("1".to_string(), sp),
            span: sp,
        };
        let try_stmt = Stmt::Try {
            body: body.clone(),
            excepts: vec![ast::ExceptClause {
                ty: None,
                binding: None,
                body: ast::Block {
                    stmts: vec![set_flag],
                    span: sp,
                },
                span: sp,
            }],
            finally: None,
            span: sp,
        };
        let check = Stmt::If {
            branches: vec![(
                Expr::Binary {
                    op: AstBin::Eq,
                    lhs: Box::new(Expr::Name(flag.clone(), sp)),
                    rhs: Box::new(Expr::Int("0".to_string(), sp)),
                    span: sp,
                },
                ast::Block {
                    stmts: vec![Stmt::Raise {
                        value: Some(Expr::Str {
                            value: miss,
                            raw: true,
                            bytes: false,
                            fstring: false,
                            span: sp,
                        }),
                        span: sp,
                    }],
                    span: sp,
                },
            )],
            else_block: None,
            span: sp,
        };

        self.push_scope();
        self.stmt(&decl)?;
        self.stmt(&try_stmt)?;
        self.stmt(&check)?;
        self.pop_scope();
        Ok(())
    }

    /// A monotonically increasing id so each `with` desugaring uses a distinct
    /// flag variable (safe even when `with`s nest).
    fn new_with_id(&mut self) -> u32 {
        let id = self.with_counter;
        self.with_counter += 1;
        id
    }

    /// `try: BODY except [Type [as e]]: HANDLER finally: FIN` with real stack
    /// unwinding. Calls in BODY become `invoke`s whose unwind edge targets a
    /// landing-pad block; the handler matches catch-all for this increment
    /// (type-directed `except` matching is deferred). `finally` runs on the
    /// normal path and on the handled/propagating paths. If no `except` clause
    /// is present the exception is re-raised after `finally` (Python semantics).
    fn lower_try(
        &mut self,
        body: &ast::Block,
        excepts: &[ast::ExceptClause],
        finally: Option<&ast::Block>,
    ) -> Result<(), LowerError> {
        let lp = self.new_block();
        let after = self.new_block();

        // --- protected body: calls unwind to `lp` ---
        self.landing_pads.push(lp);
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        self.landing_pads.pop();
        // Normal completion: run `finally`, then continue past the `try`.
        if !self.terminated() {
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            self.ensure_br(after);
        }

        // If the body emitted no throwing call it cannot raise, so the handler
        // is dead — skip the landing pad entirely (LLVM requires a `landingpad`
        // block to have an `invoke` predecessor).
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // --- landing pad: entered while unwinding ---
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        if let Some(first) = excepts.first() {
            // Catch-all handler (this increment). `as e` binds the message string.
            self.push_scope();
            if let Some(bind) = &first.binding {
                let slot = self.alloca(Ty::Ptr);
                self.store(slot, exc);
                self.declare(bind, slot, Ty::Ptr);
            }
            for st in &first.body.stmts {
                self.stmt(st)?;
            }
            self.pop_scope();
            if !self.terminated() {
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                self.ensure_br(after);
            }
            // NOTE: a second/third `except` clause is currently unreachable
            // (matching is catch-all). Type-directed dispatch is deferred.
        } else {
            // finally-only `try`: run cleanup, then propagate to the caller.
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            if !self.terminated() {
                self.emit_end_catch();
                self.throwing_call(None, "zap_panic", vec![exc]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
            }
        }

        self.position(after);
        Ok(())
    }

    fn lower_block(&mut self, b: &ast::Block) -> Result<(), LowerError> {
        self.push_scope();
        for st in &b.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn lower_if(
        &mut self,
        branches: &[(Expr, ast::Block)],
        else_block: Option<&ast::Block>,
    ) -> Result<(), LowerError> {
        let merge = self.new_block();
        for (cond, body) in branches {
            let (cv, cty) = self.expr(cond)?;
            let c = self.truthy(cv, cty);
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: c,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.lower_block(body)?;
            self.ensure_br(merge);
            self.position(next_bb);
        }
        if let Some(eb) = else_block {
            self.lower_block(eb)?;
        }
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    fn lower_while(&mut self, cond: &Expr, body: &ast::Block) -> Result<(), LowerError> {
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let (cv, cty) = self.expr(cond)?;
        let c = self.truthy(cv, cty);
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        self.loops.push((cond_bb, end_bb));
        self.lower_block(body)?;
        self.loops.pop();
        self.ensure_br(cond_bb);
        self.position(end_bb);
        Ok(())
    }

    fn lower_do_while(&mut self, body: &ast::Block, cond: &Expr) -> Result<(), LowerError> {
        let body_bb = self.new_block();
        let cond_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(body_bb));
        self.position(body_bb);
        self.loops.push((cond_bb, end_bb));
        self.lower_block(body)?;
        self.loops.pop();
        self.ensure_br(cond_bb);
        self.position(cond_bb);
        let (cv, cty) = self.expr(cond)?;
        let c = self.truthy(cv, cty);
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(end_bb);
        Ok(())
    }

    /// Resolve the iterator for a struct-typed iterable (D-0048): call
    /// `__iter__` (the `Iterable` interface) to get a fresh iterator, or, if the
    /// value is itself an iterator (it defines `__next__` but no `__iter__`), use
    /// it directly. Returns the iterator value and its struct id.
    fn setup_iter(&mut self, obj: Value, sid: u32) -> Result<(Value, u32), LowerError> {
        let name = self.ctx.names[sid as usize].clone();
        let iter_m = format!("{}__{}", name, "__iter__");
        if let Some((_p, ret)) = self.sigs.get(&iter_m).cloned() {
            let Ty::Struct(iter_sid) = ret else {
                return Err(err(format!(
                    "'{name}.__iter__' must return an iterator struct (the 'Iterable' \
                     interface, D-0048)"
                )));
            };
            let d = self.new_value(ret);
            self.throwing_call(Some(d), &iter_m, vec![obj]);
            Ok((d, iter_sid))
        } else if self.sigs.contains_key(&format!("{}__{}", name, "__next__")) {
            Ok((obj, sid)) // the value is already an iterator
        } else {
            Err(err(format!(
                "type '{name}' is not iterable: define '__iter__' (the 'Iterable' \
                 interface, D-0048)"
            )))
        }
    }

    /// Classify an iterator's `__next__` (D-0048) by its return signature: the
    /// comma-ok form `-> (T, bool)` (preferred) versus the Python/`StopIteration`
    /// form `-> T`. Returns the element type and, for comma-ok, the `(T, bool)`
    /// tuple's struct id.
    fn next_form(&self, iter_sid: u32) -> Result<NextForm, LowerError> {
        let name = &self.ctx.names[iter_sid as usize];
        let m = format!("{}__{}", name, "__next__");
        let (_p, ret) = self.sigs.get(&m).cloned().ok_or_else(|| {
            err(format!(
                "iterator '{name}' must define '__next__' (the 'Iterator' interface, D-0048)"
            ))
        })?;
        if let Ty::Struct(tsid) = ret {
            if let Some(fields) = self.ctx.tuples.borrow().fields(tsid) {
                if fields.len() == 2 && fields[1] == Ty::I1 {
                    return Ok(NextForm::CommaOk {
                        tuple_sid: tsid,
                        elem: fields[0],
                    });
                }
            }
        }
        Ok(NextForm::StopIter { elem: ret })
    }

    /// Drive the iteration protocol (D-0048) over struct-typed iterable `obj`
    /// (`sid`): obtain the iterator via [`setup_iter`], then loop calling
    /// `__next__`, invoking `body` with each element value/type. `body` owns the
    /// loop variable / accumulation; a `for` body's `break`/`continue` target the
    /// loop end / the next `__next__`.
    fn drive_iteration(
        &mut self,
        obj: Value,
        sid: u32,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let (it, iter_sid) = self.setup_iter(obj, sid)?;
        let next_m = format!("{}__{}", self.ctx.names[iter_sid as usize], "__next__");
        match self.next_form(iter_sid)? {
            NextForm::CommaOk { tuple_sid, elem } => {
                let head = self.new_block();
                let body_bb = self.new_block();
                let end = self.new_block();
                self.set_term(Term::Br(head));
                self.position(head);
                let res = self.new_value(Ty::Struct(tuple_sid));
                self.throwing_call(Some(res), &next_m, vec![it]);
                let v = self.new_value(elem);
                self.emit(Inst::FieldGet {
                    dest: v,
                    base: res,
                    sid: tuple_sid,
                    idx: 0,
                });
                let ok = self.new_value(Ty::I1);
                self.emit(Inst::FieldGet {
                    dest: ok,
                    base: res,
                    sid: tuple_sid,
                    idx: 1,
                });
                self.set_term(Term::BrIf {
                    cond: ok,
                    then_bb: body_bb,
                    else_bb: end,
                });
                self.position(body_bb);
                self.loops.push((head, end));
                body(self, v, elem)?;
                self.loops.pop();
                self.ensure_br(head);
                self.position(end);
            }
            NextForm::StopIter { elem } => {
                // The Python form: `__next__` returns the element directly and
                // raises `StopIteration` at exhaustion. Guard the call with a
                // landing pad that ends the loop on `StopIteration` and re-raises
                // anything else (D-0048; zero-cost table EH, D-0007).
                let head = self.new_block();
                let body_bb = self.new_block();
                let lp = self.new_block();
                let reraise = self.new_block();
                let end = self.new_block();
                self.set_term(Term::Br(head));
                self.position(head);
                self.landing_pads.push(lp);
                let v = self.new_value(elem);
                self.throwing_call(Some(v), &next_m, vec![it]);
                self.landing_pads.pop();
                self.set_term(Term::Br(body_bb));
                self.position(body_bb);
                self.loops.push((head, end));
                body(self, v, elem)?;
                self.loops.pop();
                self.ensure_br(head);
                // Landing pad: compare the in-flight exception message against the
                // `StopIteration` sentinel (what `raise StopIteration` lowers to).
                self.position(lp);
                let exc = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: exc });
                let sentinel = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: sentinel,
                    bytes: b"StopIteration".to_vec(),
                });
                let eq = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(eq),
                    func: "zap_str_eq".to_string(),
                    args: vec![exc, sentinel],
                });
                let zero = self.iconst(Ty::I64, 0);
                let is_stop = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: is_stop,
                    pred: CmpPred::Ne,
                    lhs: eq,
                    rhs: zero,
                });
                self.emit_end_catch();
                self.set_term(Term::BrIf {
                    cond: is_stop,
                    then_bb: end,
                    else_bb: reraise,
                });
                self.position(reraise);
                self.throwing_call(None, "zap_panic", vec![exc]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(end);
            }
        }
        self.pop_scope();
        Ok(())
    }

    /// Iterate any supported iterable `iter`, calling `body(self, value, ty)` for
    /// each element: `range(...)`, a `list`, a `set`, or a user struct via the
    /// D-0048 protocol. Used by comprehensions (and the struct case of `for`).
    fn iterate(
        &mut self,
        iter: &Expr,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        // `range(...)`: a counting loop over `[start, stop)` by `step`.
        if let Expr::Call { callee, args, .. } = iter {
            if is_name(callee, "range") {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                let (start, stop, step) = match vs.len() {
                    1 => (self.iconst(Ty::I64, 0), vs[0], self.iconst(Ty::I64, 1)),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1)),
                    3 => (vs[0], vs[1], vs[2]),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                };
                self.push_scope();
                let var = self.alloca(Ty::I64);
                self.store(var, start);
                let cond_bb = self.new_block();
                let body_bb = self.new_block();
                let incr_bb = self.new_block();
                let end_bb = self.new_block();
                self.set_term(Term::Br(cond_bb));
                self.position(cond_bb);
                let cur = self.load(var, Ty::I64);
                let c = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: c,
                    pred: CmpPred::Lt,
                    lhs: cur,
                    rhs: stop,
                });
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb: body_bb,
                    else_bb: end_bb,
                });
                self.position(body_bb);
                let cur_v = self.load(var, Ty::I64);
                self.loops.push((incr_bb, end_bb));
                body(self, cur_v, Ty::I64)?;
                self.loops.pop();
                self.ensure_br(incr_bb);
                self.position(incr_bb);
                let cur2 = self.load(var, Ty::I64);
                let next = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: next,
                    op: BinOp::Add,
                    lhs: cur2,
                    rhs: step,
                });
                self.store(var, next);
                self.set_term(Term::Br(cond_bb));
                self.position(end_bb);
                self.pop_scope();
                return Ok(());
            }
        }
        let (val, ty) = self.expr(iter)?;
        match ty {
            Ty::List(elem) => self.iterate_list(val, elem, body),
            Ty::Set(elem) => {
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_set_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            Ty::Heap(elem) => {
                // Iteration yields elements in priority (min-first) order without
                // draining the heap: `zap_heap_items` returns a sorted snapshot
                // list, which the list iterator then walks (D-0057).
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_heap_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            Ty::Struct(sid) => self.drive_iteration(val, sid, body),
            other => Err(err(format!(
                "value of type {other:?} is not iterable (D-0048)"
            ))),
        }
    }

    /// Iterate a `list[T]` value by index, re-reading the length each iteration.
    fn iterate_list(
        &mut self,
        lst: Value,
        elem: Elem,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let e = self.new_value(elem.ty());
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        self.loops.push((incr_bb, end_bb));
        body(self, e, elem.ty())?;
        self.loops.pop();
        self.ensure_br(incr_bb);
        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// `[elem for var in iter if cond]` (D-0048): build a fresh list, iterate the
    /// source through the general protocol, and push `elem` for each element that
    /// passes the optional filter.
    fn lower_list_comp(
        &mut self,
        elem_e: &Expr,
        var: &str,
        iter: &Expr,
        cond: Option<&Expr>,
    ) -> Result<(Value, Ty), LowerError> {
        let list = self.new_value(Ty::List(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let kind = std::cell::Cell::new(None);
        self.iterate(iter, |s, v, ty| {
            s.push_scope();
            let slot = s.alloca(ty);
            s.store(slot, v);
            s.declare(var, slot, ty);
            s.comp_guard(cond, |s| {
                let (ev, et) = s.expr(elem_e)?;
                let ek = elem_kind(et)?;
                kind.set(Some(ek));
                let ev = s.coerce(ev, et, ek.ty());
                let ev = s.box_if_value(ev, ek.ty());
                s.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_push_{}", ek.val_suffix()),
                    args: vec![list, ev],
                });
                Ok(())
            })?;
            s.pop_scope();
            Ok(())
        })?;
        let elem = kind.get().unwrap_or(Elem::I64);
        self.func.value_types[list as usize] = Ty::List(elem);
        Ok((list, Ty::List(elem)))
    }

    /// `{elem for var in iter if cond}` (D-0048): like [`lower_list_comp`] into a
    /// fresh set.
    fn lower_set_comp(
        &mut self,
        elem_e: &Expr,
        var: &str,
        iter: &Expr,
        cond: Option<&Expr>,
    ) -> Result<(Value, Ty), LowerError> {
        let set = self.new_value(Ty::Set(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let kind = std::cell::Cell::new(None);
        self.iterate(iter, |s, v, ty| {
            s.push_scope();
            let slot = s.alloca(ty);
            s.store(slot, v);
            s.declare(var, slot, ty);
            s.comp_guard(cond, |s| {
                let (ev, et) = s.expr(elem_e)?;
                let ek = elem_kind(et)?;
                kind.set(Some(ek));
                let ev = s.coerce(ev, et, ek.ty());
                let ev = s.box_if_value(ev, ek.ty());
                let kfns = s.key_fn_args(ek)?;
                let mut args = vec![set, ev];
                args.extend_from_slice(&kfns);
                s.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_set_add_{}", ek.key_suffix()),
                    args,
                });
                Ok(())
            })?;
            s.pop_scope();
            Ok(())
        })?;
        let elem = kind.get().unwrap_or(Elem::I64);
        self.func.value_types[set as usize] = Ty::Set(elem);
        Ok((set, Ty::Set(elem)))
    }

    /// `{key: value for var in iter if cond}` (D-0048): like [`lower_list_comp`]
    /// into a fresh dict.
    fn lower_dict_comp(
        &mut self,
        key_e: &Expr,
        val_e: &Expr,
        var: &str,
        iter: &Expr,
        cond: Option<&Expr>,
    ) -> Result<(Value, Ty), LowerError> {
        let dict = self.new_value(Ty::Dict(Elem::I64, Elem::I64));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        let kinds = std::cell::Cell::new(None);
        self.iterate(iter, |s, v, ty| {
            s.push_scope();
            let slot = s.alloca(ty);
            s.store(slot, v);
            s.declare(var, slot, ty);
            s.comp_guard(cond, |s| {
                let (kv, kt) = s.expr(key_e)?;
                let (vv, vt) = s.expr(val_e)?;
                let kk = elem_kind(kt)?;
                let vk = elem_kind(vt)?;
                kinds.set(Some((kk, vk)));
                let kv = s.coerce(kv, kt, kk.ty());
                let vv = s.coerce(vv, vt, vk.ty());
                let kv = s.box_if_value(kv, kk.ty());
                let vv = s.box_if_value(vv, vk.ty());
                let kfns = s.key_fn_args(kk)?;
                let mut args = vec![dict, kv, vv];
                args.extend_from_slice(&kfns);
                s.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args,
                });
                Ok(())
            })?;
            s.pop_scope();
            Ok(())
        })?;
        let (kk, vk) = kinds.get().unwrap_or((Elem::I64, Elem::I64));
        self.func.value_types[dict as usize] = Ty::Dict(kk, vk);
        Ok((dict, Ty::Dict(kk, vk)))
    }

    /// Emit `work` either unconditionally or guarded by `if cond:` (the optional
    /// comprehension filter).
    fn comp_guard(
        &mut self,
        cond: Option<&Expr>,
        work: impl FnOnce(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match cond {
            None => work(self),
            Some(c) => {
                let (cv, cty) = self.expr(c)?;
                let cc = self.truthy(cv, cty);
                let then_bb = self.new_block();
                let cont_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: cc,
                    then_bb,
                    else_bb: cont_bb,
                });
                self.position(then_bb);
                work(self)?;
                self.ensure_br(cont_bb);
                self.position(cont_bb);
                Ok(())
            }
        }
    }

    fn lower_for(
        &mut self,
        target: &Expr,
        iter: &Expr,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let (start, stop, step) = match iter {
            Expr::Call { callee, args, .. } if is_name(callee, "range") => {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                match vs.len() {
                    1 => (self.iconst(Ty::I64, 0), vs[0], self.iconst(Ty::I64, 1)),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1)),
                    3 => (vs[0], vs[1], vs[2]),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                }
            }
            _ => {
                // Iterating an enum *type* (`for col in Color:`, D-0018) is
                // typed by sema but its lowering is a documented follow-up
                // (it rides on the same enum-identity tracking as `.name`).
                if let Expr::Name(n, _) = iter {
                    if self.ctx.enums.contains_key(n) {
                        return Err(err(
                            "iterating an enum type is not implemented yet (deferred to a \
                             follow-up; D-0018)",
                        ));
                    }
                }
                // `for x in <list>` — iterate by index over the list's length.
                let (lst, lty) = self.expr(iter)?;
                if let Ty::List(elem) = lty {
                    return self.lower_for_list(target, lst, elem, body);
                }
                if let Ty::Set(elem) = lty {
                    return self.lower_for_set(target, lst, elem, body);
                }
                if let Ty::Heap(elem) = lty {
                    return self.lower_for_heap(target, lst, elem, body);
                }
                // `for x in <user iterable>` — the general D-0048 protocol.
                if let Ty::Struct(sid) = lty {
                    let name = match target {
                        Expr::Name(n, _) => n.clone(),
                        _ => {
                            return Err(err(
                                "only a simple loop variable is supported by codegen yet",
                            ))
                        }
                    };
                    return self.drive_iteration(lst, sid, |s, v, ty| {
                        let slot = s.alloca(ty);
                        s.store(slot, v);
                        s.declare(&name, slot, ty);
                        for st in &body.stmts {
                            s.stmt(st)?;
                        }
                        Ok(())
                    });
                }
                return Err(err(
                    "only `for ... in range(...)`, `for ... in <list>`, `for ... in <set>`, and \
                     `for ... in <user iterable>` are supported by codegen yet",
                ));
            }
        };
        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        };

        self.push_scope();
        let var = self.alloca(Ty::I64);
        self.store(var, start);
        self.declare(&name, var, Ty::I64);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(var, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: stop,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        self.loops.push((incr_bb, end_bb));
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(var, Ty::I64);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: step,
        });
        self.store(var, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// `for x in xs` where `xs : list[T]`. Re-reads the length each iteration so
    /// a body that appends behaves like a live iterator.
    fn lower_for_list(
        &mut self,
        target: &Expr,
        lst: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        };

        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let var = self.alloca(elem.ty());
        self.declare(&name, var, elem.ty());

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let e = self.new_value(elem.ty());
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        self.store(var, e);
        self.loops.push((incr_bb, end_bb));
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// `for x in s` where `s : set[T]`. Snapshots the set's live elements into a
    /// fresh list (sets are unordered; iteration order is unspecified) and then
    /// iterates that list.
    fn lower_for_set(
        &mut self,
        target: &Expr,
        set_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_set_items".to_string(),
            args: vec![set_val],
        });
        self.lower_for_list(target, items, elem, body)
    }

    /// `for x in <heap>` (D-0057): snapshot the heap into a list in priority
    /// (root-first) order via `zap_heap_items`, then iterate that list. The
    /// snapshot leaves the live heap untouched.
    fn lower_for_heap(
        &mut self,
        target: &Expr,
        heap_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_heap_items".to_string(),
            args: vec![heap_val],
        });
        self.lower_for_list(target, items, elem, body)
    }

    // --- expressions ---

    fn expr(&mut self, e: &Expr) -> Result<(Value, Ty), LowerError> {
        match e {
            Expr::Int(s, _) => {
                let v = parse_int_lit(s);
                Ok((self.iconst(Ty::I64, v), Ty::I64))
            }
            Expr::Float(s, _) => {
                let v: f64 = s.replace('_', "").parse().unwrap_or(0.0);
                Ok((self.fconst(Ty::F64, v), Ty::F64))
            }
            Expr::Bool(b, _) => {
                let d = self.new_value(Ty::I1);
                self.emit(Inst::BConst { dest: d, val: *b });
                Ok((d, Ty::I1))
            }
            Expr::Str {
                value,
                raw,
                bytes,
                fstring,
                ..
            } => {
                if *fstring {
                    return Err(err("f-strings are not supported by codegen yet"));
                }
                let data = if *raw {
                    value.clone().into_bytes()
                } else {
                    unescape(value)
                };
                let _ = bytes; // byte strings share the string object for now
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: d,
                    bytes: data,
                });
                Ok((d, Ty::Ptr))
            }
            Expr::Name(n, _) => {
                if let Some((ptr, ty)) = self.lookup(n) {
                    return Ok((self.load(ptr, ty), ty));
                }
                // A bare Python import alias (`import python "math" as m`) refers
                // to the module handle; importing is idempotent (CPython caches),
                // so a reference lowers to a `zap_py_import` (ticket #27, D-0012).
                if let Some(module) = self.ctx.py.get(n).cloned() {
                    return Ok((self.lower_py_import(&module), Ty::Py));
                }
                Err(err(format!("unknown variable '{n}'")))
            }
            Expr::Unary { op, expr, .. } => {
                let (v, t) = self.expr(expr)?;
                // Unary operator overloading on user structs (D-0047): `-a` ->
                // `a.__neg__()`, `+a` -> `a.__pos__()`, `~a` -> `a.__invert__()`.
                // `not a` goes through `truthy` (which calls `__bool__`).
                if let Ty::Struct(sid) = t {
                    match op {
                        AstUn::Neg => return self.call_dunder(sid, "__neg__", vec![(v, t)]),
                        AstUn::Pos => return self.call_dunder(sid, "__pos__", vec![(v, t)]),
                        AstUn::BitNot => return self.call_dunder(sid, "__invert__", vec![(v, t)]),
                        AstUn::Not => {}
                    }
                }
                match op {
                    AstUn::Neg => {
                        let d = self.new_value(t);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Neg,
                            val: v,
                        });
                        Ok((d, t))
                    }
                    AstUn::Pos => Ok((v, t)),
                    AstUn::Not => {
                        let b = self.truthy(v, t);
                        let d = self.new_value(Ty::I1);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Not,
                            val: b,
                        });
                        Ok((d, Ty::I1))
                    }
                    AstUn::BitNot => {
                        let d = self.new_value(t);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::BitNot,
                            val: v,
                        });
                        Ok((d, t))
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let (l, lt) = self.expr(lhs)?;
                let (r, rt) = self.expr(rhs)?;
                self.binop(*op, l, lt, r, rt)
            }
            Expr::Call { callee, args, .. } => self.call(callee, args),
            Expr::RegexMatch {
                subject,
                pattern,
                flags,
                negated,
                ..
            } => self.lower_regex_match(subject, pattern, flags, *negated),
            Expr::RegexSubst {
                subject,
                pattern,
                replacement,
                flags,
                ..
            } => self.lower_regex_subst(subject, pattern, replacement, flags),
            Expr::List(elems, _) => self.lower_list_lit(elems),
            Expr::Dict(items, _) => self.lower_dict_lit(items),
            Expr::Set(elems, _) => self.lower_set_lit(elems),
            Expr::Tuple(elems, _) => self.lower_tuple_lit(elems),
            Expr::ListComp {
                elem,
                var,
                iter,
                cond,
                ..
            } => self.lower_list_comp(elem, var, iter, cond.as_deref()),
            Expr::SetComp {
                elem,
                var,
                iter,
                cond,
                ..
            } => self.lower_set_comp(elem, var, iter, cond.as_deref()),
            Expr::DictComp {
                key,
                value,
                var,
                iter,
                cond,
                ..
            } => self.lower_dict_comp(key, value, var, iter, cond.as_deref()),
            Expr::Index { base, index, .. } => self.lower_index_get(base, index),
            Expr::Attr { base, name, .. } => {
                // Qualified enum member/variant access (`Color.RED`,
                // `Shape.Unit`): the base is a *type* name, resolved before it
                // would be (wrongly) evaluated as a value (D-0018/D-0046).
                if let Expr::Name(tyname, _) = base.as_ref() {
                    if self.ctx.enums.contains_key(tyname) {
                        return self.lower_enum_member(tyname, name);
                    }
                    // Built-in module constant `math.pi`-style access (ticket
                    // #98), intercepted before the base is lowered as a value
                    // (module names are not bindings). Not intercepted when
                    // shadowed by a local of the same name.
                    if self.lookup(tyname).is_none() {
                        if let Some(r) = modules::lower_builtin_module_const(self, tyname, name) {
                            return Ok(r);
                        }
                    }
                }
                let (bv, bty) = self.expr(base)?;
                // Attribute read on a Python handle, e.g. `np.pi` (ticket #27,
                // D-0012). A call `np.foo(...)` is intercepted in `call` before
                // reaching here, so this is a plain attribute access.
                if bty == Ty::Py {
                    return Ok((self.lower_py_getattr(bv, name), Ty::Py));
                }
                // `.name` on an enum value (D-0018) is typed by sema but its
                // lowering is a documented follow-up: a tier-1 enum collapses to
                // its backing `i64` here, losing the static enum identity the
                // member-name table lookup needs. (Preserving it needs a ZIR
                // enum type; tracked as a follow-up.)
                // The receiver type decides (I#141): a struct with a field
                // named `name` reads that field; the enum op applies only to a
                // non-struct (enum-backed) receiver.
                let Ty::Struct(sid) = bty else {
                    if name == "name" {
                        return Err(err(
                            "enum '.name' is not implemented yet (deferred to a follow-up; D-0018)",
                        ));
                    }
                    return Err(err("field access on a non-struct value"));
                };
                let (idx, fty) = self
                    .ctx
                    .field(sid, name)
                    .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                let d = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: d,
                    base: bv,
                    sid,
                    idx,
                });
                Ok((d, fty))
            }
            Expr::IfElse {
                cond, then, els, ..
            } => {
                let (cv, cty) = self.expr(cond)?;
                let c = self.truthy(cv, cty);
                let (a, at) = self.expr(then)?;
                let (b, _bt) = self.expr(els)?;
                let d = self.new_value(at);
                self.emit(Inst::Select {
                    dest: d,
                    cond: c,
                    a,
                    b,
                });
                Ok((d, at))
            }
            // Channel receive `<-ch` (ticket #25, D-0031): blocks until a value
            // is available, then yields it. The element kind carried by the
            // channel type picks the typed runtime entry point and the result
            // type. (The comma-ok `v, ok = <-ch` form and `select` are a
            // documented follow-up.)
            Expr::ChanRecv { chan, .. } => {
                let (cv, cty) = self.expr(chan)?;
                let Ty::Chan(elem) = cty else {
                    return Err(err("the operand of '<-' (receive) must be a channel"));
                };
                let ety = elem.ty();
                let d = self.new_value(ety);
                self.throwing_call(
                    Some(d),
                    &format!("zap_chan_recv_{}", elem.val_suffix()),
                    vec![cv],
                );
                Ok((d, ety))
            }
            // Statement-position `yield` is handled in `stmt`; reaching the
            // expression lowerer means a `yield` in subexpression position, which
            // v1 does not support (I#67). In a non-generator function a `yield`
            // cannot occur (sema makes any `def` with `yield` a generator).
            Expr::Yield { .. } if self.gen_entry.is_some() || self.gen_discover.is_some() => Err(
                err("'yield' is only supported as a statement in this version \
                     (I#67 v1): e.g. `yield x`, not inside a larger expression"),
            ),
            Expr::Yield { .. } => Err(err("'yield' outside a generator function (I#67)")),
            _ => Err(err(
                "this expression is not supported by codegen yet (Phase 1 slice)",
            )),
        }
    }

    /// Lower a statement-position `yield <value>` inside a generator (I#67).
    ///
    /// During **discovery** this only records the operand's type (a component of
    /// the element type `T`). During **emit** it is the generator's suspend
    /// point: store every live local back to its state-struct field, advance the
    /// `$state` discriminant, return `(value, true)`, and continue lowering in a
    /// fresh **resume block** that `__next__`'s entry dispatch will jump to.
    fn lower_yield_stmt(&mut self, value: Option<&Expr>) -> Result<(), LowerError> {
        if self.gen_discover.is_some() {
            let t = match value {
                Some(v) => self.expr(v)?.1,
                None => Ty::Void,
            };
            self.gen_elems.push(t);
            return Ok(());
        }
        if self.gen_entry.is_none() {
            return Err(err("'yield' outside a generator function (I#67)"));
        }
        let value = value.ok_or_else(|| err("'yield' requires a value (I#67 v1)"))?;

        let (self_val, sid, next_tuple_sid, elem) = {
            let ge = self.gen_entry.as_ref().unwrap();
            (ge.self_val, ge.sid, ge.next_tuple_sid, ge.elem)
        };
        let (v, vt) = self.expr(value)?;
        let v = self.coerce(v, vt, elem);

        // Persist every live local (and the parameters) to its field, so the
        // next `__next__` resumes with the same values (I#67).
        let order = self.gen_entry.as_ref().unwrap().order.clone();
        for name in &order {
            let (slot, ty) = self.gen_entry.as_ref().unwrap().slots[name];
            let idx = self.gen_entry.as_ref().unwrap().field_idx[name];
            let cur = self.load(slot, ty);
            self.emit(Inst::FieldSet {
                base: self_val,
                sid,
                idx,
                val: cur,
            });
        }

        // Advance the resume discriminant and return `(value, true)`.
        let k = {
            let ge = self.gen_entry.as_mut().unwrap();
            let k = ge.next_state;
            ge.next_state += 1;
            k
        };
        let kc = self.iconst(Ty::I64, k);
        self.emit(Inst::FieldSet {
            base: self_val,
            sid,
            idx: 0,
            val: kc,
        });
        let tup = self.new_value(Ty::Struct(next_tuple_sid));
        self.emit(Inst::StructNew {
            dest: tup,
            sid: next_tuple_sid,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid: next_tuple_sid,
            idx: 0,
            val: v,
        });
        let yes = self.bconst(true);
        self.emit(Inst::FieldSet {
            base: tup,
            sid: next_tuple_sid,
            idx: 1,
            val: yes,
        });
        self.set_term(Term::Ret(Some(tup)));

        // Continue in the resume block; `__next__`'s dispatch branches here when
        // `$state == k`.
        let resume = self.new_block();
        self.gen_entry.as_mut().unwrap().resume.push((k, resume));
        self.position(resume);
        Ok(())
    }

    fn call(&mut self, callee: &Expr, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        // Qualified tier-3 variant construction `Shape.Circle(2.0)` (D-0046):
        // the callee is `Enum.Variant`, intercepted before the plain-name and
        // method-call dispatch.
        if let Expr::Attr { base, name, .. } = callee {
            if let Expr::Name(tyname, _) = base.as_ref() {
                if self.ctx.enums.contains_key(tyname) {
                    return self.lower_variant_construct(tyname, name, args);
                }
                // Built-in `signal`/`gc`/`runtime` module calls (ticket #77,
                // D-0058), intercepted before the base is lowered as a value
                // (these module names are not value bindings). Not intercepted
                // when shadowed by a local of the same name.
                if self.lookup(tyname).is_none() {
                    if let Some(r) = modules::lower_builtin_module_call(self, tyname, name, args)? {
                        return Ok(r);
                    }
                }
                // `Struct.from_json(s)` / `Struct.from_csv(s)` — the derived
                // decode associated functions (D-0059, I#100/I#132). Dispatched
                // when the generated `{Struct}__from_<fmt>` was registered (i.e.
                // the struct derived that codec); sema type-checked the call.
                if name == "from_json" || name == "from_csv" || name == "from_yaml" {
                    let fnname = format!("{tyname}__{name}");
                    if self.sigs.contains_key(&fnname) {
                        let (s, _) = self.expr(&args[0].value)?;
                        let sid = *self
                            .ctx
                            .ids
                            .get(tyname)
                            .ok_or_else(|| err("unknown struct in from_json"))?;
                        let d = self.new_value(Ty::Struct(sid));
                        self.emit(Inst::Call {
                            dest: Some(d),
                            func: fnname,
                            args: vec![s],
                        });
                        return Ok((d, Ty::Struct(sid)));
                    }
                }
            }
        }
        // Array construction `array[dtype]([...])` (ticket #24). The callee
        // parses as a subscript `array[dtype]`, so it is intercepted here before
        // the plain-name dispatch below.
        if let Expr::Index { base, index, .. } = callee {
            if is_name(base, "array") {
                return self.lower_array_new(index, args);
            }
            if is_name(base, "channel") {
                return self.lower_chan_new(index, args);
            }
            if is_name(base, "heap") {
                return self.lower_heap_new(index, args);
            }
        }
        if let Expr::Name(fname, _) = callee {
            if fname == "sum" {
                if args.len() != 1 {
                    return Err(err("sum() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let Ty::Array(d) = vt else {
                    return Err(err("sum() expects an array[T] value"));
                };
                let dest = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_array_sum_{}", d.suffix()),
                    args: vec![v],
                });
                return Ok((dest, d.ty()));
            }
            if fname == "print" {
                let mut vals = Vec::new();
                for a in args {
                    let (v, vt) = self.expr(&a.value)?;
                    // A Python handle prints via its `str()` (ticket #27,
                    // D-0012): marshal it to a Zap string first so codegen's
                    // `Print` renders it like any other string.
                    if vt == Ty::Py {
                        let s = self.new_value(Ty::Ptr);
                        self.emit(Inst::Call {
                            dest: Some(s),
                            func: "zap_py_str".to_string(),
                            args: vec![v],
                        });
                        vals.push(s);
                    } else if let Ty::Struct(sid) = vt {
                        // A struct prints through its `__str__`/`__repr__`
                        // (D-0047): marshal to a string first so codegen's
                        // `Print` renders it like any other string.
                        let s = self.struct_to_str(sid, v)?;
                        vals.push(s);
                    } else {
                        vals.push(v);
                    }
                }
                self.emit(Inst::Print { args: vals });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            if fname == "range" {
                return Err(err("range() can only be used in a for-loop (Phase 1)"));
            }
            // `close(ch)` (ticket #25, D-0031): mark a channel closed. After this,
            // a send raises and a receive drains then reports not-ok.
            if fname == "close" {
                if args.len() != 1 {
                    return Err(err("close() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if !matches!(vt, Ty::Chan(_)) {
                    return Err(err("close() expects a channel"));
                }
                self.throwing_call(None, "zap_chan_close", vec![v]);
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            // `checkpoint()` (D-0044): a cooperative cancellation point for long
            // CPU-bound loops. It yields the coroutine and, if its owning `scope:`
            // has been cancelled, raises `Cancelled` — so it may unwind.
            if fname == "checkpoint" {
                if !args.is_empty() {
                    return Err(err("checkpoint() takes no arguments"));
                }
                self.throwing_call(None, "zap_checkpoint", vec![]);
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            // `__netpoll_selftest()` (ticket #80): internal netpoller
            // integration-test hook; returns the number of sub-checks that
            // passed. No arguments, no public I/O surface yet (sockets #79).
            if fname == "__netpoll_selftest" {
                if !args.is_empty() {
                    return Err(err("__netpoll_selftest() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_netpoll_selftest".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__signal_selftest()` (ticket #77): internal signal-subsystem
            // integration-test hook; returns the number of sub-checks that passed.
            if fname == "__signal_selftest" {
                if !args.is_empty() {
                    return Err(err("__signal_selftest() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_signal_selftest".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__gc_finalizer_selftest()` (ticket #92): internal finalizer
            // integration-test hook; returns the number of finalizers that ran.
            if fname == "__gc_finalizer_selftest" {
                if !args.is_empty() {
                    return Err(err("__gc_finalizer_selftest() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_gc_finalizer_selftest".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__raise_signal(n)` (ticket #77): internal test hook; send signal n
            // to this process so a golden can trigger delivery deterministically.
            if fname == "__raise_signal" {
                if args.len() != 1 {
                    return Err(err("__raise_signal(n) takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let n = self.coerce(v, vt, Ty::I64);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_signal_raise".to_string(),
                    args: vec![n],
                });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            // `__signal_drain()` (ticket #77): block until the signal thread has
            // dispatched everything raised so far (deterministic goldens).
            if fname == "__signal_drain" {
                if !args.is_empty() {
                    return Err(err("__signal_drain() takes no arguments"));
                }
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_signal_drain".to_string(),
                    args: vec![],
                });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            if fname == "len" {
                if args.len() != 1 {
                    return Err(err("len() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // Size overloading (D-0047): `len(a)` -> `a.__len__()`.
                if let Ty::Struct(sid) = vt {
                    return self.call_dunder(sid, "__len__", vec![(v, vt)]);
                }
                let func = match vt {
                    Ty::List(_) => "zap_list_len",
                    Ty::Dict(..) => "zap_dict_len",
                    Ty::Set(_) => "zap_set_len",
                    Ty::Heap(_) => "zap_heap_len",
                    Ty::Array(_) => "zap_array_len",
                    Ty::Ptr => "zap_str_len",
                    _ => return Err(err("len() expects a list, dict, set, heap, array, or str")),
                };
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![v],
                });
                return Ok((d, Ty::I64));
            }
            // Explicit numeric conversion: `int32(x)`, `float64(x)`, ... (D-0020).
            // These are the way to narrow or cross the int/float boundary, which
            // implicit coercion refuses.
            if let Some(to) = numeric_conv_ty(fname) {
                if args.len() != 1 {
                    return Err(err(format!("{fname}() takes exactly one argument")));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // Marshal a Python handle back to a Zap scalar (ticket #27,
                // D-0012): `int64(x)` / `float64(x)` are the conversion surface
                // the design uses (`print(int64(b))`). Route through the runtime
                // marshaler (i64 or f64), then coerce to the requested width.
                if vt == Ty::Py {
                    let (helper, mid) = if to.is_float() {
                        ("zap_py_to_f64", Ty::F64)
                    } else {
                        ("zap_py_to_i64", Ty::I64)
                    };
                    let raw = self.new_value(mid);
                    self.throwing_call(Some(raw), helper, vec![v]);
                    return Ok((self.convert(raw, mid, to), to));
                }
                if !(vt.is_float() || vt.int_width().is_some()) {
                    return Err(err(format!("{fname}() expects a numeric value")));
                }
                return Ok((self.convert(v, vt, to), to));
            }
            // Runtime reflection (ticket #16, D-0036): `reflect_has(type, name)`
            // and `reflect_names(type)` query the metadata registry populated at
            // the top of `main`. Both take string arguments.
            if fname == "reflect_has" || fname == "reflect_names" {
                return self.lower_reflect_query(fname, args);
            }
            // Tier-1 enum conversion `Color(x)` (D-0018/D-0020): the enum is its
            // backing `i64`, so this is just a coercion of the int argument.
            if let Some(info) = self.ctx.enums.get(fname) {
                if !info.is_sum {
                    if args.len() != 1 {
                        return Err(err(format!("{fname}() takes exactly one argument")));
                    }
                    let (v, vt) = self.expr(&args[0].value)?;
                    return Ok((self.coerce(v, vt, Ty::I64), Ty::I64));
                }
            }
            // Struct construction: `StructName(fields...)`.
            // A generator call `gen(args)` (I#67) constructs the compiler-
            // generated state struct — capturing the arguments in its parameter
            // fields and starting at `$state == 0` — rather than calling a
            // function; the original body was compiled into the state struct's
            // `__next__`.
            if let Some(gi) = self.ctx.generators.get(fname).cloned() {
                return self.construct_generator(&gi, args);
            }
            if let Some(&sid) = self.ctx.ids.get(fname) {
                return self.construct(sid, args);
            }
            if let Some((ptys, ret)) = self.sigs.get(fname).cloned() {
                let mut argvals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let (v, vt) = self.expr(&a.value)?;
                    let target = *ptys.get(i).unwrap_or(&vt);
                    let v = self.coerce(v, vt, target);
                    // A value struct passed by value is copied (D-0051): the
                    // callee's mutations do not escape to the caller. An inline-
                    // eligible aggregate gets that copy from the register/sret
                    // ABI; a non-inline value struct is deep-copied. See
                    // `copy_for_pass`.
                    let v = self.copy_for_pass(v, target, &a.value);
                    argvals.push(v);
                }
                let dest = if ret == Ty::Void {
                    None
                } else {
                    Some(self.new_value(ret))
                };
                self.throwing_call(dest, fname, argvals);
                let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
                return Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }));
            }
            // A callable user struct held in a variable (D-0047): `f(args)` where
            // `f` is a struct value desugars to `f.__call__(args)`. `lookup`
            // yields the variable's stack slot, so the receiver must be loaded.
            if let Some((slot, Ty::Struct(sid))) = self.lookup(fname) {
                let recv = self.load(slot, Ty::Struct(sid));
                let mut dargs = vec![(recv, Ty::Struct(sid))];
                for a in args {
                    dargs.push(self.expr(&a.value)?);
                }
                return self.call_dunder(sid, "__call__", dargs);
            }
            // A Python callable held in a variable (`auto f = m.sqrt; f(2.0)`)
            // or an imported module name used as a callable (ticket #27, D-0012).
            if matches!(self.lookup(fname), Some((_, Ty::Py))) {
                let (callable, _) = self.expr(callee)?;
                return self.lower_py_call(callable, args);
            }
            if let Some(module) = self.ctx.py.get(fname).cloned() {
                let callable = self.lower_py_import(&module);
                return self.lower_py_call(callable, args);
            }
            Err(err(format!("call to unknown function '{fname}'")))
        } else if let Expr::Attr { base, name, .. } = callee {
            // Method call: `recv.method(args)`.
            let (recv, bty) = self.expr(base)?;
            if let Ty::List(elem) = bty {
                return self.lower_list_method(recv, elem, name, args);
            }
            if let Ty::Set(elem) = bty {
                return self.lower_set_method(recv, elem, name, args);
            }
            if let Ty::Heap(elem) = bty {
                return self.lower_heap_method(recv, elem, name, args);
            }
            if bty == Ty::Match {
                return self.lower_match_method(recv, name, args);
            }
            if bty == Ty::Py {
                // `obj.method(args)` on a Python handle (ticket #27, D-0012):
                // read the bound attribute, then call it with marshaled args.
                let callable = self.lower_py_getattr(recv, name);
                return self.lower_py_call(callable, args);
            }
            let Ty::Struct(sid) = bty else {
                return Err(err("method call on a non-struct value"));
            };
            // Dynamic dispatch through an interface value (D-0016): the receiver
            // is an interface fat pointer, not a concrete struct.
            if let Some(iid) = self.ctx.iid_of_fat_sid(sid) {
                return self.lower_iface_call(recv, iid, name, args);
            }
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], name);
            let (ptys, ret) = self
                .sigs
                .get(&mangled)
                .cloned()
                .ok_or_else(|| err(format!("no method '{name}' on that struct")))?;
            // `self` is passed by reference (a value-type mutating method acts
            // on the receiver variable, Swift-style); the remaining arguments
            // are copied when they are value structs (D-0051). An inline-eligible
            // argument is copied by the register/sret ABI, so no explicit copy is
            // emitted for it (see `copy_for_pass`).
            let mut argvals = vec![recv]; // self
            for (i, a) in args.iter().enumerate() {
                let (v, vt) = self.expr(&a.value)?;
                let target = *ptys.get(i + 1).unwrap_or(&vt);
                let v = self.coerce(v, vt, target);
                let v = self.copy_for_pass(v, target, &a.value);
                argvals.push(v);
            }
            let dest = if ret == Ty::Void {
                None
            } else {
                Some(self.new_value(ret))
            };
            self.throwing_call(dest, &mangled, argvals);
            let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
            Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }))
        } else {
            Err(err(
                "only direct function calls are supported by codegen yet",
            ))
        }
    }

    /// Lower a method call on an interface-typed receiver to **dynamic dispatch**
    /// (D-0016): resolve the method's slot in the interface's dispatch table and
    /// emit an [`Inst::IfaceCall`] / [`Inst::IfaceInvoke`] that loads the slot
    /// from the fat pointer's itable and calls it with `data` as `self`. The
    /// callee's signature is the interface's declared one, so arguments are
    /// coerced to the interface method's parameter types.
    fn lower_iface_call(
        &mut self,
        iface: Value,
        iid: u32,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let iface_info = &self.ctx.interfaces[iid as usize];
        let method = iface_info
            .methods
            .iter()
            .position(|m| m.name == name)
            .ok_or_else(|| {
                err(format!(
                    "interface '{}' has no method '{name}'",
                    iface_info.name
                ))
            })? as u32;
        // Clone the slot signature (ends the borrow of `self.ctx`).
        let msig = self.ctx.interfaces[iid as usize].methods[method as usize].clone();
        let mut argvals = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let (v, vt) = self.expr(&a.value)?;
            let target = *msig.params.get(i).unwrap_or(&vt);
            let v = self.coerce(v, vt, target);
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        let dest = if msig.ret == Ty::Void {
            None
        } else {
            Some(self.new_value(msig.ret))
        };
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::IfaceInvoke {
                dest,
                iface,
                iid,
                method,
                args: argvals,
                unwind: lp,
            });
        } else {
            self.emit(Inst::IfaceCall {
                dest,
                iface,
                iid,
                method,
                args: argvals,
            });
        }
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((
            rv,
            if msig.ret == Ty::Void {
                Ty::I32
            } else {
                msig.ret
            },
        ))
    }

    fn construct(&mut self, sid: u32, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (i, a) in args.iter().enumerate() {
            let (idx, fty) = match &a.name {
                Some(fname) => self
                    .ctx
                    .field(sid, fname)
                    .ok_or_else(|| err(format!("no field '{fname}' on struct")))?,
                None => {
                    let f = self.ctx.fields[sid as usize]
                        .get(i)
                        .ok_or_else(|| err("too many fields in constructor"))?;
                    (i as u32, f.1)
                }
            };
            let (v, vt) = self.expr(&a.value)?;
            let v = self.coerce(v, vt, fty);
            // A value-struct field is copied in (D-0051): the new struct holds an
            // independent instance of the aggregate.
            let v = self.copy_if_value(v, fty, &a.value);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Construct a generator's state struct for a call `gen(args)` (I#67).
    ///
    /// Allocates the (zeroed → `$state == 0`) state struct and stores each
    /// argument into its parameter field, so the first `__next__` starts the body
    /// from the top with the captured arguments. Arguments may be positional or
    /// keyword (matched to parameter names), mirroring an ordinary call.
    fn construct_generator(
        &mut self,
        gi: &GeneratorInfo,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.len() != gi.n_params {
            return Err(err(format!(
                "generator '{}' expects {} argument(s), got {}",
                gi.name,
                gi.n_params,
                args.len()
            )));
        }
        let sid = gi.sid;
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (i, a) in args.iter().enumerate() {
            let p = match &a.name {
                Some(kw) => gi.vars[..gi.n_params]
                    .iter()
                    .position(|(n, _)| n == kw)
                    .ok_or_else(|| {
                        err(format!("generator '{}' has no parameter '{kw}'", gi.name))
                    })?,
                None => i,
            };
            let pty = gi.vars[p].1;
            let (v, vt) = self.expr(&a.value)?;
            let v = self.coerce(v, vt, pty);
            // Capturing a value-struct argument copies it (D-0051), like any
            // by-value parameter pass.
            let v = self.copy_for_pass(v, pty, &a.value);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: gen_var_field(p),
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    // --- enums (ticket #51, D-0018/D-0046) ---
    //
    // A tier-1 enum is its unboxed backing `i64`; a member access lowers to the
    // discriminant constant. A tier-3 enum is a GC-allocated tagged struct
    // `{ tag:i32, payload… }`; a variant construction allocates it, stores the
    // tag, and stores each payload field — reusing the struct machinery, so a
    // recursive variant field is simply a pointer to a heap enum value (which
    // breaks the otherwise-infinite size, D-0046). `match` branches on the
    // backing integer (tier 1) or the tag field (tier 3).

    /// Lower a qualified member/variant access with no call: `Color.RED`
    /// (tier-1 constant) or `Shape.Unit` (tier-3 payload-free variant).
    fn lower_enum_member(
        &mut self,
        enum_name: &str,
        member: &str,
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .get(enum_name)
            .cloned()
            .expect("caller checked this is an enum");
        if !info.is_sum {
            let val = info
                .member_value(member)
                .ok_or_else(|| err(format!("enum '{enum_name}' has no member '{member}'")))?;
            return Ok((self.iconst(Ty::I64, val), Ty::I64));
        }
        // Tier-3: a bare variant must be payload-free (data-carrying variants are
        // constructed through `call`).
        let v = info
            .variant(member)
            .ok_or_else(|| err(format!("enum '{enum_name}' has no variant '{member}'")))?
            .clone();
        if !v.fields.is_empty() {
            return Err(err(format!(
                "variant '{enum_name}.{member}' carries data; construct it with \
                 '{enum_name}.{member}(...)'"
            )));
        }
        let sid = info.sid.expect("tier-3 enum has a struct id");
        self.build_variant(sid, &v, &[])
    }

    /// Lower a qualified variant construction `Enum.Variant(args)` (D-0046):
    /// allocate the tagged struct, store the tag, and store each payload field
    /// (positional or named). Returns the enum value (a `Struct` pointer).
    fn lower_variant_construct(
        &mut self,
        enum_name: &str,
        variant: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .get(enum_name)
            .cloned()
            .expect("caller checked this is an enum");
        if !info.is_sum {
            return Err(err(format!(
                "'{enum_name}' is a tier-1 enum; '{enum_name}.{variant}(...)' is not a \
                 constructor"
            )));
        }
        let v = info
            .variant(variant)
            .ok_or_else(|| err(format!("enum '{enum_name}' has no variant '{variant}'")))?
            .clone();
        let sid = info.sid.expect("tier-3 enum has a struct id");
        self.build_variant(sid, &v, args)
    }

    /// Allocate a tier-3 tagged struct for `variant` and fill the tag slot and
    /// its payload fields from `args` (named or positional, declaration order).
    fn build_variant(
        &mut self,
        sid: u32,
        variant: &VariantLow,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        // Tag slot (field 0).
        let tag = self.iconst(Ty::I32, variant.tag);
        self.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: 0,
            val: tag,
        });
        // Payload fields.
        let mut pos = 0usize;
        for a in args {
            let (fidx, fty) = match &a.name {
                Some(fname) => variant
                    .fields
                    .iter()
                    .find(|(n, _, _)| n == fname)
                    .map(|(_, idx, t)| (*idx, *t))
                    .ok_or_else(|| {
                        err(format!("variant '{}' has no field '{fname}'", variant.name))
                    })?,
                None => {
                    let f = variant
                        .fields
                        .get(pos)
                        .ok_or_else(|| err("too many fields in variant constructor"))?;
                    pos += 1;
                    (f.1, f.2)
                }
            };
            let (val, vt) = self.expr(&a.value)?;
            let val = self.coerce(val, vt, fty);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: fidx,
                val,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Lower a `match` statement (D-0040/D-0046). The scrutinee is evaluated
    /// once; arms are tested top-to-bottom like an `if`/`elif` chain. The test
    /// compares the backing integer (tier-1 enum / literal) or the tag field
    /// (tier-3 enum); a matching arm binds its captures/payload fields, checks
    /// any guard, and runs its body. Exhaustiveness is already enforced by sema.
    fn lower_match(&mut self, scrutinee: &Expr, arms: &[ast::MatchArm]) -> Result<(), LowerError> {
        let (sv, sty) = self.expr(scrutinee)?;
        let merge = self.new_block();
        for arm in arms {
            let (cond, _catch_all) = self.pattern_cond(sv, sty, &arm.pattern)?;
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.push_scope();
            self.bind_pattern(sv, sty, &arm.pattern)?;
            if let Some(g) = &arm.guard {
                // A guard runs after binding; on failure fall through to the
                // next arm (it does not count toward exhaustiveness — sema).
                let (gv, gty) = self.expr(g)?;
                let gc = self.truthy(gv, gty);
                let body_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: gc,
                    then_bb: body_bb,
                    else_bb: next_bb,
                });
                self.position(body_bb);
            }
            self.lower_block(&arm.body)?;
            self.ensure_br(merge);
            self.pop_scope();
            self.position(next_bb);
        }
        // Fall-through (no arm matched): for a non-enum scrutinee this is the
        // Python no-op; for an exhaustive enum sema guarantees this is dead.
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    /// Emit the test for one pattern against the scrutinee and return its `i1`
    /// condition plus whether the pattern is an irrefutable catch-all.
    fn pattern_cond(
        &mut self,
        sv: Value,
        sty: Ty,
        pat: &Pattern,
    ) -> Result<(Value, bool), LowerError> {
        match pat {
            Pattern::Wildcard(_) | Pattern::Capture { .. } => Ok((self.bconst(true), true)),
            Pattern::Literal(e) => {
                let (lv, lt) = self.expr(e)?;
                let (c, _) = self.binop(AstBin::Eq, sv, sty, lv, lt)?;
                Ok((c, false))
            }
            Pattern::Or(ps, _) => {
                let mut acc: Option<Value> = None;
                let mut catch_all = false;
                for sub in ps {
                    let (c, ca) = self.pattern_cond(sv, sty, sub)?;
                    catch_all |= ca;
                    acc = Some(match acc {
                        None => c,
                        Some(prev) => {
                            let d = self.new_value(Ty::I1);
                            self.emit(Inst::Bin {
                                dest: d,
                                op: BinOp::Or,
                                lhs: prev,
                                rhs: c,
                            });
                            d
                        }
                    });
                }
                Ok((acc.unwrap_or_else(|| self.bconst(false)), catch_all))
            }
            Pattern::Struct {
                path,
                args,
                span: _,
            } => self.variant_cond(sv, sty, path, args.as_deref()),
            Pattern::Tuple(..) | Pattern::Sequence(..) | Pattern::Rest { .. } => Err(err(
                "tuple/sequence patterns in match are not supported yet (Phase 1 slice)",
            )),
        }
    }

    /// The test for a qualified enum pattern (`Color.RED`, `Shape.Circle(r)`):
    /// compare the backing integer (tier 1) or the tag field (tier 3).
    fn variant_cond(
        &mut self,
        sv: Value,
        sty: Ty,
        path: &[String],
        _args: Option<&[PatArg]>,
    ) -> Result<(Value, bool), LowerError> {
        if path.len() != 2 {
            return Err(err(
                "only qualified enum patterns (Enum.Variant) are supported in match",
            ));
        }
        let info = self
            .ctx
            .enums
            .get(&path[0])
            .cloned()
            .ok_or_else(|| err(format!("'{}' is not an enum", path[0])))?;
        if !info.is_sum {
            let val = info
                .member_value(&path[1])
                .ok_or_else(|| err(format!("enum '{}' has no member '{}'", path[0], path[1])))?;
            let k = self.iconst(Ty::I64, val);
            let (c, _) = self.binop(AstBin::Eq, sv, sty, k, Ty::I64)?;
            Ok((c, false))
        } else {
            let v = info
                .variant(&path[1])
                .ok_or_else(|| err(format!("enum '{}' has no variant '{}'", path[0], path[1])))?;
            let sid = info.sid.expect("tier-3 enum has a struct id");
            let tagv = self.new_value(Ty::I32);
            self.emit(Inst::FieldGet {
                dest: tagv,
                base: sv,
                sid,
                idx: 0,
            });
            let k = self.iconst(Ty::I32, v.tag);
            let (c, _) = self.binop(AstBin::Eq, tagv, Ty::I32, k, Ty::I32)?;
            Ok((c, false))
        }
    }

    /// Declare the bindings a matched pattern introduces into the current scope.
    fn bind_pattern(&mut self, sv: Value, sty: Ty, pat: &Pattern) -> Result<(), LowerError> {
        match pat {
            Pattern::Capture { name, .. } => {
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::Struct {
                path,
                args: Some(args),
                ..
            } if path.len() == 2 => {
                let info = self
                    .ctx
                    .enums
                    .get(&path[0])
                    .cloned()
                    .ok_or_else(|| err(format!("'{}' is not an enum", path[0])))?;
                let Some(v) = info.variant(&path[1]).cloned() else {
                    return Ok(());
                };
                let sid = info.sid.expect("tier-3 enum has a struct id");
                let mut pos = 0usize;
                for a in args {
                    let field = match &a.name {
                        Some(fname) => v.fields.iter().find(|(n, _, _)| n == fname).cloned(),
                        None => {
                            let f = v.fields.get(pos).cloned();
                            pos += 1;
                            f
                        }
                    };
                    let Some((_, fidx, fty)) = field else {
                        continue;
                    };
                    self.bind_field(sv, sid, fidx, fty, &a.pattern)?;
                }
                Ok(())
            }
            // Payload-free qualified constant (`Color.RED`, `Shape.Unit`) and
            // other refutable leaves bind nothing.
            _ => Ok(()),
        }
    }

    /// Bind one payload field's value to the capture in its sub-pattern.
    fn bind_field(
        &mut self,
        base: Value,
        sid: u32,
        fidx: u32,
        fty: Ty,
        sub: &Pattern,
    ) -> Result<(), LowerError> {
        match sub {
            Pattern::Wildcard(_) => Ok(()),
            Pattern::Capture { name, .. } => {
                let fv = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: fv,
                    base,
                    sid,
                    idx: fidx,
                });
                let slot = self.alloca(fty);
                self.store(slot, fv);
                self.declare(name, slot, fty);
                Ok(())
            }
            _ => Err(err(
                "nested patterns inside a variant field are not supported yet (bind to a \
                 name or use '_')",
            )),
        }
    }

    // --- collections (ticket #21) ---
    //
    // Lists and dicts lower to runtime calls (like strings), so the ZIR -> LLVM
    // step stays mechanical. The element/key/value `Elem` kind chosen here
    // selects the typed entry point (`..._i64` / `..._ptr` / `..._str`).

    // --- numeric arrays (ticket #24) ---
    //
    // Construction, element access, `len`, and `sum` lower to runtime calls
    // (like collections). Only the element-wise ops (`binop`) use a dedicated
    // ZIR node so codegen can emit SIMD; everything else stays a plain call.

    /// `array[dtype]([e0, e1, ...])` — allocate an array of the literal's length
    /// and fill it element-by-element. N-D literals, non-literal arguments
    /// (`array[f64](xs)`), and `zeros`/`arange` constructors are deferred.
    fn lower_array_new(
        &mut self,
        dtype_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let Expr::Name(dname, _) = dtype_expr else {
            return Err(err("array[...] expects a dtype name, e.g. array[f64]"));
        };
        let dtype = dtype_from_name(dname)?;
        if args.len() != 1 {
            return Err(err(
                "array[T](...) takes exactly one argument (a list literal)",
            ));
        }
        let Expr::List(elems, _) = &args[0].value else {
            return Err(err(
                "array[T](...) takes a list literal, e.g. array[f64]([1.0, 2.0, 3.0]) \
                 (N-D and non-literal construction are not supported yet)",
            ));
        };
        let code = self.iconst(Ty::I64, dtype.code());
        let n = self.iconst(Ty::I64, elems.len() as i64);
        let arr = self.new_value(Ty::Array(dtype));
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_new".to_string(),
            args: vec![code, n],
        });
        let setter = format!("zap_array_set_{}", dtype.suffix());
        for (i, e) in elems.iter().enumerate() {
            let (v, vt) = self.expr(e)?;
            let v = self.coerce(v, vt, dtype.ty());
            let idx = self.iconst(Ty::I64, i as i64);
            self.emit(Inst::Call {
                dest: None,
                func: setter.clone(),
                args: vec![arr, idx, v],
            });
        }
        Ok((arr, Ty::Array(dtype)))
    }

    /// Lower channel construction `channel[T]()` (unbuffered) or
    /// `channel[T](cap)` (buffered) (ticket #25, D-0031). The runtime stores
    /// elements as uniform 8-byte words, so construction only needs the
    /// capacity; the element kind `T` is carried in the ZIR [`Ty::Chan`] so
    /// later send/recv pick the typed entry point. An absent capacity means an
    /// unbuffered (rendezvous) channel.
    fn lower_chan_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let Expr::Name(ename, _) = elem_expr else {
            return Err(err(
                "channel[...] expects an element type name, e.g. channel[int64]",
            ));
        };
        let elem = chan_elem_from_name(ename)?;
        let cap = match args.len() {
            0 => self.iconst(Ty::I64, 0),
            1 => {
                let (v, vt) = self.expr(&args[0].value)?;
                self.coerce(v, vt, Ty::I64)
            }
            _ => {
                return Err(err(
                    "channel[T](...) takes at most one argument (the buffer capacity)",
                ))
            }
        };
        let ch = self.new_value(Ty::Chan(elem));
        self.emit(Inst::Call {
            dest: Some(ch),
            func: "zap_chan_new".to_string(),
            args: vec![cap],
        });
        Ok((ch, Ty::Chan(elem)))
    }

    fn lower_list_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        // Evaluate elements left-to-right first; the first fixes the element
        // kind (an empty literal defaults to `i64`).
        let mut lowered = Vec::with_capacity(elems.len());
        for e in elems {
            lowered.push(self.expr(e)?);
        }
        let elem = match lowered.first() {
            Some(&(_, t)) => elem_kind(t)?,
            None => Elem::I64,
        };
        let list = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let push = format!("zap_list_push_{}", elem.val_suffix());
        for (v, vt) in lowered {
            if elem_kind(vt)? != elem {
                return Err(err("list elements must all have the same representation"));
            }
            let v = self.coerce(v, vt, elem.ty());
            let v = self.box_if_value(v, elem.ty());
            self.emit(Inst::Call {
                dest: None,
                func: push.clone(),
                args: vec![list, v],
            });
        }
        Ok((list, Ty::List(elem)))
    }

    fn lower_dict_lit(&mut self, items: &[(Expr, Expr)]) -> Result<(Value, Ty), LowerError> {
        let mut lowered = Vec::with_capacity(items.len());
        for (k, v) in items {
            let kv = self.expr(k)?;
            let vv = self.expr(v)?;
            lowered.push((kv, vv));
        }
        let (kk, vk) = match lowered.first() {
            Some(&((_, kt), (_, vt))) => (elem_kind(kt)?, elem_kind(vt)?),
            None => (Elem::I64, Elem::I64),
        };
        let dict = self.new_value(Ty::Dict(kk, vk));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        let set = format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix());
        // Struct keys hash/compare through their own `__hash__`/`__eq__`, passed
        // as function pointers to the `_fn` entry point (D-0047); empty otherwise.
        let kfns = self.key_fn_args(kk)?;
        for ((kv, kt), (vv, vt)) in lowered {
            if elem_kind(kt)? != kk || elem_kind(vt)? != vk {
                return Err(err(
                    "dict keys/values must all have the same representation",
                ));
            }
            let kv = self.coerce(kv, kt, kk.ty());
            let vv = self.coerce(vv, vt, vk.ty());
            let kv = self.box_if_value(kv, kk.ty());
            let vv = self.box_if_value(vv, vk.ty());
            let mut cargs = vec![dict, kv, vv];
            cargs.extend_from_slice(&kfns);
            self.emit(Inst::Call {
                dest: None,
                func: set.clone(),
                args: cargs,
            });
        }
        Ok((dict, Ty::Dict(kk, vk)))
    }

    fn lower_set_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        // Elements are evaluated left-to-right; the first fixes the element kind
        // (an empty literal is a dict, so a set literal always has >= 1 element).
        let mut lowered = Vec::with_capacity(elems.len());
        for e in elems {
            lowered.push(self.expr(e)?);
        }
        let elem = match lowered.first() {
            Some(&(_, t)) => elem_kind(t)?,
            None => Elem::I64,
        };
        let set = self.new_value(Ty::Set(elem));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let add = format!("zap_set_add_{}", elem.key_suffix());
        let kfns = self.key_fn_args(elem)?;
        for (v, vt) in lowered {
            if elem_kind(vt)? != elem {
                return Err(err("set elements must all have the same representation"));
            }
            let v = self.coerce(v, vt, elem.ty());
            let v = self.box_if_value(v, elem.ty());
            let mut cargs = vec![set, v];
            cargs.extend_from_slice(&kfns);
            self.emit(Inst::Call {
                dest: None,
                func: add.clone(),
                args: cargs,
            });
        }
        Ok((set, Ty::Set(elem)))
    }

    fn lower_set_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "add" => {
                if args.len() != 1 {
                    return Err(err("add() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if elem_kind(vt)? != elem {
                    return Err(err("add() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_if_value(v, elem.ty());
                let kfns = self.key_fn_args(elem)?;
                let mut cargs = vec![recv, v];
                cargs.extend_from_slice(&kfns);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_set_add_{}", elem.key_suffix()),
                    args: cargs,
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!("set has no method '{other}' (supported: add)"))),
        }
    }

    /// Lower `heap[T]()` / `heap[T](cmp="max")` (D-0057). The element kind is the
    /// subscript; the only argument is the keyword `cmp` selecting direction
    /// (`"min"` is the default min-heap, `"max"` a max-heap). Lowers to a
    /// `zap_heap_new(kind, dir)` runtime call where `kind` picks the element word
    /// representation (0 = i64, 1 = str) and `dir` the ordering (0 = min, 1 = max).
    fn lower_heap_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let Expr::Name(ename, _) = elem_expr else {
            return Err(err(
                "heap[...] expects an element type name, e.g. heap[int64]",
            ));
        };
        let elem = heap_elem_from_name(ename)?;
        let mut dir = 0i64; // 0 = min-heap (default), 1 = max-heap
        for a in args {
            match a.name.as_deref() {
                Some("cmp") => {
                    let Expr::Str { value, .. } = &a.value else {
                        return Err(err(
                            "heap cmp must be a string literal (\"min\" or \"max\")",
                        ));
                    };
                    dir = match value.as_str() {
                        "min" => 0,
                        "max" => 1,
                        other => {
                            return Err(err(format!(
                                "heap cmp must be \"min\" or \"max\", found \"{other}\""
                            )))
                        }
                    };
                }
                Some(other) => {
                    return Err(err(format!(
                        "heap[T](...) has no keyword argument '{other}' (only 'cmp')"
                    )))
                }
                None => {
                    return Err(err(
                        "heap[T](...) takes only the keyword argument 'cmp' (e.g. cmp=\"max\")",
                    ))
                }
            }
        }
        let kind = self.iconst(Ty::I64, if elem == Elem::I64 { 0 } else { 1 });
        let d = self.iconst(Ty::I64, dir);
        let h = self.new_value(Ty::Heap(elem));
        self.emit(Inst::Call {
            dest: Some(h),
            func: "zap_heap_new".to_string(),
            args: vec![kind, d],
        });
        Ok((h, Ty::Heap(elem)))
    }

    /// Lower a heap method call (D-0057): `push(v)`, `pop()`, `peek()`. Each maps
    /// to the element-kind-suffixed runtime entry point. `pop`/`peek` return the
    /// element and abort at runtime on an empty heap (no exception path yet, like
    /// an out-of-range list index).
    fn lower_heap_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "push" => {
                if args.len() != 1 {
                    return Err(err("push() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if elem_kind(vt)? != elem {
                    return Err(err("push() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_heap_push_{}", elem.val_suffix()),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            "pop" => {
                if !args.is_empty() {
                    return Err(err("pop() takes no arguments"));
                }
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_heap_pop_{}", elem.val_suffix()),
                    args: vec![recv],
                });
                Ok((d, elem.ty()))
            }
            "peek" => {
                if !args.is_empty() {
                    return Err(err("peek() takes no arguments"));
                }
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_heap_peek_{}", elem.val_suffix()),
                    args: vec![recv],
                });
                Ok((d, elem.ty()))
            }
            other => Err(err(format!(
                "heap has no method '{other}' (supported: push, pop, peek)"
            ))),
        }
    }

    /// Lower a tuple literal (D-0049): a by-value anonymous-struct aggregate. The
    /// element-type sequence interns to a struct id, then `StructNew` + one
    /// `FieldSet` per element build the value — exactly the struct path, so
    /// codegen needs no tuple-specific handling.
    fn lower_tuple_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        let mut vals = Vec::with_capacity(elems.len());
        let mut tys = Vec::with_capacity(elems.len());
        for e in elems {
            let (v, t) = self.expr(e)?;
            vals.push(v);
            tys.push(t);
        }
        let sid = self.ctx.tuples.borrow_mut().intern(tys);
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (idx, v) in vals.into_iter().enumerate() {
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Element-wise tuple comparison (D-0049). `op` is `==` or `!=`; the result
    /// is the conjunction of the per-element `==` results (then negated for
    /// `!=`). Elements are compared by recursing through [`binop`], so nested
    /// tuples and scalar elements work; a reference-typed element (e.g. `str`)
    /// currently surfaces `binop`'s "unsupported reference type" error.
    fn tuple_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        ls: u32,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let Ty::Struct(rs) = rt else {
            return Err(err("a tuple can only be compared with another tuple"));
        };
        if ls != rs {
            return Err(err("cannot compare tuples of different types"));
        }
        let field_tys = self
            .ctx
            .tuples
            .borrow()
            .fields(ls)
            .ok_or_else(|| err("not a tuple"))?;
        // Start from `true` and AND each element's equality.
        let mut acc = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: acc,
            val: true,
        });
        for (i, fty) in field_tys.iter().enumerate() {
            let lf = self.field_get(l, ls, i as u32, *fty);
            let rf = self.field_get(r, rs, i as u32, *fty);
            let (eq, _) = self.binop(AstBin::Eq, lf, *fty, rf, *fty)?;
            let next = self.new_value(Ty::I1);
            self.emit(Inst::Bin {
                dest: next,
                op: BinOp::And,
                lhs: acc,
                rhs: eq,
            });
            acc = next;
        }
        if matches!(op, AstBin::Ne) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: acc,
            });
            return Ok((d, Ty::I1));
        }
        Ok((acc, Ty::I1))
    }

    fn lower_index_get(&mut self, base: &Expr, index: &Expr) -> Result<(Value, Ty), LowerError> {
        let (bv, bty) = self.expr(base)?;
        // Tuple element access `t[k]` (D-0049): `k` must be a compile-time
        // constant (a tuple is heterogeneous), and the result is that field.
        if let Ty::Struct(sid) = bty {
            if let Some(field_tys) = self.ctx.tuples.borrow().fields(sid) {
                let k = const_int_value(index)
                    .ok_or_else(|| err("a tuple index must be a compile-time-constant integer"))?;
                if k < 0 || k as usize >= field_tys.len() {
                    return Err(err(format!(
                        "tuple index {k} out of range for a {}-element tuple",
                        field_tys.len()
                    )));
                }
                let fty = field_tys[k as usize];
                let d = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: d,
                    base: bv,
                    sid,
                    idx: k as u32,
                });
                return Ok((d, fty));
            }
        }
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_list_get_{}", elem.val_suffix()),
                    args: vec![bv, iv],
                });
                Ok((d, elem.ty()))
            }
            Ty::Dict(kk, vk) => {
                let (kv, kt) = self.expr(index)?;
                let kv = self.coerce(kv, kt, kk.ty());
                let kfns = self.key_fn_args(kk)?;
                let d = self.new_value(vk.ty());
                let mut cargs = vec![bv, kv];
                cargs.extend_from_slice(&kfns);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: cargs,
                });
                Ok((d, vk.ty()))
            }
            Ty::Array(d) => {
                // 1-D element access `a[i]`. Multi-axis indexing `m[i, j]` and
                // slices are deferred (N-D track).
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let dest = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_array_get_{}", d.suffix()),
                    args: vec![bv, iv],
                });
                Ok((dest, d.ty()))
            }
            Ty::Struct(sid) => {
                // Indexing overloading (D-0047): `a[i]` -> `a.__getitem__(i)`.
                let (iv, it) = self.expr(index)?;
                self.call_dunder(sid, "__getitem__", vec![(bv, bty), (iv, it)])
            }
            _ => Err(err(
                "indexing is only supported on list, dict, and array values",
            )),
        }
    }

    fn lower_index_set(
        &mut self,
        base: &Expr,
        index: &Expr,
        value: &Expr,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let (v, vt) = self.expr(value)?;
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_if_value(v, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_set_{}", elem.val_suffix()),
                    args: vec![bv, iv, v],
                });
                Ok(())
            }
            Ty::Dict(kk, vk) => {
                let (kv, kt) = self.expr(index)?;
                let kv = self.coerce(kv, kt, kk.ty());
                let kv = self.box_if_value(kv, kk.ty());
                let (v, vt) = self.expr(value)?;
                let v = self.coerce(v, vt, vk.ty());
                let v = self.box_if_value(v, vk.ty());
                let kfns = self.key_fn_args(kk)?;
                let mut cargs = vec![bv, kv, v];
                cargs.extend_from_slice(&kfns);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: cargs,
                });
                Ok(())
            }
            Ty::Array(d) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let (v, vt) = self.expr(value)?;
                let v = self.coerce(v, vt, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![bv, iv, v],
                });
                Ok(())
            }
            Ty::Struct(sid) => {
                // Indexed-assignment overloading (D-0047):
                // `a[i] = v` -> `a.__setitem__(i, v)`.
                let (iv, it) = self.expr(index)?;
                let (v, vt) = self.expr(value)?;
                self.call_dunder(sid, "__setitem__", vec![(bv, bty), (iv, it), (v, vt)])?;
                Ok(())
            }
            _ => Err(err(
                "indexed assignment is only supported on list, dict, and array values",
            )),
        }
    }

    fn lower_list_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "append" => {
                if args.len() != 1 {
                    return Err(err("append() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if elem_kind(vt)? != elem {
                    return Err(err("append() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_if_value(v, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_push_{}", elem.val_suffix()),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!(
                "list has no method '{other}' (supported: append)"
            ))),
        }
    }

    // --- regex (ticket #23, D-0010) ------------------------------------------
    //
    // `m/.../`, `s/.../.../`, `=~`/`!~` and the match object all lower to calls
    // into the PCRE2-backed runtime (`zap_regex_*`); no new ZIR instruction is
    // needed (the string/collection pattern). Literal patterns are emitted as
    // string constants and compiled by the runtime on each call — a compiled-
    // pattern cache is future work (design/0001 step 4). A match object is a
    // GC'd heap value that owns its captured groups, so capture state is
    // coroutine-safe by construction (no magic globals, D-0010).

    /// `subject =~ m/pattern/flags` → a match object (`Ty::Match`, null = no
    /// match), or, for `!~`, an `i1` that is true when there is no match.
    fn lower_regex_match(
        &mut self,
        subject: &Expr,
        pattern: &str,
        flags: &str,
        negated: bool,
    ) -> Result<(Value, Ty), LowerError> {
        let (sv, _st) = self.expr(subject)?;
        let pv = self.strconst(pattern);
        let fv = self.iconst(Ty::I64, regex_flag_bits(flags));
        let m = self.new_value(Ty::Match);
        self.emit(Inst::Call {
            dest: Some(m),
            func: "zap_regex_match".to_string(),
            args: vec![sv, pv, fv],
        });
        if !negated {
            return Ok((m, Ty::Match));
        }
        // `!~`: true exactly when the match object is null.
        let matched = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(matched),
            func: "zap_ptr_nonnull".to_string(),
            args: vec![m],
        });
        let zero = self.iconst(Ty::I64, 0);
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred: CmpPred::Eq,
            lhs: matched,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    /// `subject =~ s/pattern/replacement/flags` → a new string (the subject is
    /// never mutated). The `g` flag replaces all occurrences, else the first.
    fn lower_regex_subst(
        &mut self,
        subject: &Expr,
        pattern: &str,
        replacement: &str,
        flags: &str,
    ) -> Result<(Value, Ty), LowerError> {
        let (sv, _st) = self.expr(subject)?;
        let pv = self.strconst(pattern);
        let rv = self.strconst(replacement);
        let fv = self.iconst(Ty::I64, regex_flag_bits(flags));
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: "zap_regex_subst".to_string(),
            args: vec![sv, pv, rv, fv],
        });
        Ok((d, Ty::Ptr))
    }

    /// Methods on a match object: `m.group(n)` (positional, 0 = whole match) and
    /// `m.group("name")` (named capture). Both return a `str` (empty if the
    /// group did not participate).
    fn lower_match_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "group" => {
                if args.len() != 1 {
                    return Err(err("group() takes exactly one argument"));
                }
                let (arg, aty) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                if aty == Ty::Ptr {
                    // Named group: `m.group("year")`.
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_regex_group_name".to_string(),
                        args: vec![recv, arg],
                    });
                } else {
                    // Positional group: `m.group(0)`.
                    let idx = self.coerce(arg, aty, Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_regex_group".to_string(),
                        args: vec![recv, idx],
                    });
                }
                Ok((d, Ty::Ptr))
            }
            other => Err(err(format!(
                "a match object has no method '{other}' (supported: group)"
            ))),
        }
    }

    // --- Python interop (ticket #27, D-0012, design/0003-python-interop.md) ---
    //
    // `import python "mod" as m` makes `m` a Python module handle; attribute
    // access and calls dispatch through the embedded CPython C-API, and values
    // marshal across the boundary. Nothing here needs a new ZIR instruction (the
    // strings/collections/regex pattern): every operation lowers to a
    // `zap_py_*` runtime call. A Python value flows as `Ty::Py` — a GC-managed
    // handle that owns one CPython reference (decref'd by a finalizer). Calls
    // that can surface a Python exception use `throwing_call`, so a raised
    // Python error becomes a Zap exception catchable with `try`/`except`
    // (D-0007): inside a `try` it is an `invoke` to the handler, else it unwinds
    // to the caller / top level like any other `zap_panic`.

    /// `import python "module"` → a module handle. Importing is idempotent
    /// (CPython caches modules), so each reference re-imports cheaply.
    fn lower_py_import(&mut self, module: &str) -> Value {
        let name = self.strconst(module);
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_import", vec![name]);
        d
    }

    /// `obj.attr` on a Python handle → the attribute value (another handle).
    fn lower_py_getattr(&mut self, base: Value, name: &str) -> Value {
        let nv = self.strconst(name);
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_getattr", vec![base, nv]);
        d
    }

    /// Call a Python callable with the given Zap argument expressions. Arguments
    /// are marshaled to Python values and collected into a `list[ptr]` the
    /// runtime turns into a call tuple (any arity; keyword arguments are a
    /// documented follow-up).
    fn lower_py_call(
        &mut self,
        callable: Value,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let argl = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(argl),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        for a in args {
            if a.name.is_some() {
                return Err(err(
                    "keyword arguments to Python calls are not supported yet",
                ));
            }
            let p = self.marshal_to_py(&a.value)?;
            self.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![argl, p],
            });
        }
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_call", vec![callable, argl]);
        Ok((d, Ty::Py))
    }

    /// Marshal one Zap value into a Python handle (ticket #27 marshaling table).
    /// Scalars and `str` convert by value; a value already on the Python side is
    /// passed through. Collections and other types are a documented follow-up.
    fn marshal_to_py(&mut self, e: &Expr) -> Result<Value, LowerError> {
        let (v, vt) = self.expr(e)?;
        let (helper, arg) = if vt == Ty::Py {
            return Ok(v);
        } else if vt == Ty::I1 {
            ("zap_py_from_bool", self.coerce(v, vt, Ty::I64))
        } else if vt.int_width().is_some() {
            ("zap_py_from_i64", self.coerce(v, vt, Ty::I64))
        } else if vt.is_float() {
            ("zap_py_from_f64", self.as_f64(v, vt))
        } else if vt == Ty::Ptr {
            ("zap_py_from_str", v)
        } else {
            return Err(err(
                "cannot marshal this value to Python yet (supported: int, float, bool, str, and Python handles)",
            ));
        };
        let d = self.new_value(Ty::Py);
        self.emit(Inst::Call {
            dest: Some(d),
            func: helper.to_string(),
            args: vec![arg],
        });
        Ok(d)
    }

    /// Call a struct's dunder method (operator overloading, D-0047). `args` are
    /// `(value, type)` pairs starting with the receiver (`self`); each is coerced
    /// to the registered parameter type. A missing dunder is a clean error (sema
    /// normally catches it first, but the ir crate is exercised without sema).
    fn call_dunder(
        &mut self,
        sid: u32,
        dunder: &str,
        args: Vec<(Value, Ty)>,
    ) -> Result<(Value, Ty), LowerError> {
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
        let (ptys, ret) = self.sigs.get(&mangled).cloned().ok_or_else(|| {
            err(format!(
                "type '{}' does not define '{}' (required by this operator; D-0047)",
                self.ctx.names[sid as usize], dunder
            ))
        })?;
        let mut argvals = Vec::with_capacity(args.len());
        for (i, (v, vt)) in args.into_iter().enumerate() {
            let target = *ptys.get(i).unwrap_or(&vt);
            argvals.push(self.coerce(v, vt, target));
        }
        let dest = if ret == Ty::Void {
            None
        } else {
            Some(self.new_value(ret))
        };
        self.throwing_call(dest, &mangled, argvals);
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }))
    }

    /// The extra runtime arguments for a collection *key* of the given element
    /// kind: for a struct key (D-0047), the `__hash__` and `__eq__` function
    /// pointers the `_fn` entry points take; for any other key, none. A struct
    /// key without both dunders is a clean error (needs `@derive(Hash, Eq)` or
    /// hand-written methods).
    fn key_fn_args(&mut self, elem: Elem) -> Result<Vec<Value>, LowerError> {
        let Elem::Struct(sid) = elem else {
            return Ok(vec![]);
        };
        let name = self.ctx.names[sid as usize].clone();
        let hashn = format!("{name}__{}", "__hash__");
        let eqn = format!("{name}__{}", "__eq__");
        if !self.sigs.contains_key(&hashn) {
            return Err(err(format!(
                "type '{name}' needs '__hash__' to be a dict/set key \
                 (use @derive(Hash) or define it; D-0047)"
            )));
        }
        if !self.sigs.contains_key(&eqn) {
            return Err(err(format!(
                "type '{name}' needs '__eq__' to be a dict/set key \
                 (use @derive(Eq) or define it; D-0047)"
            )));
        }
        let hv = self.new_value(Ty::Ptr);
        self.emit(Inst::FuncAddr {
            dest: hv,
            func: hashn,
        });
        let ev = self.new_value(Ty::Ptr);
        self.emit(Inst::FuncAddr {
            dest: ev,
            func: eqn,
        });
        Ok(vec![hv, ev])
    }

    /// Render a struct value to a string via its `__str__` (preferred) or
    /// `__repr__` dunder (D-0047), for `print` / string contexts.
    fn struct_to_str(&mut self, sid: u32, v: Value) -> Result<Value, LowerError> {
        for dunder in ["__str__", "__repr__"] {
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
            if self.sigs.contains_key(&mangled) {
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), &mangled, vec![v]);
                return Ok(d);
            }
        }
        Err(err(format!(
            "cannot print a '{}' value: it defines neither '__str__' nor \
             '__repr__' (define one or use @derive(Repr); D-0047)",
            self.ctx.names[sid as usize]
        )))
    }

    /// Desugar a binary operator whose left operand is a user struct to the
    /// matching dunder (D-0047). Equality/relational operators derive from
    /// `__eq__` / `__cmp__`; `and`/`or` use truthiness; the rest call their
    /// arithmetic/bitwise dunder directly and take its return type.
    fn struct_binop(
        &mut self,
        op: AstBin,
        sid: u32,
        l: Value,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let lt = Ty::Struct(sid);
        match op {
            Eq | Ne => {
                let (bit, _) = self.call_dunder(sid, "__eq__", vec![(l, lt), (r, rt)])?;
                if matches!(op, Ne) {
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::Un {
                        dest: d,
                        op: UnOp::Not,
                        val: bit,
                    });
                    Ok((d, Ty::I1))
                } else {
                    Ok((bit, Ty::I1))
                }
            }
            Lt | Le | Gt | Ge => {
                // `a < b` -> `a.__cmp__(b) < 0`, and likewise for the others.
                let (cmp, _) = self.call_dunder(sid, "__cmp__", vec![(l, lt), (r, rt)])?;
                let pred = match op {
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: cmp,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            And | Or => {
                let lb = self.truthy(l, lt);
                let rb = self.truthy(r, rt);
                let irop = if matches!(op, And) {
                    BinOp::And
                } else {
                    BinOp::Or
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: lb,
                    rhs: rb,
                });
                Ok((d, Ty::I1))
            }
            Send => Err(err("'<-' (channel send) is not an overloadable operator")),
            In => unreachable!("'in' dispatches on the right operand"),
            _ => {
                let dunder = match op {
                    Add => "__add__",
                    Sub => "__sub__",
                    Mul => "__mul__",
                    Div => "__truediv__",
                    FloorDiv => "__floordiv__",
                    Mod => "__mod__",
                    Pow => "__pow__",
                    BitAnd => "__and__",
                    BitOr => "__or__",
                    BitXor => "__xor__",
                    Shl => "__lshift__",
                    Shr => "__rshift__",
                    _ => unreachable!(),
                };
                self.call_dunder(sid, dunder, vec![(l, lt), (r, rt)])
            }
        }
    }

    fn binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;

        // Channel send `ch <- v` (ticket #25, D-0031): handled before the
        // generic reference-type guard below, since the channel operand is a
        // reference type. Blocks until the value can be delivered; the element
        // kind picks the typed runtime entry point. Used as a statement, so the
        // result is a unit-like `i32 0`.
        if matches!(op, Send) {
            let Ty::Chan(elem) = lt else {
                return Err(err("the left operand of '<-' (send) must be a channel"));
            };
            let v = self.coerce(r, rt, elem.ty());
            self.throwing_call(
                None,
                &format!("zap_chan_send_{}", elem.val_suffix()),
                vec![l, v],
            );
            return Ok((self.iconst(Ty::I32, 0), Ty::I32));
        }

        // Element-wise numeric-array ops (ticket #24): same-shape `a + b` and
        // scalar broadcast `a + 2.0`, lowered to a SIMD kernel in codegen.
        if matches!(lt, Ty::Array(_)) || matches!(rt, Ty::Array(_)) {
            return self.array_binop(op, l, lt, r, rt);
        }

        // Element-wise tuple comparison (D-0049): `t1 == t2` / `t1 != t2` compares
        // positionally, recursing into each element (so nested tuples compare
        // element-wise too). A tuple is an anonymous struct but not a *user*
        // struct, so this must precede operator-overloading dispatch below (a
        // tuple has no dunder methods); other operators on tuples are unsupported
        // and fall through to the struct path, which reports a clear error.
        if matches!(op, Eq | Ne) {
            if let Ty::Struct(ls) = lt {
                if self.ctx.tuples.borrow().is_tuple(ls) {
                    return self.tuple_cmp(op, l, ls, r, rt);
                }
            }
        }

        // Operator overloading on user structs (D-0047): `a op b` desugars to the
        // matching dunder method, resolved like any struct method call. `in`
        // dispatches on the right operand (`x in a` -> `a.__contains__(x)`); every
        // other operator dispatches on the left.
        if let (Ty::Struct(sid), false) = (lt, matches!(op, In)) {
            return self.struct_binop(op, sid, l, r, rt);
        }
        if matches!(op, In) {
            if let Ty::Struct(sid) = rt {
                let contains = format!("{}__{}", self.ctx.names[sid as usize], "__contains__");
                if self.sigs.contains_key(&contains) {
                    let (ret, rty) =
                        self.call_dunder(sid, "__contains__", vec![(r, rt), (l, lt)])?;
                    return Ok((ret, rty));
                }
                // No `__contains__`: membership falls back to iterating the
                // container and comparing each element for equality (D-0048).
                let result = self.alloca(Ty::I1);
                let f = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: f,
                    val: false,
                });
                self.store(result, f);
                self.drive_iteration(r, sid, |s, ev, ety| {
                    let (eq, _) = s.binop(AstBin::Eq, ev, ety, l, lt)?;
                    let then_bb = s.new_block();
                    let cont_bb = s.new_block();
                    s.set_term(Term::BrIf {
                        cond: eq,
                        then_bb,
                        else_bb: cont_bb,
                    });
                    s.position(then_bb);
                    let t = s.new_value(Ty::I1);
                    s.emit(Inst::BConst { dest: t, val: true });
                    s.store(result, t);
                    let (_, brk) = *s.loops.last().expect("in-iteration loop");
                    s.set_term(Term::Br(brk));
                    s.position(cont_bb);
                    Ok(())
                })?;
                let d = self.load(result, Ty::I1);
                return Ok((d, Ty::I1));
            }
        }

        // Membership: `x in <set>` -> `zap_set_contains_*`, normalized to a bool.
        if matches!(op, In) {
            let Ty::Set(elem) = rt else {
                return Err(err("'in' is only supported with a set on the right"));
            };
            if elem_kind(lt)? != elem {
                return Err(err("'in' element has the wrong type for this set"));
            }
            let l = self.coerce(l, lt, elem.ty());
            let kfns = self.key_fn_args(elem)?;
            let word = self.new_value(Ty::I64);
            let mut cargs = vec![r, l];
            cargs.extend_from_slice(&kfns);
            self.emit(Inst::Call {
                dest: Some(word),
                func: format!("zap_set_contains_{}", elem.key_suffix()),
                args: cargs,
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: word,
                rhs: zero,
            });
            return Ok((d, Ty::I1));
        }

        // Reference-typed operands: the only supported one is `str + str`.
        if matches!(lt, Ty::Ptr) || matches!(rt, Ty::Ptr) {
            if matches!(op, Add) && lt == Ty::Ptr && rt == Ty::Ptr {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_concat".to_string(),
                    args: vec![l, r],
                });
                return Ok((d, Ty::Ptr));
            }
            return Err(err(
                "unsupported operation on a reference type (only str + str)",
            ));
        }

        let is_float = lt.is_float() || rt.is_float();

        if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
            let pred = match op {
                Eq => CmpPred::Eq,
                Ne => CmpPred::Ne,
                Lt => CmpPred::Lt,
                Le => CmpPred::Le,
                Gt => CmpPred::Gt,
                Ge => CmpPred::Ge,
                _ => unreachable!(),
            };
            let d = self.new_value(Ty::I1);
            if is_float {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                self.emit(Inst::FCmp {
                    dest: d,
                    pred,
                    lhs: lf,
                    rhs: rf,
                });
            } else {
                // Integer comparisons are done at i64 so mixed-width operands
                // (e.g. `int32 < int64`, or a literal against an int32) share a
                // type (D-0019 all-signed tower).
                let li = self.coerce(l, lt, Ty::I64);
                let ri = self.coerce(r, rt, Ty::I64);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: li,
                    rhs: ri,
                });
            }
            return Ok((d, Ty::I1));
        }

        match op {
            Add | Sub | Mul | Mod => {
                let irop = match op {
                    Add => BinOp::Add,
                    Sub => BinOp::Sub,
                    Mul => BinOp::Mul,
                    Mod => BinOp::Rem,
                    _ => unreachable!(),
                };
                if is_float {
                    // Float arithmetic computes at f64 (operands widened), per
                    // the numeric join in sema (any float operand -> float64).
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: lf,
                        rhs: rf,
                    });
                    Ok((d, Ty::F64))
                } else {
                    // Integer arithmetic computes at i64 (operands widened), so
                    // `int32`/untyped-literal operands share a width (D-0019).
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: li,
                        rhs: ri,
                    });
                    Ok((d, Ty::I64))
                }
            }
            FloorDiv => {
                // Floor division (D-0024) rounds toward negative infinity,
                // unlike LLVM `sdiv` (toward zero) / `fdiv` (no rounding). The
                // toward-negative-infinity adjustment lives in a runtime helper
                // so codegen stays mechanical and the semantics are in one place.
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_ffloordiv".to_string(),
                        args: vec![lf, rf],
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_ifloordiv".to_string(),
                        args: vec![li, ri],
                    });
                    Ok((d, Ty::I64))
                }
            }
            Pow => {
                // `**` over the numeric tower (D-0019). A floating-point base or
                // exponent goes through libm `pow` (f64); the all-integer case
                // uses a binary-exponentiation runtime helper and keeps an
                // integer result per the static type (a negative exponent yields
                // the floor of the true power — 0 for |base| > 1).
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_powf".to_string(),
                        args: vec![lf, rf],
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_ipow".to_string(),
                        args: vec![li, ri],
                    });
                    Ok((d, Ty::I64))
                }
            }
            Div => {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                let d = self.new_value(Ty::F64);
                self.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::Div,
                    lhs: lf,
                    rhs: rf,
                });
                Ok((d, Ty::F64))
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                let irop = match op {
                    BitAnd => BinOp::And,
                    BitOr => BinOp::Or,
                    BitXor => BinOp::Xor,
                    Shl => BinOp::Shl,
                    Shr => BinOp::Shr,
                    _ => unreachable!(),
                };
                let li = self.coerce(l, lt, Ty::I64);
                let ri = self.coerce(r, rt, Ty::I64);
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: li,
                    rhs: ri,
                });
                Ok((d, Ty::I64))
            }
            And | Or => {
                let lb = self.truthy(l, lt);
                let rb = self.truthy(r, rt);
                let irop = if matches!(op, And) {
                    BinOp::And
                } else {
                    BinOp::Or
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: lb,
                    rhs: rb,
                });
                Ok((d, Ty::I1))
            }
            In => Err(err("'in' is not supported by codegen yet")),
            // `Send` is intercepted at the top of `binop` (before the generic
            // reference-type guard), so it never reaches here.
            Send => unreachable!("channel send handled above"),
            Eq | Ne | Lt | Le | Gt | Ge => unreachable!(),
        }
    }

    /// Element-wise `+`/`-`/`*` on arrays, including scalar broadcast and
    /// mixed-dtype promotion (ticket #49). Emits the [`Inst::ArrayBinOp`] node
    /// codegen vectorizes. Two arrays of different dtypes promote to the higher
    /// dtype ([`Dtype::promote`], so `i64 + f64 -> f64`); a scalar is weak
    /// ([`Dtype::promote_scalar`], so `uint8 + 1` stays `uint8`). `scalar op
    /// array` is only accepted for the commutative `+`/`*` (reordered to `array
    /// op scalar`), so the SIMD kernel always computes `array[i] op scalar`.
    fn array_binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let irop = match op {
            Add => BinOp::Add,
            Sub => BinOp::Sub,
            Mul => BinOp::Mul,
            _ => {
                return Err(err(
                    "only +, -, and * are supported on arrays yet (/, reductions beyond sum, etc. are deferred)",
                ))
            }
        };
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(ld), Ty::Array(rd)) => {
                let out = Dtype::promote(ld, rd);
                let dest = self.new_value(Ty::Array(out));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: r,
                    rhs_dtype: rd,
                    scalar: false,
                });
                Ok((dest, Ty::Array(out)))
            }
            (Ty::Array(ld), _) if is_scalar(rt) => {
                // Weak scalar (NEP 50): an int scalar keeps the array's dtype,
                // a float scalar forces `f64`. The scalar crosses to codegen as
                // its natural `i64`/`f64` word (`rhs_dtype`); codegen converts
                // it to `out` (possibly a narrowing `i64 -> u8`).
                let scalar_is_float = rt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(r, rt, sdt.ty());
                let out = Dtype::promote_scalar(ld, scalar_is_float);
                let dest = self.new_value(Ty::Array(out));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                });
                Ok((dest, Ty::Array(out)))
            }
            (_, Ty::Array(rd)) if is_scalar(lt) => {
                if !matches!(op, Add | Mul) {
                    return Err(err(
                        "scalar-on-the-left broadcast is only supported for + and * (write `a - scalar`, not `scalar - a`)",
                    ));
                }
                let scalar_is_float = lt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(l, lt, sdt.ty());
                let out = Dtype::promote_scalar(rd, scalar_is_float);
                let dest = self.new_value(Ty::Array(out));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: r,
                    lhs_dtype: rd,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                });
                Ok((dest, Ty::Array(out)))
            }
            _ => Err(err(
                "unsupported array operand (element-wise ops take two arrays or an array and a numeric scalar)",
            )),
        }
    }

    // --- coercions / helpers ---

    fn coerce(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        // Concrete struct -> interface value: build the `{data, itable}` fat
        // pointer (D-0016, design/0006 §2.4). A reference struct is used directly
        // as `data` (shared heap identity). A `value struct` is **boxed** first
        // (I#75 Part 1, D-0051): copied to a fresh heap instance so `data` holds a
        // stable, GC-traced pointer to a copy that is independent of the source
        // (whose storage may be a stack slot under the register/sret ABI, or an
        // lvalue that must not alias). interface -> interface and `None` ->
        // interface are documented follow-ups.
        if let Ty::Struct(to_sid) = to {
            if let Some(iid) = self.ctx.iid_of_fat_sid(to_sid) {
                if let Ty::Struct(from_sid) = from {
                    if self.ctx.iid_of_fat_sid(from_sid).is_none() {
                        let data = if self.ctx.is_value_struct(from_sid) {
                            self.copy_value_struct(v, from_sid)
                        } else {
                            v
                        };
                        let dest = self.new_value(to);
                        self.emit(Inst::IfaceMake {
                            dest,
                            data,
                            iid,
                            concrete_sid: from_sid,
                        });
                        return dest;
                    }
                }
                // Not a concrete-struct source we can box — leave unchanged; sema
                // has already rejected a non-conforming or unsupported source.
                return v;
            }
        }
        if let (Some(fw), Some(tw)) = (from.int_width(), to.int_width()) {
            let d = self.new_value(to);
            let kind = if tw > fw {
                CastKind::SExt
            } else {
                CastKind::Trunc
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
                val: v,
            });
            return d;
        }
        if from.int_width().is_some() && to.is_float() {
            let d = self.new_value(to);
            self.emit(Inst::Cast {
                dest: d,
                kind: CastKind::SIToFP,
                val: v,
            });
            return d;
        }
        if from.is_float() && to.is_float() {
            let d = self.new_value(to);
            let kind = if to == Ty::F64 {
                CastKind::FPExt
            } else {
                CastKind::FPTrunc
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
                val: v,
            });
            return d;
        }
        v
    }

    /// Widen a numeric value to `f64` (int -> sitofp, f32 -> fpext, f64 -> self).
    fn as_f64(&mut self, v: Value, t: Ty) -> Value {
        self.coerce(v, t, Ty::F64)
    }

    /// An explicit numeric conversion to `to` (the `int32(x)` / `float64(x)`
    /// form, D-0020). Unlike [`coerce`], which only widens, this also narrows
    /// and converts float -> int (a user wrote the cast, so data loss is their
    /// intent). Non-numeric operands pass through unchanged.
    fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        let kind = match (from, to) {
            (f, t) if f.int_width().is_some() && t.int_width().is_some() => {
                if t.int_width().unwrap() > f.int_width().unwrap() {
                    CastKind::SExt
                } else {
                    CastKind::Trunc
                }
            }
            (f, t) if f.int_width().is_some() && t.is_float() => CastKind::SIToFP,
            (f, t) if f.is_float() && t.int_width().is_some() => CastKind::FPToSI,
            (f, t) if f.is_float() && t.is_float() => {
                if t == Ty::F64 {
                    CastKind::FPExt
                } else {
                    CastKind::FPTrunc
                }
            }
            _ => return v,
        };
        let d = self.new_value(to);
        self.emit(Inst::Cast {
            dest: d,
            kind,
            val: v,
        });
        d
    }

    fn truthy(&mut self, v: Value, t: Ty) -> Value {
        if t == Ty::I1 {
            return v;
        }
        // Truthiness overloading on user structs (D-0047): `if a:` / `not a` /
        // `a and b` call `a.__bool__()` when defined. A struct with no `__bool__`
        // is a non-null object, so it is truthy.
        if let Ty::Struct(sid) = t {
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], "__bool__");
            if self.sigs.contains_key(&mangled) {
                let d = self.new_value(Ty::I1);
                self.throwing_call(Some(d), &mangled, vec![v]);
                return d;
            }
            return self.iconst(Ty::I1, 1);
        }
        if matches!(t, Ty::Match | Ty::Ptr | Ty::Py) {
            // Reference types are truthy when non-null. Codegen's `ICmp` only
            // compares integers, so go through the runtime (`zap_ptr_nonnull`
            // returns 0/1) and compare that i64 against zero. For a match
            // object, null means "no match" (D-0010). A Python handle is a
            // non-null wrapper, so it is effectively always truthy here; real
            // Python truthiness (`PyObject_IsTrue`) is a documented follow-up.
            let nn = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(nn),
                func: "zap_ptr_nonnull".to_string(),
                args: vec![v],
            });
            let z = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: nn,
                rhs: z,
            });
            return d;
        }
        if t.is_float() {
            let z = self.fconst(t, 0.0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::FCmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: v,
                rhs: z,
            });
            d
        } else {
            let z = self.iconst(t, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: v,
                rhs: z,
            });
            d
        }
    }
}

fn is_name(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Name(n, _) if n == name)
}

/// Comparison operators for which power-assert shows both operand values
/// (D-0054). Membership (`in`) and the boolean connectives are deliberately
/// excluded — their operand values read poorly side-by-side.
fn is_power_comparison(op: AstBin) -> bool {
    matches!(
        op,
        AstBin::Eq | AstBin::Ne | AstBin::Lt | AstBin::Le | AstBin::Gt | AstBin::Ge
    )
}

/// `StopIteration` or `StopIteration()` — the iterator exhaustion signal
/// (D-0048), accepted in either the bare-name or called form.
fn is_stop_iteration(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "StopIteration",
        Expr::Call { callee, .. } => is_name(callee, "StopIteration"),
        _ => false,
    }
}

/// The ZIR target type of an explicit numeric conversion call, by builtin name
/// (`int32(x)`, `float64(x)`, ...). `None` for any other callee.
fn numeric_conv_ty(name: &str) -> Option<Ty> {
    match name {
        "int32" | "i32" => Some(Ty::I32),
        "int64" | "i64" => Some(Ty::I64),
        "float32" | "f32" => Some(Ty::F32),
        "float64" | "f64" => Some(Ty::F64),
        _ => None,
    }
}

/// The regex flag bitmask passed to the runtime. Must stay in sync with the
/// `ZAP_RE_*` constants in `zaprt.c`. Unknown flag letters are ignored.
fn regex_flag_bits(flags: &str) -> i64 {
    let mut bits = 0i64;
    for c in flags.chars() {
        bits |= match c {
            'i' => 1,  // caseless
            'm' => 2,  // multiline ^$
            's' => 4,  // dotall
            'x' => 8,  // extended
            'g' => 16, // global (substitute-all / match-all)
            _ => 0,
        };
    }
    bits
}

/// Process the common escape sequences in a (non-raw) string literal.
fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('0') => out.push(0),
            Some('\\') => out.push(b'\\'),
            Some('\'') => out.push(b'\''),
            Some('"') => out.push(b'"'),
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

/// Evaluate a tier-1 enum discriminant expression (`GREEN = 10`), which the
/// grammar allows to be an integer literal, optionally signed. Richer constant
/// expressions are a follow-up.
fn const_int_value(e: &ast::Expr) -> Option<i64> {
    match e {
        ast::Expr::Int(s, _) => Some(parse_int_lit(s)),
        ast::Expr::Unary {
            op: ast::UnOp::Neg,
            expr,
            ..
        } => const_int_value(expr).map(|v| -v),
        ast::Expr::Unary {
            op: ast::UnOp::Pos,
            expr,
            ..
        } => const_int_value(expr),
        _ => None,
    }
}

fn parse_int_lit(s: &str) -> i64 {
    let clean: String = s.chars().filter(|c| *c != '_').collect();
    let r = |b: &str, radix: u32| i64::from_str_radix(b, radix).unwrap_or(0);
    if let Some(h) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        r(h, 16)
    } else if let Some(o) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        r(o, 8)
    } else if let Some(bn) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        r(bn, 2)
    } else {
        clean.parse::<i64>().unwrap_or(0)
    }
}

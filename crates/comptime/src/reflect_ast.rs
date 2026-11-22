//! Reflection records built **directly from the concrete AST**, with no sema
//! `Checker` (#1251).
//!
//! E1's resolver (`zapc_sema::analyze_reflect`) produces [`TypeInfo`] from the
//! checker's registries; that path depends on sema. Monomorphization does *not*
//! depend on sema — by the time it specializes a `comptime for` the concrete
//! `struct`/`enum` declaration is in hand as plain AST, with field names,
//! declaration order, attributes and spans all present (#1249: the
//! generator/verification boundary). So the unroller reflects the shape it needs
//! straight off the `StructDef` / `EnumDef`, mirroring E1's field/variant shape.
//!
//! `FieldInfo::ty` is a best-effort primitive classification of the *declared*
//! type (`decl_ty` holds the exact declared [`Type`]); resolving a field's full
//! comptime type — the kind-switch recursion — is deferred to #1253, and the
//! evaluator reports `.ty` as out of fragment until then, so an unresolved
//! element type never silently misleads.

use zapc_sema_types::{AttrArg, AttrInfo, FieldInfo, Ty, TypeInfo, TypeShape, VariantInfo};
use zapc_syntax::ast::{
    Decorator, EnumDef, EnumVariant, Expr, Field, StructDef, Type, VariantField,
};

/// Build the [`TypeInfo`] for a concrete `struct` declaration: its declared data
/// fields in declaration order (#1066), each a full [`FieldInfo`].
pub fn type_info_of_struct(def: &StructDef) -> TypeInfo {
    TypeInfo {
        shape: TypeShape::Struct {
            name: def.name.clone(),
            fields: def
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| field_info(i, f))
                .collect(),
        },
    }
}

/// Build the [`TypeInfo`] for a concrete `enum` declaration: its variants in
/// declaration order, each with its payload fields (empty for a tier-1 constant).
pub fn type_info_of_enum(def: &EnumDef) -> TypeInfo {
    TypeInfo {
        shape: TypeShape::Enum {
            name: def.name.clone(),
            variants: def
                .variants
                .iter()
                .enumerate()
                .map(|(i, v)| variant_info(i, v))
                .collect(),
        },
    }
}

fn field_info(index: usize, f: &Field) -> FieldInfo {
    FieldInfo {
        name: f.name.clone(),
        ty: ty_of_decl(&f.ty),
        decl_ty: f.ty.clone(),
        index,
        attrs: f.decorators.iter().map(attr_info_of).collect(),
        span: f.span,
    }
}

fn variant_info(index: usize, v: &EnumVariant) -> VariantInfo {
    VariantInfo {
        name: v.name.clone(),
        index,
        fields: v
            .fields
            .iter()
            .enumerate()
            .map(|(i, vf)| variant_field_info(i, vf))
            .collect(),
        // Enum variant payloads carry no attribute grammar (E1, HQ8).
        attrs: vec![],
        span: v.span,
    }
}

fn variant_field_info(index: usize, vf: &VariantField) -> FieldInfo {
    FieldInfo {
        name: vf.name.clone(),
        ty: ty_of_decl(&vf.ty),
        decl_ty: vf.ty.clone(),
        index,
        attrs: vec![],
        span: vf.span,
    }
}

/// Mirror of E1's `attr_info_of`: a decorator rendered to structured,
/// comptime-queryable data (name + args; no per-attribute compiler meaning).
fn attr_info_of(d: &Decorator) -> AttrInfo {
    AttrInfo {
        name: d.name.clone(),
        args: d
            .args
            .iter()
            .map(|a| AttrArg {
                name: a.name.clone(),
                value: render_attr_value(&a.value),
            })
            .collect(),
        span: d.span,
    }
}

/// Mirror of E1's `render_attr_value`: an attribute argument expression rendered
/// to its canonical string (a string literal without quotes, a numeric lexeme,
/// `true`/`false`, a bare name, or `…` for a form E1 does not render).
fn render_attr_value(e: &Expr) -> String {
    match e {
        Expr::Int(lex, _) | Expr::Float(lex, _) | Expr::Imaginary(lex, _) => lex.clone(),
        Expr::Str { value, .. } => value.clone(),
        Expr::Bool(b, _) => b.to_string(),
        Expr::None(_) => "None".to_string(),
        Expr::Name(n, _) => n.clone(),
        _ => "…".to_string(),
    }
}

/// Best-effort classification of a declared primitive type to its canonical
/// [`Ty`]. A non-primitive (container, user type, generic parameter) maps to
/// [`Ty::Unknown`] — `decl_ty` keeps the exact declared type, and full
/// resolution of a field's element/kind is #1253.
fn ty_of_decl(t: &Type) -> Ty {
    let Type::Named { name, args, .. } = t else {
        return Ty::Unknown;
    };
    if !args.is_empty() {
        return Ty::Unknown;
    }
    match name.as_str() {
        "int64" => Ty::Int64,
        "int32" => Ty::Int32,
        "uint8" => Ty::Uint8,
        "uint16" => Ty::Uint16,
        "uint32" => Ty::Uint32,
        "uint64" => Ty::Uint64,
        "usize" => Ty::Usize,
        "float64" => Ty::Float64,
        "float32" => Ty::Float32,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        "bytes" => Ty::Bytes,
        "char" => Ty::Char,
        "bigint" => Ty::Bigint,
        "complex" => Ty::Complex,
        _ => Ty::Unknown,
    }
}

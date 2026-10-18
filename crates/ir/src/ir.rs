//! ZIR data structures — see `design/IR.md`.
//!
//! A [`Module`] holds [`Func`]s. Each function is SSA over numbered [`Value`]s
//! (values `0..params.len()` are the parameters) with a per-value type table,
//! and a list of [`Block`]s each ending in a [`Term`]. Mutable locals are
//! lowered via stack slots (`Alloca`/`Load`/`Store`); control flow is explicit.

use std::fmt;

/// A ZIR type (lower-level than a Zap type; reference types become `Ptr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    I1,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Ptr,
    /// A pointer to a struct instance; the `u32` indexes [`Module::structs`].
    Struct(u32),
    Void,
}

impl Ty {
    pub fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
    pub fn int_width(self) -> Option<u32> {
        match self {
            Ty::I1 => Some(1),
            Ty::I8 => Some(8),
            Ty::I16 => Some(16),
            Ty::I32 => Some(32),
            Ty::I64 => Some(64),
            _ => None,
        }
    }
}

/// An SSA value id (index into [`Func::value_types`]).
pub type Value = u32;
/// A basic-block id (index into [`Func::blocks`]).
pub type BlockId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div, // sdiv for ints, fdiv for floats (chosen by result type)
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr, // arithmetic (signed) right shift
}

/// Comparison predicate (signed for ints, ordered for floats).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpPred {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    SExt,
    Trunc,
    SIToFP,
    FPToSI,
    FPExt,
    FPTrunc,
}

#[derive(Clone, Debug)]
pub enum Inst {
    IConst {
        dest: Value,
        val: i64,
    },
    FConst {
        dest: Value,
        val: f64,
    },
    BConst {
        dest: Value,
        val: bool,
    },
    Bin {
        dest: Value,
        op: BinOp,
        lhs: Value,
        rhs: Value,
    },
    ICmp {
        dest: Value,
        pred: CmpPred,
        lhs: Value,
        rhs: Value,
    },
    FCmp {
        dest: Value,
        pred: CmpPred,
        lhs: Value,
        rhs: Value,
    },
    Un {
        dest: Value,
        op: UnOp,
        val: Value,
    },
    Cast {
        dest: Value,
        kind: CastKind,
        val: Value,
    },
    /// `dest : Ptr` is a stack slot holding a `pointee`.
    Alloca {
        dest: Value,
        pointee: Ty,
    },
    /// Load `value_types[dest]` from `ptr`.
    Load {
        dest: Value,
        ptr: Value,
    },
    Store {
        ptr: Value,
        val: Value,
    },
    Select {
        dest: Value,
        cond: Value,
        a: Value,
        b: Value,
    },
    Call {
        dest: Option<Value>,
        func: String,
        args: Vec<Value>,
    },
    /// `print(...)` — lowered to runtime output; args printed space-separated.
    Print {
        args: Vec<Value>,
    },
    /// A string literal: `dest : Ptr` is a freshly built string object holding
    /// `bytes` (interned via the runtime).
    StrConst {
        dest: Value,
        bytes: Vec<u8>,
    },
    /// Allocate a zeroed struct instance; `dest : Struct(sid)`.
    StructNew {
        dest: Value,
        sid: u32,
    },
    /// Load field `idx` of struct `sid` from `base`; `dest` gets the field type.
    FieldGet {
        dest: Value,
        base: Value,
        sid: u32,
        idx: u32,
    },
    /// Store `val` into field `idx` of struct `sid` at `base`.
    FieldSet {
        base: Value,
        sid: u32,
        idx: u32,
        val: Value,
    },
}

#[derive(Clone, Debug)]
pub enum Term {
    Ret(Option<Value>),
    Br(BlockId),
    BrIf {
        cond: Value,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    Unreachable,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub insts: Vec<Inst>,
    pub term: Term,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub is_main: bool,
    /// Type of every value; `value_types[0..params.len()]` are the parameters.
    pub value_types: Vec<Ty>,
    pub blocks: Vec<Block>,
}

impl Func {
    pub fn value_ty(&self, v: Value) -> Ty {
        self.value_types[v as usize]
    }
}

/// A struct layout: a name and its field types, in declaration order.
#[derive(Clone, Debug)]
pub struct StructLayout {
    pub name: String,
    pub fields: Vec<Ty>,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub structs: Vec<StructLayout>,
    pub funcs: Vec<Func>,
}

// --- textual form (for `zapc ir` and tests) ---

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Ty::I1 => "i1",
            Ty::I8 => "i8",
            Ty::I16 => "i16",
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::F32 => "f32",
            Ty::F64 => "f64",
            Ty::Ptr => "ptr",
            Ty::Void => "void",
            Ty::Struct(id) => return write!(f, "struct#{id}"),
        };
        f.write_str(s)
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for func in &self.funcs {
            let params: Vec<String> = func
                .params
                .iter()
                .enumerate()
                .map(|(i, t)| format!("%{i}: {t}"))
                .collect();
            writeln!(
                f,
                "func @{}({}) -> {} {{",
                func.name,
                params.join(", "),
                func.ret
            )?;
            for b in &func.blocks {
                writeln!(f, "  block{}:", b.id)?;
                for inst in &b.insts {
                    writeln!(f, "    {}", fmt_inst(inst))?;
                }
                writeln!(f, "    {}", fmt_term(&b.term))?;
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}

fn fmt_inst(i: &Inst) -> String {
    match i {
        Inst::IConst { dest, val } => format!("%{dest} = iconst {val}"),
        Inst::FConst { dest, val } => format!("%{dest} = fconst {val}"),
        Inst::BConst { dest, val } => format!("%{dest} = bconst {val}"),
        Inst::Bin { dest, op, lhs, rhs } => format!("%{dest} = {op:?} %{lhs}, %{rhs}"),
        Inst::ICmp {
            dest,
            pred,
            lhs,
            rhs,
        } => format!("%{dest} = icmp.{pred:?} %{lhs}, %{rhs}"),
        Inst::FCmp {
            dest,
            pred,
            lhs,
            rhs,
        } => format!("%{dest} = fcmp.{pred:?} %{lhs}, %{rhs}"),
        Inst::Un { dest, op, val } => format!("%{dest} = {op:?} %{val}"),
        Inst::Cast { dest, kind, val } => format!("%{dest} = {kind:?} %{val}"),
        Inst::Alloca { dest, pointee } => format!("%{dest} = alloca {pointee}"),
        Inst::Load { dest, ptr } => format!("%{dest} = load %{ptr}"),
        Inst::Store { ptr, val } => format!("store %{ptr}, %{val}"),
        Inst::Select { dest, cond, a, b } => format!("%{dest} = select %{cond}, %{a}, %{b}"),
        Inst::Call { dest, func, args } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            match dest {
                Some(d) => format!("%{d} = call @{func}({})", a.join(", ")),
                None => format!("call @{func}({})", a.join(", ")),
            }
        }
        Inst::Print { args } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            format!("print({})", a.join(", "))
        }
        Inst::StrConst { dest, bytes } => {
            format!("%{dest} = strconst {:?}", String::from_utf8_lossy(bytes))
        }
        Inst::StructNew { dest, sid } => format!("%{dest} = structnew struct#{sid}"),
        Inst::FieldGet {
            dest,
            base,
            sid,
            idx,
        } => {
            format!("%{dest} = fieldget struct#{sid} %{base}.{idx}")
        }
        Inst::FieldSet {
            base,
            sid,
            idx,
            val,
        } => {
            format!("fieldset struct#{sid} %{base}.{idx} = %{val}")
        }
    }
}

fn fmt_term(t: &Term) -> String {
    match t {
        Term::Ret(Some(v)) => format!("ret %{v}"),
        Term::Ret(None) => "ret".to_string(),
        Term::Br(b) => format!("br block{b}"),
        Term::BrIf {
            cond,
            then_bb,
            else_bb,
        } => {
            format!("br_if %{cond}, block{then_bb}, block{else_bb}")
        }
        Term::Unreachable => "unreachable".to_string(),
    }
}

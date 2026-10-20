//! `yaml` module — runtime-symbol declarations (ticket #149, D-0063).
//!
//! `yaml.load`/`dump` are Zap prelude functions built on the existing json DOM
//! helpers, so the only yaml-specific runtime symbol is the YAML -> JSON
//! transpiler cursor (`zap_yaml_to_json`), which lives in the yaml section of
//! `zaprt.c`. `str`/`bytes` values are pointers.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // yaml._to_json(s) -> str: transpile the supported YAML subset to JSON.
    cg.declare_rt("zap_yaml_to_json", &[p.into()], Some(p.into()));
    // @derive(Yaml) `to_yaml` block encoder: indent a nested child's YAML.
    cg.declare_rt("zap_yaml_indent", &[p.into(), p.into()], Some(p.into()));
}

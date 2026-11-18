//! `uuid` module — runtime-symbol declarations (ticket #975, D-0232/D-0233).
//!
//! RFC 4122 / RFC 9562 UUIDs + NanoID. The C implementations live in
//! `runtime/uuid.inc.c` (reusing crypto.inc.c's CSPRNG / MD5 / SHA-1). A UUID
//! value is 16 raw bytes as a `ZapStr*`; `str`/`bytes` all share the `ZapStr*`
//! ABI. The generators and `parse` return the raw bytes (the lowering wraps them
//! in the `Uuid` struct); the accessors take the raw bytes.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());

    // Zero-argument generators + Appendix C namespace constants -> ZapStr* (16 bytes).
    for f in [
        "zap_uuid_v4",
        "zap_uuid_v7",
        "zap_uuid_v1",
        "zap_uuid_nil",
        "zap_uuid_max",
        "zap_uuid_ns_dns",
        "zap_uuid_ns_url",
        "zap_uuid_ns_oid",
        "zap_uuid_ns_x500",
    ] {
        cg.declare_rt(f, &[], Some(p.into()));
    }

    // Namespaced: (ns_raw: ZapStr*, name: ZapStr*) -> ZapStr* (16 bytes).
    cg.declare_rt("zap_uuid_v3", &[p.into(), p.into()], Some(p.into()));
    cg.declare_rt("zap_uuid_v5", &[p.into(), p.into()], Some(p.into()));

    // parse(s: ZapStr*) -> ZapStr* (16 bytes); raises ValueError on bad input.
    cg.declare_rt("zap_uuid_parse", &[p.into()], Some(p.into()));

    // Accessors over the raw 16 bytes.
    cg.declare_rt("zap_uuid_format", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_uuid_hex", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_uuid_urn", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_uuid_version", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_uuid_variant", &[p.into()], Some(i.into()));

    // NanoID: (alphabet: ZapStr*, size: i64) -> ZapStr*.
    cg.declare_rt("zap_uuid_nanoid", &[p.into(), i.into()], Some(p.into()));
}

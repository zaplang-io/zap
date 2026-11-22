//! Comptime / `@derive` diagnostics — the HQ4 rendering layer (#1256).
//!
//! The evaluator (#1250) produces a span-attributed [`CtError`] and never
//! panics. This module turns one into the human-facing diagnostic a programmer
//! reads, reusing the front-end's stable [`ErrorCode`] scheme (#239) and its
//! `file:line:column` + caret renderer (#571) so a comptime failure looks like
//! any other Zap error. It adds three things HQ4 calls for:
//!
//! 1. **A stable code per failure** ([`error_code`]): an engine fault/cap maps
//!    to `ZAP7001`, a library-authored `comptime.error(...)` to `ZAP7002`, and
//!    a derive that cannot apply to `ZAP7003`.
//! 2. **A derive / instantiation backtrace** ([`DeriveBacktrace`]): the rendered
//!    `required because of …` chain (Rust's model), threading the per-field /
//!    per-element spans the generator descends through (the unroller #1251 and
//!    the kind-switch #1253) from the `@derive` consumer down to the failing
//!    point.
//! 3. **Consumer-grade derive errors** ([`render_derive_error`]) and
//!    **dual-span generator faults** ([`render_generator_fault`]): a
//!    `@derive(X)` that cannot apply points at the *user's own field* with a
//!    "cannot derive X for T" header (never a comptime-internal symbol), and a
//!    bug *inside* a generator cites **both** the generator line and the
//!    instantiation site — never a crash.
//!
//! # Driver-built, host-tested now
//!
//! The backtrace is assembled by the generator *driver* (mono / E3) as it
//! descends — the same layer [`crate::SpecializationCache`] (#1255) models — not
//! by the evaluator, which only supplies the leaf [`CtError`]. E2 ships the data
//! model and the rendering, host-tested; E3 wires the real descent into it. The
//! backtrace polish (multi-file frames, richer labels) continues into E7
//! (#1248); v1 renders every frame against one compilation unit's source.

use zapc_diag::render::{render_diagnostic, render_note};
use zapc_diag::ErrorCode;
use zapc_syntax::span::Span;

use crate::error::{CtError, CtErrorKind};

/// The stable `ZAPnnnn` code for a comptime failure, by kind (#1256).
///
/// - [`CtErrorKind::NotAvailable`] / [`CtErrorKind::Value`] /
///   [`CtErrorKind::Budget`] → `ZAP7001` ("comptime evaluation error"): the
///   engine rejected a construct or hit a fault/cap.
/// - [`CtErrorKind::UserError`] → `ZAP7002` ("error raised by comptime code"):
///   a library deliberately aborted with `comptime.error(...)`.
///
/// The derive-consumer code `ZAP7003` ("cannot derive X for T") is *not*
/// produced from a `CtError` kind — it is the header the derive driver stamps
/// on top of whatever inner cause via [`render_derive_error`].
pub fn error_code(err: &CtError) -> ErrorCode {
    match err.kind {
        CtErrorKind::NotAvailable | CtErrorKind::Value | CtErrorKind::Budget => ErrorCode::ZAP7001,
        CtErrorKind::UserError => ErrorCode::ZAP7002,
    }
}

/// One frame of a derive / instantiation backtrace (#1256): a human-readable
/// `label` and the source `span` it points at. Rendered as a single
/// `required because of <label>` note (Rust's "required because …" style).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// What this context *is*, phrased to follow "required because of " — e.g.
    /// `` field `tags` of `User` `` or `` element type of `list[Widget]` ``.
    pub label: String,
    /// The source span this frame points at (the field, the element type, …).
    pub span: Span,
}

impl Frame {
    /// A frame pointing `label` at `span`.
    pub fn new(label: impl Into<String>, span: Span) -> Frame {
        Frame {
            label: label.into(),
            span,
        }
    }
}

/// A derive / instantiation backtrace: the chain of contexts an error unwound
/// through (#1256, HQ4). Frames are stored in **descent order** — the driver
/// pushes the outermost context (the `@derive` consumer) first and the one
/// nearest the failing point last — which is how a generator naturally descends
/// through the unroller (#1251) and kind-switch (#1253).
///
/// Rendering ([`render_ct_error`]) walks them *inner-first* (the context nearest
/// the failure, climbing outward), each as a `required because of …` note, so
/// the chain reads from the failing point back up to the derive site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeriveBacktrace {
    frames: Vec<Frame>,
}

impl DeriveBacktrace {
    /// An empty backtrace (the failure needs no instantiation context).
    pub fn new() -> DeriveBacktrace {
        DeriveBacktrace::default()
    }

    /// Push one outer context frame as the driver descends. The first pushed is
    /// the outermost consumer; the last is nearest the failing point.
    pub fn push(&mut self, frame: Frame) {
        self.frames.push(frame);
    }

    /// Builder form of [`push`](DeriveBacktrace::push), for assembling a
    /// backtrace inline (and in tests).
    pub fn with(mut self, frame: Frame) -> DeriveBacktrace {
        self.frames.push(frame);
        self
    }

    /// The frames, in descent order (outermost first).
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The number of context frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// `true` when there is no instantiation context to show.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Render a comptime failure as the final multi-line diagnostic (#1256): the
/// primary error (its stable code via [`error_code`], at `err.span`, with the
/// offending source line and caret), followed by one `required because of …`
/// note per backtrace frame — walked **inner-first** (nearest the failure,
/// climbing to the derive site). An empty backtrace renders the primary error
/// alone. Every frame is rendered against `src`/`file` (one compilation unit;
/// multi-file frames are E7 polish, #1248).
pub fn render_ct_error(
    src: &str,
    file: &str,
    err: &CtError,
    backtrace: &DeriveBacktrace,
) -> String {
    let mut out = render_diagnostic(
        Some(src),
        file,
        error_code(err),
        &err.message,
        Some((err.span.start, err.span.end)),
    );
    append_backtrace(&mut out, src, file, backtrace);
    out
}

/// Render a **consumer-grade** "cannot derive `trait_name` for `type_name`"
/// diagnostic (#1256, `ZAP7003`), pointing at the user's own offending
/// declaration (`at`, typically a field's span). The underlying `cause` (an
/// engine fault, or a library `comptime.error`) and the `backtrace` follow as
/// notes, so the user sees *their* field and source line — never a
/// comptime-internal symbol. This is the message the ordinary programmer who
/// only wrote `@derive(Json)` sees; the authoring-tier tolerance (#1249) does
/// **not** relax it.
pub fn render_derive_error(
    src: &str,
    file: &str,
    trait_name: &str,
    type_name: &str,
    at: Span,
    cause: &CtError,
    backtrace: &DeriveBacktrace,
) -> String {
    let header = format!("cannot derive `{trait_name}` for `{type_name}`");
    let mut out = render_diagnostic(
        Some(src),
        file,
        ErrorCode::ZAP7003,
        &header,
        Some((at.start, at.end)),
    );
    // The backtrace (inner-first) sits between the header and the root cause,
    // so the reader climbs from the field to the derive site, then sees why.
    append_backtrace(&mut out, src, file, backtrace);
    // The root cause last, as a located note at its own span — the library's
    // own words for a `comptime.error`, or the engine's for a fault.
    out.push('\n');
    out.push_str(&render_note(
        Some(src),
        file,
        &format!("{}: {}", cause_lead(cause), cause.message),
        Some((cause.span.start, cause.span.end)),
    ));
    out
}

/// Render a **dual-span** generator-fault diagnostic (#1256): a fault *inside* a
/// generator (an engine error, `err`) is cited at the generator line
/// (`generator_span`) **and** at the instantiation site (`instantiation_span`),
/// so a generator bug is a clean compile error naming both places, never a
/// panic. Use this for an engine fault within generator code; a deliberate
/// `comptime.error` is a `ZAP7002` user abort (render it with
/// [`render_ct_error`] / [`render_derive_error`]).
pub fn render_generator_fault(
    src: &str,
    file: &str,
    err: &CtError,
    generator_span: Span,
    instantiation_span: Span,
) -> String {
    let message = format!("error in comptime generator: {}", err.message);
    let mut out = render_diagnostic(
        Some(src),
        file,
        error_code(err),
        &message,
        Some((generator_span.start, generator_span.end)),
    );
    out.push('\n');
    out.push_str(&render_note(
        Some(src),
        file,
        "instantiated here",
        Some((instantiation_span.start, instantiation_span.end)),
    ));
    out
}

/// Append the `required because of …` notes for `backtrace`, inner-first.
fn append_backtrace(out: &mut String, src: &str, file: &str, backtrace: &DeriveBacktrace) {
    for frame in backtrace.frames().iter().rev() {
        out.push('\n');
        out.push_str(&render_note(
            Some(src),
            file,
            &format!("required because of {}", frame.label),
            Some((frame.span.start, frame.span.end)),
        ));
    }
}

/// The lead-in for a root-cause note: a library `comptime.error` is the author's
/// own message, so it reads "the generator reported"; an engine fault reads
/// "comptime evaluation failed".
fn cause_lead(cause: &CtError) -> &'static str {
    if cause.is_user() {
        "the generator reported"
    } else {
        "comptime evaluation failed"
    }
}

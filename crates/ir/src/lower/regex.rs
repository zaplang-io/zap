//! Regex lowering (ticket #23, D-0010).
//!
//! `m/.../`, `s/.../.../`, `=~`/`!~` lower to PCRE2-backed `zap_regex_*` runtime
//! calls; no new ZIR instruction (the string/collection pattern). A match object
//! is a GC'd heap value owning its captured groups, so capture state is
//! coroutine-safe. Extracted verbatim from `lower.rs` (#1126); `use super::*`
//! re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    /// `subject =~ m/pattern/flags` → a match object (`Ty::Match`, null = no
    /// match), or, for `!~`, an `i1` that is true when there is no match.
    pub(super) fn lower_regex_match(
        &mut self,
        subject: &Expr,
        pattern: &str,
        flags: &str,
        negated: bool,
    ) -> Result<(Value, Ty), LowerError> {
        let (sv, _st) = self.expr(subject)?;
        let pv = self.strconst(pattern);
        let fv = self.iconst(Ty::I64, regex_flag_bits(flags));
        // The global form `=~ m/.../g` (non-negated) yields an iterable of all
        // ordered, non-overlapping matches, collected eagerly into a
        // `list[Match]` (#1078, D-0010 — Python `re.finditer`). `!~ m/.../g` is
        // still a single truthiness test, so the `g` flag is irrelevant there
        // and it takes the single-match path below. No new ZIR instruction: the
        // value is an ordinary pointer result of a runtime call, rooted like any
        // other `Inst::Call` dest (the string/collection pattern).
        if !negated && flags.contains('g') {
            // A `list[Match]`: match objects are reference values stored as
            // pointer words (pushed with `zap_list_push_ptr`, read with
            // `zap_list_get_ptr`), so the list traces them for GC like
            // `list[str]`. We intern the element as `Elem::Nested(Ty::Match)`
            // (the #625 mechanism) rather than a bare `Elem::Ptr` so read-back —
            // `for mt in ...` — recovers the precise `Ty::Match` and `mt.group()`
            // dispatches; a flat pointer would default to `str` (ZAP6001).
            let elem = Elem::Nested(self.ctx.nested.borrow_mut().intern(Ty::Match));
            let list_ty = Ty::List(elem);
            let l = self.new_value(list_ty);
            self.emit(Inst::Call {
                dest: Some(l),
                func: "zap_regex_match_all".to_string(),
                args: vec![sv, pv, fv],
            });
            return Ok((l, list_ty));
        }
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
    pub(super) fn lower_regex_subst(
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
}

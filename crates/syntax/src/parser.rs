//! Recursive-descent parser with a precedence-climbing ("Pratt") expression
//! parser (Phase 1).
//!
//! Consumes the token stream from [`crate::lexer`] (including the synthesized
//! `Newline`/`Indent`/`Dedent` layout tokens) and produces the [`crate::ast`].
//! Errors are collected rather than aborting, with best-effort recovery to the
//! next line.
//!
//! Precedence (low → high): lambda · `if`/`else` · `or` · `and` · `not` ·
//! comparison / `in` / `=~` / `!~` · `|` · `^` · `&` · `<<`/`>>` · `+`/`-` ·
//! `*`/`/`/`//`/`%` · unary `+`/`-`/`~` · `**` · call/index/slice/attr.

use crate::ast::*;
use crate::lexer::{tokenize, tokenize_with_trivia};
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};
use crate::trivia::Comment;
use zapc_diag::ErrorCode;

/// A parse error with its stable [`ErrorCode`] and the offending source span.
///
/// The `code` is the machine-parseable identifier (see `zapc_diag`); `message`
/// is human-facing prose that may change between releases.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub code: ErrorCode,
    pub message: String,
    pub span: Span,
}

/// Parse a token stream into a module plus any errors.
pub fn parse(tokens: Vec<Token>) -> (Module, Vec<ParseError>) {
    parse_with_source(tokens, "")
}

/// Like [`parse`], but with the original source so nodes that need to echo the
/// exact source text (e.g. `assert` for power-assert diagnostics, D-0054) can
/// slice it by span.
pub fn parse_with_source(tokens: Vec<Token>, src: &str) -> (Module, Vec<ParseError>) {
    let mut p = Parser::new(tokens, src);
    let module = p.parse_module();
    (module, p.errors)
}

/// Convenience: lex and parse `src`. Lex errors are returned alongside parse
/// errors.
pub fn parse_str(src: &str) -> (Module, Vec<ParseError>, Vec<crate::lexer::LexError>) {
    let (tokens, lex_errors) = tokenize(src);
    let (module, parse_errors) = parse_with_source(tokens, src);
    (module, parse_errors, lex_errors)
}

/// Like [`parse_str`], but also recovers comment trivia (I#157/A1, #225) for
/// tooling that must re-emit source faithfully (`zap fmt`, `zap doc`). The
/// `Module`/errors are identical to [`parse_str`]; the `Vec<Comment>` is the
/// additive side-channel.
pub fn parse_str_with_trivia(
    src: &str,
) -> (
    Module,
    Vec<ParseError>,
    Vec<crate::lexer::LexError>,
    Vec<Comment>,
) {
    let (tokens, lex_errors, comments) = tokenize_with_trivia(src);
    let (module, parse_errors) = parse_with_source(tokens, src);
    (module, parse_errors, lex_errors, comments)
}

/// Parse a single stand-alone expression from `src` (an f-string replacement
/// field, D-0026). Token spans are shifted by `base` so diagnostics land inside
/// the enclosing f-string literal rather than at offset 0. Returns the parsed
/// expression together with any lex/parse errors, including a trailing-token
/// error when `src` is not a single expression (e.g. `f"{a b}"`).
pub fn parse_expr_fragment(src: &str, base: u32) -> (Expr, Vec<ParseError>) {
    let (mut tokens, lex_errors) = tokenize(src);
    for t in &mut tokens {
        t.span = Span::new(
            t.span.start.saturating_add(base),
            t.span.end.saturating_add(base),
        );
    }
    let mut errors: Vec<ParseError> = lex_errors
        .into_iter()
        .map(|e| ParseError {
            code: e.code,
            message: e.message,
            span: Span::new(
                e.span.start.saturating_add(base),
                e.span.end.saturating_add(base),
            ),
        })
        .collect();
    let mut p = Parser::new(tokens, "");
    let expr = p.parse_expr();
    // A bare expression lexes with trailing layout tokens (a synthesized
    // `Newline`, then `Eof`); only a real token left over means the field held
    // more than one expression.
    while matches!(
        p.peek(),
        TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
    ) {
        p.bump();
    }
    if !matches!(p.peek(), TokenKind::Eof) {
        errors.push(ParseError {
            code: ErrorCode::ZAP2017,
            message: "f-string field must be a single expression".to_string(),
            span: p.peek_span(),
        });
    }
    errors.append(&mut p.errors);
    (expr, errors)
}

/// The compiler-known built-in prelude (ticket #77, D-0058; ticket #99): the
/// `Signal` enum and `MemStats`/`DateTime` structs that back the `signal`/`gc`/
/// `runtime`/`datetime` surface. They are injected ahead of the user's items
/// (see [`prelude_items`]) so every ordinary enum/struct path — member access,
/// `match`, `channel[Signal]`, field access — works unchanged. A `Signal`
/// variant's backing integer IS its Unix (Linux) signal number, so the runtime
/// ABI speaks raw signal numbers. SIGKILL/SIGSTOP are intentionally absent
/// (uncatchable). `MemStats` mirrors `gc.stats()`; `DateTime` is the civil-time
/// value `datetime.*` produces (its methods live on the module, so the struct
/// itself stays method-free and emits no code when a program never uses it).
/// `KeyInfo` is the validated asymmetric-key metadata `crypto.key_info()`
/// returns (#643, D-0126) — likewise method-free and zero-code when unused.
/// `JsonValue` is the dynamic JSON DOM sum type (D-0059, I#100/I#131): a
/// data-carrying enum (D-0046) with `Int(int64)`/`Float(float64)` kept distinct,
/// and recursive `Array(list[JsonValue])` / `Object(dict[str, JsonValue])`
/// variants; `json.loads`/`dumps` and `@derive(Json)` are built on it.
pub const PRELUDE_SRC: &str = "\
enum Signal:
    SIGHUP = 1
    SIGINT = 2
    SIGQUIT = 3
    SIGABRT = 6
    SIGUSR1 = 10
    SIGUSR2 = 12
    SIGPIPE = 13
    SIGALRM = 14
    SIGTERM = 15
    SIGCHLD = 17
    SIGCONT = 18
    SIGTSTP = 20
    SIGWINCH = 28

value struct MemStats:
    int64 heap_bytes
    int64 live_bytes
    int64 free_bytes
    int64 gc_count
    int64 coroutines

value struct DateTime:
    int64 year
    int64 month
    int64 day
    int64 hour
    int64 minute
    int64 second

value struct KeyInfo:
    str kind
    int64 bits
    str curve
    bool is_private

enum Optional[T]:
    Some(T value)
    None

    def is_some(self) -> bool:
        match self:
            case Optional.Some(v):
                return True
            case Optional.None:
                return False

    def is_none(self) -> bool:
        match self:
            case Optional.Some(v):
                return False
            case Optional.None:
                return True

    def unwrap(self) -> T:
        match self:
            case Optional.Some(v):
                return v
            case Optional.None:
                raise \"called unwrap on an Optional.None value\"

    def unwrap_or(self, T fallback) -> T:
        match self:
            case Optional.Some(v):
                return v
            case Optional.None:
                return fallback

    def and_opt[U](self, Optional[U] other) -> Optional[U]:
        match self:
            case Optional.Some(v):
                return other
            case Optional.None:
                return None

# Atomic[T] — lock-free atomic cell (I#176, sync primitive alongside Mutex/
# WaitGroup/Once). A machine-word wrapper over a native type T (int64/int32/bool
# or a reference). All operations are sequentially consistent. The method bodies
# below are compiler intrinsics: lowering recognizes an `Atomic[T]` receiver and
# emits inline LLVM atomic instructions (atomic load/store, atomicrmw, cmpxchg)
# over the `value` cell, so these source bodies are never executed. They are
# written as plain field access only so the generic surface type-checks and
# monomorphizes like any other generic struct (which also yields the correct
# per-instance GC descriptor — a reference cell is traced, a scalar cell is not).
struct Atomic[T]:
    T value

    def load(self) -> T:
        return self.value

    def store(self, T v):
        self.value = v

    def add(self, T n) -> T:
        return n

    def sub(self, T n) -> T:
        return n

    def swap(self, T v) -> T:
        return v

    def compare_and_swap(self, T old, T new) -> bool:
        return False

# threading (I#175) — Python-`threading`-shaped OS-thread compat (migration
# escape hatch, NOT recommended; idiomatic Zap = coroutines + channels). A
# `Thread` runs its target on a dedicated OS thread beyond MAXPROCS by pinning
# its coroutine via the #172 LockOSThread substrate, so it still participates in
# precise-GC stop-the-world + safepoints and roots through the shadow stack. The
# method bodies delegate to the `threading` module's flat runtime entry points.
struct Thread:
    fn() _target
    bytes _handle
    bool _daemon

    def start(self):
        # Register liveness synchronously (before the coroutine runs, so a
        # non-daemon thread has no exit race), then launch the pinned trampoline.
        threading._register(self._handle, self._daemon)
        spawn detached __thread_entry(self._target, self._handle, self._daemon)

    def join(self):
        threading._join(self._handle)

    def join_timeout(self, float64 timeout) -> bool:
        return threading._join_timeout(self._handle, timeout)

    def is_alive(self) -> bool:
        return threading._is_alive(self._handle)

# The thread body: pin to a dedicated OS thread, run the target, and always
# publish completion + unpin (even if the target raises).
def __thread_entry(fn() target, bytes handle, bool daemon):
    threading._pin()
    try:
        target()
    finally:
        threading._unpin()
        threading._finish(handle, daemon)

# The synchronization primitives are implemented ONCE in the runtime and exposed
# on two surfaces: the flat `sync.*` functions (idiomatic, over opaque handles)
# and these Python-`threading`-shaped wrappers. Each wrapper is a thin reference
# struct over the same handle; its methods delegate to `sync.*`. The method
# bodies run on whatever coroutine (or dedicated Thread) calls them, parking
# cooperatively on the shared scheduler primitive — so they work equally from a
# coroutine and from a `threading.Thread`.

# Lock — non-reentrant mutual exclusion (reuses sync.Mutex).
struct Lock:
    bytes _h

    def acquire(self):
        sync.lock(self._h)

    def acquire_timeout(self, float64 timeout) -> bool:
        return sync.lock_timeout(self._h, timeout)

    def try_acquire(self) -> bool:
        return sync.trylock(self._h)

    def release(self):
        sync.unlock(self._h)

    def __enter__(self) -> Lock:
        sync.lock(self._h)
        return self

    def __exit__(self):
        sync.unlock(self._h)

# RLock — reentrant lock, owned by the acquiring coroutine/Thread.
struct RLock:
    bytes _h

    def acquire(self):
        sync.rlock_lock(self._h)

    def try_acquire(self) -> bool:
        return sync.rlock_trylock(self._h)

    def release(self):
        sync.rlock_unlock(self._h)

    def __enter__(self) -> RLock:
        sync.rlock_lock(self._h)
        return self

    def __exit__(self):
        sync.rlock_unlock(self._h)

# Event — a latch; `wait` blocks until `set_` (Python's `set`, renamed: `set`
# is a reserved type keyword in Zap).
struct Event:
    bytes _h

    def set_(self):
        sync.event_set(self._h)

    def clear(self):
        sync.event_clear(self._h)

    def is_set(self) -> bool:
        return sync.event_is_set(self._h)

    def wait(self):
        sync.event_wait(self._h)

    def wait_timeout(self, float64 timeout) -> bool:
        return sync.event_wait_timeout(self._h, timeout)

# Semaphore — counting semaphore.
struct Semaphore:
    bytes _h

    def acquire(self):
        sync.sem_acquire(self._h)

    def acquire_timeout(self, float64 timeout) -> bool:
        return sync.sem_acquire_timeout(self._h, timeout)

    def release(self):
        sync.sem_release(self._h)

    def __enter__(self) -> Semaphore:
        sync.sem_acquire(self._h)
        return self

    def __exit__(self):
        sync.sem_release(self._h)

# Condition — a condition variable over its own internal lock (matches Python's
# default `Condition()`). `wait` atomically releases the lock, parks, and
# reacquires on wake.
struct Condition:
    bytes _h

    def acquire(self):
        sync.cond_acquire(self._h)

    def release(self):
        sync.cond_release(self._h)

    def __enter__(self) -> Condition:
        sync.cond_acquire(self._h)
        return self

    def __exit__(self):
        sync.cond_release(self._h)

    def wait(self):
        sync.cond_wait(self._h)

    def notify(self, int64 n = 1):
        sync.cond_notify(self._h, n)

    def notify_all(self):
        sync.cond_notify_all(self._h)

enum JsonValue:
    Null
    Bool(bool b)
    Int(int64 i)
    Float(float64 f)
    Str(str s)
    Array(list[JsonValue] items)
    Object(dict[str, JsonValue] fields)

def __json_value(bytes c, int64 k) -> JsonValue:
    if k == 1:
        return JsonValue.Null
    if k == 2:
        return JsonValue.Bool(True)
    if k == 3:
        return JsonValue.Bool(False)
    if k == 4:
        return JsonValue.Int(json._lex_int(c))
    if k == 5:
        return JsonValue.Float(json._lex_float(c))
    if k == 6:
        return JsonValue.Str(json._lex_str(c))
    if k == 9:
        return __json_array(c)
    if k == 7:
        return __json_object(c)
    return JsonValue.Null

# The DOM builders assume `_lex_new` already validated a complete document
# (zap_json_document_valid), so a well-formed array always closes with EndArray
# and a well-formed object with EndObject. The explicit End (token 0) guard below
# is defence-in-depth: without it a truncated array (e.g. `[1,`) would spin
# forever re-reading End at EOF. It raises a catchable error instead.
def __json_array(bytes c) -> JsonValue:
    list[JsonValue] xs = []
    auto k = json._lex_next(c)
    while k != 10:
        if k == 0:
            raise \"truncated JSON array: unexpected end of input\"
        xs.append(__json_value(c, k))
        k = json._lex_next(c)
        if k == 12:
            k = json._lex_next(c)
    return JsonValue.Array(xs)

def __json_object(bytes c) -> JsonValue:
    dict[str, JsonValue] m = {}
    auto k = json._lex_next(c)
    while k == 6:
        auto key = json._lex_str(c)
        json._lex_next(c)
        auto vk = json._lex_next(c)
        m[key] = __json_value(c, vk)
        k = json._lex_next(c)
        if k == 12:
            k = json._lex_next(c)
    return JsonValue.Object(m)

def __json_loads(str s) -> JsonValue:
    auto c = json._lex_new(s)
    auto k = json._lex_next(c)
    return __json_value(c, k)

def __json_dump(JsonValue v, list[str] out):
    match v:
        case JsonValue.Null:
            out.append(\"null\")
        case JsonValue.Bool(b):
            if b:
                out.append(\"true\")
            else:
                out.append(\"false\")
        case JsonValue.Int(i):
            out.append(json._i64_str(i))
        case JsonValue.Float(f):
            out.append(json._f64_str(f))
        case JsonValue.Str(s):
            out.append(json._escape(s))
        case JsonValue.Array(items):
            out.append(\"[\")
            auto first = True
            for x in items:
                if first:
                    first = False
                else:
                    out.append(\",\")
                __json_dump(x, out)
            out.append(\"]\")
        case JsonValue.Object(fields):
            out.append(\"{\")
            auto first = True
            list[str] ks = json._obj_keys(fields)
            for key in ks:
                if first:
                    first = False
                else:
                    out.append(\",\")
                out.append(json._escape(key))
                out.append(\":\")
                __json_dump(fields[key], out)
            out.append(\"}\")

def __json_dumps(JsonValue v) -> str:
    list[str] parts = []
    __json_dump(v, parts)
    return json._join(parts)

def __json_as_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def __json_as_float(JsonValue v) -> float64:
    match v:
        case JsonValue.Float(f):
            return f
        case JsonValue.Int(i):
            return 0.0
        case _:
            return 0.0

def __json_as_bool(JsonValue v) -> bool:
    match v:
        case JsonValue.Bool(b):
            return b
        case _:
            return False

def __json_as_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(s):
            return s
        case _:
            return \"\"

def __json_field(JsonValue v, str k) -> JsonValue:
    match v:
        case JsonValue.Object(m):
            return m[k]
        case _:
            return JsonValue.Null

def __yaml_obj_empty(dict[str, JsonValue] m) -> bool:
    for k in json._obj_keys(m):
        return False
    return True

def __yaml_arr_empty(list[JsonValue] xs) -> bool:
    for x in xs:
        return False
    return True

def __yaml_inline_p(JsonValue v) -> bool:
    match v:
        case JsonValue.Object(m):
            return __yaml_obj_empty(m)
        case JsonValue.Array(xs):
            return __yaml_arr_empty(xs)
        case _:
            return True

def __yaml_inline(JsonValue v) -> str:
    match v:
        case JsonValue.Null:
            return \"null\"
        case JsonValue.Bool(b):
            if b:
                return \"true\"
            return \"false\"
        case JsonValue.Int(i):
            return json._i64_str(i)
        case JsonValue.Float(f):
            return json._f64_str(f)
        case JsonValue.Str(s):
            return json._escape(s)
        case JsonValue.Object(m):
            return \"{}\"
        case JsonValue.Array(xs):
            return \"[]\"

def __yaml_dump_val(JsonValue v, str pad, list[str] out):
    match v:
        case JsonValue.Object(m):
            for key in json._obj_keys(m):
                auto cv = m[key]
                out.append(pad)
                out.append(key)
                if __yaml_inline_p(cv):
                    out.append(\": \")
                    out.append(__yaml_inline(cv))
                    out.append(\"\\n\")
                else:
                    out.append(\":\\n\")
                    __yaml_dump_val(cv, pad + \"  \", out)
        case JsonValue.Array(xs):
            for x in xs:
                out.append(pad)
                if __yaml_inline_p(x):
                    out.append(\"- \")
                    out.append(__yaml_inline(x))
                    out.append(\"\\n\")
                else:
                    out.append(\"-\\n\")
                    __yaml_dump_val(x, pad + \"  \", out)
        case _:
            out.append(pad)
            out.append(__yaml_inline(v))
            out.append(\"\\n\")

def __yaml_dumps(JsonValue v) -> str:
    list[str] parts = []
    if __yaml_inline_p(v):
        parts.append(__yaml_inline(v))
        parts.append(\"\\n\")
    else:
        __yaml_dump_val(v, \"\", parts)
    return json._join(parts)

def __yaml_loads(str s) -> JsonValue:
    return __json_loads(yaml._to_json(s))

enum JsonToken:
    End
    Null
    Bool(bool b)
    Int(int64 i)
    Float(float64 f)
    Str(str s)
    BeginObject
    EndObject
    BeginArray
    EndArray
    Colon
    Comma

struct JsonScanner:
    bytes cur
    def __iter__(self) -> JsonScanner:
        return self
    def __next__(self) -> (JsonToken, bool):
        auto k = json._lex_next(self.cur)
        if k == 1:
            return (JsonToken.Null, True)
        if k == 2:
            return (JsonToken.Bool(True), True)
        if k == 3:
            return (JsonToken.Bool(False), True)
        if k == 4:
            return (JsonToken.Int(json._lex_int(self.cur)), True)
        if k == 5:
            return (JsonToken.Float(json._lex_float(self.cur)), True)
        if k == 6:
            return (JsonToken.Str(json._lex_str(self.cur)), True)
        if k == 7:
            return (JsonToken.BeginObject, True)
        if k == 8:
            return (JsonToken.EndObject, True)
        if k == 9:
            return (JsonToken.BeginArray, True)
        if k == 10:
            return (JsonToken.EndArray, True)
        if k == 11:
            return (JsonToken.Colon, True)
        if k == 12:
            return (JsonToken.Comma, True)
        return (JsonToken.End, False)

# concurrent (I#184, D-0078) — Experimental lock-free collections built on the
# prelude Atomic[T] (D-0066). Reclamation leans entirely on the tracing moving
# Immix collector (D-0008/I#120): a node is an ordinary GC heap struct and every
# stored pointer lives in a genuine reference slot (an Atomic cell or a struct
# field), never packed into an integer word, so the existing GC machinery traces
# and (under the moving collector) forwards every link with NO new runtime code.
# These types ship Experimental (STABILITY.md 4.4) and are namespaced under
# `import concurrent`; concurrency lives in the type name, not in new syntax.

# A lock-free counter: a thin, discoverable wrapper over Atomic[int64]. Every op
# is an indivisible seq_cst atomic, so concurrent increments never lose an update
# the way a non-atomic load/add/store would. Correct on every platform, aarch64
# included (no pointer payload). Build one with concurrent.Counter().
struct Counter:
    Atomic[int64] _cell

    # add(n): atomically add n and return the NEW value (Go atomic.Int64 add).
    def add(self, int64 n) -> int64:
        return self._cell.add(n)

    # incr(): add 1 and return the new value.
    def incr(self) -> int64:
        return self._cell.add(1)

    # get(): the current value (a seq_cst atomic load).
    def get(self) -> int64:
        return self._cell.load()

# The backing constructor for `concurrent.Counter(start)` (its lowering emits a
# call to this; see crates/ir/src/lower/modules/concurrent.rs). Kept internal
# (`__`-prefixed) so the public surface is `concurrent.Counter()`.
def __concurrent_counter(int64 start) -> Counter:
    return Counter(_cell=Atomic(start))

# An internal node of the MPSC Queue. `value` is None only for the sentinel/stub
# node; a live node carries Some(payload). `next` is an atomic nullable link
# (None == no successor yet) — a genuine traced reference slot, so the moving
# collector scans and forwards it with no special runtime code.
struct QNode[T]:
    Optional[T] value
    Atomic[Optional[QNode[T]]] next

# An unbounded multi-producer / single-consumer (MPSC) lock-free FIFO queue
# (Vyukov). ANY number of producers may call push() concurrently; EXACTLY ONE
# consumer calls pop(). A permanent stub node keeps _head/_tail non-null, so a
# drained queue refills correctly. There is intentionally no len()/iteration: a
# lock-free queue has no consistent snapshot, so exposing either would be a
# Python-faithful lie (D-0078). Construct with new_queue[T]().
#
# The queue allocates a GC-managed node per element whatever the payload type, so
# node liveness rides the collector's root scan. The multicore hang + queue-node
# use-after-free seen at high core count under gc-boehm (I#710, a #223 follow-up)
# was root-caused NOT to this algorithm but to the #172 fairness monitor sending
# its preempt signal to a worker during a bdwgc stop-the-world; it is fixed by
# gating that signal on the STW flag (D-0128). The defect reproduced on x86_64 as
# well as aarch64, so it was never arch- or queue-specific. Box confirmation of
# the fix at scale on the Graviton host (#650) is the remaining acceptance step
# before aarch64-Linux leaves experimental; the node-free concurrent.Counter was
# never affected.
struct Queue[T]:
    Atomic[QNode[T]] _head   # consumer end: the stub / last-consumed node
    Atomic[QNode[T]] _tail   # producer end: the most recently pushed node

    # push(v): enqueue v. Safe from any number of producer coroutines/threads.
    # The atomic swap on _tail is the single serialization point; the follow-up
    # store links the previous tail to the new node, publishing it to the
    # consumer. seq_cst throughout (Atomic[T], D-0066).
    def push(self, T v):
        Optional[QNode[T]] empty = None
        QNode[T] n = QNode(value=Optional.Some(v), next=Atomic(empty))
        auto prev = self._tail.swap(n)
        prev.next.store(Optional.Some(n))

    # pop() -> (Optional[T], bool): dequeue the oldest element. Call from the
    # SINGLE consumer only. Returns (Some(v), True) on success and (None, False)
    # when the queue is observably empty — which also covers the transient window
    # where a producer has swapped the tail but not yet linked its node, so the
    # consumer simply retries. comma-ok, like a channel receive: `v, ok = q.pop()`.
    def pop(self) -> (Optional[T], bool):
        auto head = self._head.load()
        auto nxt = head.next.load()
        if nxt.is_none():
            Optional[T] none = None
            return (none, False)
        auto nn = nxt.unwrap()
        self._head.store(nn)
        return (nn.value, True)

# Build an empty MPSC Queue[T]. A free constructor (rather than a bare, field-
# defaulted `Queue()`) because the stub node and its nullable atomic links must
# be initialized from typed None values.
def new_queue[T]() -> Queue[T]:
    Optional[QNode[T]] elink = None
    Optional[T] eval = None
    QNode[T] stub = QNode(value=eval, next=Atomic(elink))
    return Queue(_head=Atomic(stub), _tail=Atomic(stub))

# ---------------------------------------------------------------------------
# ConcurrentMap[K, V] — lock-free, resizable hash map (#184, D-0147).
#
# A split-ordered hash table (Shalev & Shavit): one lock-free singly linked list
# of all entries, kept sorted by the BIT-REVERSED hash (split order), plus a
# bucket directory (an `AtomicArray` of sentinel-node pointers, #844) giving
# O(1) entry into the list. Resize is lazy and lock-free: a bucket's sentinel is
# created on first use by splitting its parent bucket; growing `size` adds new
# sentinels without ever moving an entry — the entire point of split order.
#
# Concurrency is entirely via seq_cst atomics (`Atomic[T]`/`AtomicArray[T]`,
# D-0066/D-0146): the only mutable field on a node is its `next` link (a
# single-word `Atomic` reference), CAS-updated. Values are IMMUTABLE per node —
# `set` and `delete` publish a new node at the front of the key's split-order
# group (an update/tombstone shadows the older node), so no multi-word atomic is
# needed and `V` is unconstrained. Reclamation leans on the tracing moving GC
# (D-0078 §4.3): an unlinked/shadowed node is collected once unreachable; CAS is
# pointer-identity and ABA is moot (a referenced node is never reclaimed).
#
# v1 (Experimental, D-0076) scope / limits, documented:
#   * K: Hashable (dict-key capability: __hash__ + __eq__). V: any type.
#   * Fixed maximum bucket count (`_CMAP_MAXB`); `size` doubles lazily up to it.
#   * Shadowed (updated) and tombstoned (deleted) nodes are not physically
#     reclaimed until the whole map is unreachable — an update-/delete-heavy
#     workload grows the backing list. Physical unlink is a follow-up.
#   * `len()` is an approximate live-entry count (racy by construction).
#   * No consistent snapshot: iteration is intentionally not offered (a lock-free
#     map has none), matching the Queue's honest surface (D-0078).
# aarch64-Linux inherits the node-liveness caveat (#223/#650) like every other
# node-allocating concurrent structure.

# Low 32 bits of `x`, bit-reversed — the core of the split-order key.
def _cmap_reverse32(int64 x) -> int64:
    int64 r = 0
    int64 v = x & 4294967295
    int64 i = 0
    while i < 32:
        r = (r << 1) | (v & 1)
        v = v >> 1
        i = i + 1
    return r

# Regular (data) split-order key: odd, so a data node sorts AFTER the sentinel of
# its bucket. Setting bit 31 before reversing makes the reversed bit 0 a 1.
def _cmap_regular(int64 h) -> int64:
    return _cmap_reverse32((h & 4294967295) | 2147483648)

# Dummy (sentinel) split-order key for bucket `b`: even.
def _cmap_dummy(int64 b) -> int64:
    return _cmap_reverse32(b)

# A node in the split-order list. A sentinel marks a bucket's entry point
# (`sentinel=True`, `key`/`val` = None); a data node carries `key=Some(k)` and
# an immutable `val` (`Some(v)` live, `None` tombstone). `next` is the only
# mutable field — a single-word atomic reference (null == end of list).
struct MNode[K, V]:
    int64 sokey
    Optional[K] key
    Optional[V] val
    Atomic[MNode[K, V]] next
    bool sentinel

struct ConcurrentMap[K: Hashable, V]:
    AtomicArray[MNode[K, V]] buckets
    Atomic[int64] size
    Atomic[int64] count

    # The last node with sokey < `so` and the first with sokey >= `so`, starting
    # from `start`. The list is insert-only (no physical unlink), so no
    # marker-helping is needed here.
    def _search(self, MNode[K, V] start, int64 so) -> (MNode[K, V], MNode[K, V]):
        auto prev = start
        auto cur = start.next.load()
        while cur is not None and cur.sokey < so:
            prev = cur
            cur = cur.next.load()
        return (prev, cur)

    # Parent of bucket `b` (b with its most-significant set bit cleared); only
    # called for b > 0 (bucket 0's sentinel is created eagerly at construction).
    def _parent(self, int64 b) -> int64:
        int64 bit = 1
        while (bit << 1) <= b:
            bit = bit << 1
        return b - bit

    # The sentinel node for bucket `b`, created lazily on first use.
    def _get_bucket(self, int64 b) -> MNode[K, V]:
        auto s = self.buckets.load(b)
        if s is None:
            return self._init_bucket(b)
        return s

    def _init_bucket(self, int64 b) -> MNode[K, V]:
        int64 parent = self._parent(b)
        auto psent = self._get_bucket(parent)
        int64 so = _cmap_dummy(b)
        Optional[K] nk = None
        Optional[V] nv = None
        MNode[K, V] nil = None
        while True:
            auto prev, cur = self._search(psent, so)
            if cur is not None and cur.sokey == so:
                self.buckets.store(b, cur)
                return cur
            MNode[K, V] sent = MNode(sokey=so, key=nk, val=nv, next=Atomic[MNode[K, V]](cur), sentinel=True)
            if prev.next.compare_and_swap(cur, sent):
                self.buckets.store(b, sent)
                return sent
        return nil

    def _maybe_grow(self, int64 cnt, int64 sz):
        if sz >= 65536:
            return
        if cnt > sz * 4:
            self.size.compare_and_swap(sz, sz + sz)

    # Insert or update `key` -> `v`. A new node is published at the front of the
    # key's split-order group, shadowing any older node for the same key.
    def put(self, K key, V v):
        int64 h = key.__hash__()
        int64 so = _cmap_regular(h)
        Optional[K] sk = Optional.Some(key)
        Optional[V] sv = Optional.Some(v)
        while True:
            int64 sz = self.size.load()
            int64 b = h & (sz - 1)
            auto sent = self._get_bucket(b)
            auto prev, cur = self._search(sent, so)
            bool existed_live = False
            auto scan = cur
            while scan is not None and scan.sokey == so:
                if (not scan.sentinel) and scan.key.unwrap() == key:
                    existed_live = scan.val.is_some()
                    break
                scan = scan.next.load()
            MNode[K, V] nn = MNode(sokey=so, key=sk, val=sv, next=Atomic[MNode[K, V]](cur), sentinel=False)
            if prev.next.compare_and_swap(cur, nn):
                if not existed_live:
                    auto c = self.count.add(1)
                    self._maybe_grow(c, sz)
                return

    # get(key) -> (Optional[V], bool), comma-ok like a channel receive. The
    # newest node for a key sorts first, so the first match wins.
    def get(self, K key) -> (Optional[V], bool):
        int64 h = key.__hash__()
        int64 so = _cmap_regular(h)
        int64 sz = self.size.load()
        int64 b = h & (sz - 1)
        auto sent = self._get_bucket(b)
        auto node = sent.next.load()
        while node is not None and node.sokey < so:
            node = node.next.load()
        Optional[V] none = None
        while node is not None and node.sokey == so:
            if (not node.sentinel) and node.key.unwrap() == key:
                if node.val.is_some():
                    return (node.val, True)
                return (none, False)
            node = node.next.load()
        return (none, False)

    def contains(self, K key) -> bool:
        auto r, ok = self.get(key)
        return ok

    def get_or(self, K key, V default) -> V:
        auto r, ok = self.get(key)
        if ok:
            return r.unwrap()
        return default

    # Remove `key`; returns True iff it was present and live. Publishes a
    # tombstone node that shadows the live node.
    def delete(self, K key) -> bool:
        int64 h = key.__hash__()
        int64 so = _cmap_regular(h)
        Optional[K] sk = Optional.Some(key)
        Optional[V] nv = None
        while True:
            int64 sz = self.size.load()
            int64 b = h & (sz - 1)
            auto sent = self._get_bucket(b)
            auto prev, cur = self._search(sent, so)
            bool live = False
            auto scan = cur
            while scan is not None and scan.sokey == so:
                if (not scan.sentinel) and scan.key.unwrap() == key:
                    live = scan.val.is_some()
                    break
                scan = scan.next.load()
            if not live:
                return False
            MNode[K, V] tomb = MNode(sokey=so, key=sk, val=nv, next=Atomic[MNode[K, V]](cur), sentinel=False)
            if prev.next.compare_and_swap(cur, tomb):
                self.count.sub(1)
                return True

    # Approximate number of live entries (racy by construction).
    def length(self) -> int64:
        return self.count.load()

# Build an empty ConcurrentMap[K, V]. Bucket 0's sentinel (the list head) is
# created eagerly so lazy bucket init always has a parent to split.
def new_concurrent_map[K: Hashable, V]() -> ConcurrentMap[K, V]:
    AtomicArray[MNode[K, V]] bk = AtomicArray[MNode[K, V]](65536)
    Optional[K] nk = None
    Optional[V] nv = None
    MNode[K, V] nil = None
    MNode[K, V] s0 = MNode(sokey=0, key=nk, val=nv, next=Atomic[MNode[K, V]](nil), sentinel=True)
    bk.store(0, s0)
    return ConcurrentMap(buckets=bk, size=Atomic[int64](2), count=Atomic[int64](0))

# ---------------------------------------------------------------------------
# ConcurrentSet[K] — lock-free, resizable concurrent set (#184, D-0148). A thin
# membership wrapper over ConcurrentMap[K, bool]: the stored value is unused
# (membership is the key's presence), so every lock-free / resizable / GC-safety
# property of the map (D-0147) carries over unchanged. @shareable (D-0113).
struct ConcurrentSet[K: Hashable]:
    ConcurrentMap[K, bool] _m

    # Add `key` to the set (idempotent).
    def add(self, K key):
        self._m.put(key, True)

    def contains(self, K key) -> bool:
        return self._m.contains(key)

    # Remove `key`; returns True iff it was present.
    def remove(self, K key) -> bool:
        return self._m.delete(key)

    # Approximate number of members (racy by construction, like the map).
    def length(self) -> int64:
        return self._m.length()

def new_concurrent_set[K: Hashable]() -> ConcurrentSet[K]:
    ConcurrentMap[K, bool] m = new_concurrent_map()
    return ConcurrentSet(_m=m)

# ---------------------------------------------------------------------------
# ConcurrentStack[T] — a lock-free Treiber stack (#184, D-0148). One atomic head
# pointer; push and pop are each a single-CAS retry loop. Reclamation leans on
# the tracing moving GC (D-0078 §4.3): a popped node is collected once
# unreachable, CAS is pointer-identity, and ABA is moot (a node any party still
# references is never reclaimed, so its address is never reused under a stale
# CAS). `pop` is comma-ok, like the Queue. @shareable (D-0113).
struct CStackNode[T]:
    Optional[T] value
    Atomic[CStackNode[T]] next

struct ConcurrentStack[T]:
    Atomic[CStackNode[T]] _head

    # push(v): LIFO insert. Safe from any number of coroutines/threads.
    def push(self, T v):
        while True:
            auto head = self._head.load()
            CStackNode[T] n = CStackNode(value=Optional.Some(v), next=Atomic[CStackNode[T]](head))
            if self._head.compare_and_swap(head, n):
                return

    # pop() -> (Optional[T], bool): remove and return the top, comma-ok. Returns
    # (None, False) when the stack is observably empty.
    def pop(self) -> (Optional[T], bool):
        while True:
            auto head = self._head.load()
            if head is None:
                Optional[T] none = None
                return (none, False)
            auto nxt = head.next.load()
            if self._head.compare_and_swap(head, nxt):
                return (head.value, True)

def new_concurrent_stack[T]() -> ConcurrentStack[T]:
    CStackNode[T] nil = None
    return ConcurrentStack(_head=Atomic[CStackNode[T]](nil))

# xml (I#351, D-0085) — Experimental core XML DOM for the AWS rest-xml / query
# protocols. `XmlElement`/`XmlAttr` are compiler-known reference structs (like
# `JsonValue`); the tree build + serialize walk live in these prelude functions,
# which drive the `xml._lex_*` C tokenizer and the `_esc_*`/`_join` C helpers, so
# the runtime never depends on the DOM layout (design/0033-xml.md).
struct XmlAttr:
    str name
    str value

struct XmlElement:
    str name
    str namespace
    str text
    list[XmlAttr] attrs
    list[XmlElement] children

    def find(self, str name) -> Optional[XmlElement]:
        for c in self.children:
            if c.name == name:
                return Optional.Some(c)
        return Optional.None

    def find_all(self, str name) -> list[XmlElement]:
        list[XmlElement] out = []
        for c in self.children:
            if c.name == name:
                out.append(c)
        return out

    def get_attr(self, str name) -> Optional[str]:
        for a in self.attrs:
            if a.name == name:
                return Optional.Some(a.value)
        return Optional.None

    def get_text(self) -> str:
        return self.text

    def local_name(self) -> str:
        auto i = self.name.find(\":\")
        if i < 0:
            return self.name
        return self.name[i + 1:]

    def set_text(self, str t) -> XmlElement:
        self.text = t
        return self

    def set_attr(self, str name, str value) -> XmlElement:
        for a in self.attrs:
            if a.name == name:
                a.value = value
                return self
        self.attrs.append(XmlAttr(name=name, value=value))
        return self

    def add_child(self, XmlElement c) -> XmlElement:
        self.children.append(c)
        return self

# xml.element(name): a fresh empty element — the builder entry point.
def __xml_element(str name) -> XmlElement:
    list[XmlAttr] a = []
    list[XmlElement] c = []
    return XmlElement(name=name, namespace=\"\", text=\"\", attrs=a, children=c)

# Resolve an element's namespace URI from its (possibly prefixed) name and the
# in-scope default + prefix bindings.
def __xml_resolve_ns(str nm, str cdef, dict[str, str] cpre) -> str:
    auto i = nm.find(\":\")
    if i < 0:
        return cdef
    auto p = nm[0:i]
    return cpre.get(p).unwrap_or(\"\")

# Build the element whose OPEN token the cursor currently sits on, recursing into
# children. `pdef`/`ppre` are the inherited default namespace and prefix map.
def __xml_build(bytes c, str pdef, dict[str, str] ppre) -> XmlElement:
    str nm = xml._lex_name(c)
    list[XmlAttr] at = []
    auto k = xml._lex_next(c)
    while k == 2:
        at.append(XmlAttr(name=xml._lex_name(c), value=xml._lex_value(c)))
        k = xml._lex_next(c)
    str cdef = pdef
    dict[str, str] cpre = ppre.copy()
    for a in at:
        if a.name == \"xmlns\":
            cdef = a.value
        elif a.name.startswith(\"xmlns:\"):
            cpre[a.name[6:]] = a.value
    str ns = __xml_resolve_ns(nm, cdef, cpre)
    list[XmlElement] ch = []
    list[str] tx = []
    if k == 3:
        k = xml._lex_next(c)
        while k != 6 and k != 0:
            if k == 5:
                tx.append(xml._lex_value(c))
            elif k == 1:
                ch.append(__xml_build(c, cdef, cpre))
            k = xml._lex_next(c)
    return XmlElement(name=nm, namespace=ns, text=xml._join(tx), attrs=at, children=ch)

# xml.parse(s): parse a document and return its root element.
def __xml_parse(str s) -> XmlElement:
    auto c = xml._lex_new(s)
    dict[str, str] pre = {}
    auto k = xml._lex_next(c)
    while k == 5:
        k = xml._lex_next(c)
    if k == 1:
        return __xml_build(c, \"\", pre)
    return __xml_element(\"\")

# Serialize one element (and its subtree) into the fragment list.
def __xml_dump(XmlElement e, list[str] out):
    out.append(\"<\")
    out.append(e.name)
    for a in e.attrs:
        out.append(\" \")
        out.append(a.name)
        out.append(\"=\")
        out.append(xml._esc_attr(a.value))
    auto has_text = e.text != \"\"
    if len(e.children) == 0 and not has_text:
        out.append(\"/>\")
        return
    out.append(\">\")
    if has_text:
        out.append(xml._esc_text(e.text))
    for ch in e.children:
        __xml_dump(ch, out)
    out.append(\"</\")
    out.append(e.name)
    out.append(\">\")

# xml.dumps(e): deterministic serialization of an element tree.
def __xml_dumps(XmlElement e) -> str:
    list[str] parts = []
    __xml_dump(e, parts)
    return xml._join(parts)

# arena — scope-bound, non-moving, bulk-freed region allocator (#553, D-0158;
# docs/design/arena.md). A @scoped affine resource (D-0159): constructible only
# as a `with` head, freed deterministically at block exit. Allocation is explicit
# and handle-targeted: `a.alloc(bytes, n)` for a non-moving byte buffer (the FFI
# pin case), `a.new(T, ...)` for a single leaf (pointer-free) object. Arena v1 is
# LEAF-ONLY — arena objects hold no outgoing heap pointers (sema rejects a
# pointerful `T`), and an arena-allocated value may not escape its block.
#
# The method / with bodies below are COMPILER INTRINSICS: lowering recognizes an
# `arena` with-head and `arena`-typed receiver and emits the runtime region calls
# (zap_arena_open / zap_arena_alloc_* / zap_arena_close — crates/ir/src/lower.rs),
# never these Zap bodies. They exist only so the type checks and so `arena`
# satisfies the @scoped context-manager protocol (ZAP5127).
@scoped
struct arena:
    def __enter__(self) -> arena:
        return self

    def __exit__(self):
        pass
";

/// Parse the built-in [`PRELUDE_SRC`] into AST items. The prelude is authored to
/// lex and parse cleanly; a failure here is a compiler bug. Callers (sema and
/// the lowerer) prepend these ahead of the user's items, skipping any whose name
/// the user has already defined (so a program may still declare its own type of
/// that name without a duplicate-definition error).
pub fn prelude_items() -> Vec<crate::ast::Item> {
    let (module, perrs, lerrs) = parse_str(PRELUDE_SRC);
    debug_assert!(
        perrs.is_empty() && lerrs.is_empty(),
        "built-in prelude must parse cleanly: {perrs:?} {lerrs:?}"
    );
    module.items
}

/// The declared type name of a top-level item, if it introduces one.
fn item_type_name(item: &crate::ast::Item) -> Option<&str> {
    use crate::ast::Item;
    match item {
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::Interface(i) => Some(&i.name),
        Item::Annotation(a) => Some(&a.name),
        _ => None,
    }
}

/// Does `module` reference any of the signals surface (ticket #77)? Used to
/// gate prelude injection so programs that never touch signals are byte-for-byte
/// unchanged (no extra `Signal`/`MemStats` type in the lowered module). The scan
/// is over the derived `Debug` of the AST with QUOTED needles, so each needle
/// matches a whole string-valued field (an identifier, type name, decorator name
/// or scope-option key) — `"gc"` matches `Name("gc")` but not `Name("gc_total")`.
/// A rare false positive (e.g. a string literal `"gc"`) only injects two unused
/// declarations, which emit no code; it never changes observable behaviour.
fn references_prelude(module: &Module) -> bool {
    const NEEDLES: &[&str] = &[
        "\"Signal\"",
        "\"MemStats\"",
        "\"signal\"",
        "\"gc\"",
        "\"runtime\"",
        "\"on_signal\"",
        "\"cancel_on\"",
        "\"grace\"",
        "\"DateTime\"",
        "\"time\"",
        "\"datetime\"",
        // The validated asymmetric-key metadata `crypto.key_info()` returns
        // (#643, D-0126): the `KeyInfo` value struct ships in the prelude,
        // injected for any program that names the method or the struct.
        "\"KeyInfo\"",
        "\"key_info\"",
        "\"JsonValue\"",
        "\"json\"",
        "\"Json\"",
        "\"JsonToken\"",
        "\"JsonScanner\"",
        "\"yaml\"",
        "\"Yaml\"",
        // The generic `Optional[T]` ships in the prelude (I#154, D-0064/D-0065);
        // inject it for any program that names it.
        "\"Optional\"",
        // The generic `Atomic[T]` sync primitive ships in the prelude (I#176).
        "\"Atomic\"",
        // The `threading` OS-thread compat surface (I#175): `Thread` and the
        // other primitive wrappers ship in the prelude, injected for any program
        // that names the module or a wrapper type.
        "\"threading\"",
        "\"Thread\"",
        "\"Lock\"",
        "\"RLock\"",
        "\"Event\"",
        "\"Semaphore\"",
        "\"Condition\"",
        // The `concurrent` module (I#184, D-0078): the lock-free `Counter` and
        // the MPSC `Queue[T]` (+ its `QNode[T]` and `new_queue` constructor)
        // ship in the prelude, injected for any program that names the module or
        // one of these types.
        "\"concurrent\"",
        "\"Counter\"",
        "\"Queue\"",
        "\"QNode\"",
        "\"new_queue\"",
        // The lock-free, resizable `ConcurrentMap[K, V]` (#184, D-0147): its
        // `MNode[K, V]` node and `new_concurrent_map` constructor ship in the
        // prelude, injected for any program that names one of them.
        "\"ConcurrentMap\"",
        "\"MNode\"",
        "\"new_concurrent_map\"",
        // ConcurrentSet[K] + the Treiber ConcurrentStack[T] (#184, D-0148),
        // injected for any program that names one of them.
        "\"ConcurrentSet\"",
        "\"new_concurrent_set\"",
        "\"ConcurrentStack\"",
        "\"CStackNode\"",
        "\"new_concurrent_stack\"",
        // The `xml` core DOM (I#351, D-0085): `XmlElement`/`XmlAttr` and the
        // `__xml_*` build/serialize helpers ship in the prelude, injected for any
        // program that names the module or one of these types.
        "\"xml\"",
        "\"XmlElement\"",
        "\"XmlAttr\"",
        // The scope-bound `arena` region allocator (#553, D-0158): the `@scoped`
        // `arena` struct ships in the prelude, injected for any program that names
        // it (`with arena(...) as a:`).
        "\"arena\"",
    ];
    let dbg = format!("{module:?}");
    NEEDLES.iter().any(|n| dbg.contains(n))
}

/// Does `module` contain a `.get(...)` attribute access? The built-in
/// `dict.get(key)` returns the prelude `Optional[V]` (I#154/#281) without the
/// program ever spelling `Optional`, so an inline `d.get(k).unwrap_or(...)`
/// still needs the `Optional[T]` enum injected even though [`references_prelude`]
/// — which keys off the spelled type name — does not fire (#497). The scan is
/// over the derived `Debug` of the AST for the `name: "get"` field of an
/// `Expr::Attr`, mirroring the quoted-needle style of [`references_prelude`]. A
/// false positive (a `.get` on some other receiver) only injects an *unused*
/// generic `Optional[T]`, which monomorphization drops (it emits no code), so it
/// never changes observable behaviour or output.
fn references_optional_get(module: &Module) -> bool {
    format!("{module:?}").contains("name: \"get\"")
}

/// Does this prelude item mention any of the given (user-shadowed) prelude type
/// names? Used by [`with_prelude`] to drop a prelude helper function whose body
/// or signature names a prelude type the user has redefined (#689): the prelude's
/// own definition of that type is dropped in favor of the user's, so a helper
/// like `__concurrent_counter` (which builds `Counter(_cell=…)`) would otherwise
/// bind to the user's unrelated `Counter` and fail at lowering. The scan is over
/// the item's derived `Debug` with quoted needles, matching the style of
/// [`references_prelude`]; an empty `names` set never matches, so a program with
/// no such collision keeps the full prelude unchanged.
fn item_mentions_any(item: &crate::ast::Item, names: &std::collections::HashSet<String>) -> bool {
    if names.is_empty() {
        return false;
    }
    let dbg = format!("{item:?}");
    names.iter().any(|n| dbg.contains(&format!("\"{n}\"")))
}

/// Return `module` augmented with the built-in [`prelude_items`] when it uses the
/// signals surface (see [`references_prelude`]); otherwise return a plain clone.
/// A prelude item is skipped if the user already declares a type of that name, so
/// user code can still shadow `Signal`/`MemStats` without a duplicate-definition
/// error. Both the type checker and the lowerer apply this, so they agree on the
/// surface a program sees (ticket #77, D-0058).
///
/// When the heavier prelude surface is not referenced but the program uses a
/// `.get(...)` call (see [`references_optional_get`]), only the generic
/// `Optional[T]` enum is injected — just enough for an inline
/// `d.get(k).unwrap_or(...)` to resolve and monomorphize (#497) — keeping every
/// other program byte-for-byte unchanged.
pub fn with_prelude(module: &Module) -> Module {
    let full = references_prelude(module);
    // `dict.get(key) -> Optional[V]` produces an `Optional` without the program
    // ever naming it. `references_optional_get` catches a parsed `.get(...)`
    // call (#497/#555); a `.get` that occurs only inside an f-string field is
    // hidden in the literal's text (re-parsed only at lowering), so also inject
    // for any program that uses a dict (#498). Either way only the generic
    // `Optional[T]` enum is added, and an unused injection emits no code.
    let optional_only = !full && (references_optional_get(module) || uses_dict(module));
    if !full && !optional_only {
        return module.clone();
    }
    let user_names: std::collections::HashSet<&str> =
        module.items.iter().filter_map(item_type_name).collect();
    let prelude = prelude_items();
    // Prelude *type* names the user has redefined (#689). A user type of the same
    // name shadows the prelude's (handled by the `Some(n)` arm below), but a
    // prelude helper *function* that references the shadowed type — e.g.
    // `__concurrent_counter`, whose body builds `Counter(_cell=Atomic(start))` —
    // has no type name of its own, so it was injected unconditionally and then
    // bound its `._cell` field access to the user's unrelated `Counter`, failing
    // at lowering with `no field '_cell' on struct`. Collect the shadowed names
    // so those dependent helpers are dropped alongside the type.
    let shadowed: std::collections::HashSet<String> = prelude
        .iter()
        .filter_map(item_type_name)
        .filter(|n| user_names.contains(n))
        .map(|n| n.to_string())
        .collect();
    let mut items: Vec<crate::ast::Item> = prelude
        .into_iter()
        .filter(|it| {
            // In the `Optional`-only path, inject nothing but the `Optional[T]`
            // enum (it is self-contained — its methods reference only itself).
            if optional_only {
                return item_type_name(it) == Some("Optional") && !user_names.contains("Optional");
            }
            match item_type_name(it) {
                Some(n) => !user_names.contains(n),
                // A prelude helper with no type name of its own (a free
                // function) is kept unless it mentions a user-shadowed prelude
                // type (#689); `shadowed` empty → every program without such a
                // collision is byte-for-byte unchanged.
                None => !item_mentions_any(it, &shadowed),
            }
        })
        .collect();
    items.extend(module.items.iter().cloned());
    Module {
        items,
        span: module.span,
    }
}

/// Does `module` use a `dict` in any form (typed binding/param/field, literal,
/// or comprehension)? Used to make `Optional[T]` available for `dict.get`'s
/// implicit `Optional[V]` result (#498). Like [`references_prelude`], this is a
/// coarse scan over the AST's `Debug`; a false positive only injects the unused
/// generic, which emits no code.
fn uses_dict(module: &Module) -> bool {
    let dbg = format!("{module:?}");
    dbg.contains("\"dict\"") || dbg.contains("Dict(") || dbg.contains("DictComp")
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
    /// The original source, kept so a node can echo its exact spelling by span
    /// (power-assert, D-0054). Empty when the source was not supplied.
    src: String,
}

impl Parser {
    fn new(mut toks: Vec<Token>, src: &str) -> Parser {
        if toks.last().map(|t| &t.kind) != Some(&TokenKind::Eof) {
            let end = toks.last().map(|t| t.span.end).unwrap_or(0);
            toks.push(Token::new(TokenKind::Eof, Span::empty_at(end)));
        }
        Parser {
            toks,
            pos: 0,
            errors: Vec::new(),
            src: src.to_string(),
        }
    }

    /// The exact source text covered by `span`, or `""` when the source was not
    /// provided or the span is out of range.
    fn source_slice(&self, span: Span) -> &str {
        let (lo, hi) = (span.start as usize, span.end as usize);
        if lo <= hi
            && hi <= self.src.len()
            && self.src.is_char_boundary(lo)
            && self.src.is_char_boundary(hi)
        {
            &self.src[lo..hi]
        } else {
            ""
        }
    }

    // --- cursor ---

    fn peek(&self) -> &TokenKind {
        &self.toks[self.pos].kind
    }

    fn peek2(&self) -> &TokenKind {
        self.toks
            .get(self.pos + 1)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn peek_span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn prev_end(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].span.end
        }
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        t
    }

    fn is(&self, k: &TokenKind) -> bool {
        self.peek() == k
    }

    fn is_kw(&self, kw: Keyword) -> bool {
        matches!(self.peek(), TokenKind::Keyword(k) if *k == kw)
    }

    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.is(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: Keyword) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: &TokenKind, what: &str) {
        if !self.eat(k) {
            self.error_here(ErrorCode::ZAP2001, format!("expected {what}"));
        }
    }

    fn expect_kw(&mut self, kw: Keyword, what: &str) {
        if !self.eat_kw(kw) {
            self.error_here(ErrorCode::ZAP2001, format!("expected {what}"));
        }
    }

    fn error_here(&mut self, code: ErrorCode, message: impl Into<String>) {
        self.errors.push(ParseError {
            code,
            message: message.into(),
            span: self.peek_span(),
        });
    }

    /// Record an error at an explicit span (e.g. the offending value's span,
    /// which may already have been consumed).
    fn error_at(&mut self, span: Span, code: ErrorCode, message: impl Into<String>) {
        self.errors.push(ParseError {
            code,
            message: message.into(),
            span,
        });
    }

    fn span_from(&self, start: u32) -> Span {
        Span::new(start, self.prev_end())
    }

    fn ident_opt(&mut self) -> Option<(String, Span)> {
        if let TokenKind::Ident(name) = self.peek() {
            let name = name.clone();
            let span = self.peek_span();
            self.bump();
            Some((name, span))
        } else {
            None
        }
    }

    fn ident(&mut self, what: &str) -> String {
        match self.ident_opt() {
            Some((name, _)) => name,
            None => {
                self.error_here(ErrorCode::ZAP2001, format!("expected {what}"));
                "<error>".to_string()
            }
        }
    }

    /// A member name in a `.name` position or an `Enum.Variant` pattern path.
    /// Accepts an ordinary identifier, plus the `None` keyword — `None` doubles
    /// as the conventional empty-variant name of `Optional[T]` and similar sum
    /// types (I#154 Fork O), so `opt.None` / `case Optional.None:` must read it as
    /// a member rather than the null literal.
    fn member_name(&mut self, what: &str) -> String {
        if self.is_kw(Keyword::None) {
            self.bump();
            "None".to_string()
        } else {
            self.ident(what)
        }
    }

    fn skip_newlines(&mut self) {
        while self.is(&TokenKind::Newline) {
            self.bump();
        }
    }

    // --- module / items ---

    fn parse_module(&mut self) -> Module {
        let start = self.peek_span().start;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at_eof() {
            let before = self.pos;
            self.parse_top_level(&mut items);
            self.skip_newlines();
            if self.pos == before {
                // No progress: skip a token to avoid an infinite loop.
                self.error_here(ErrorCode::ZAP2002, "unexpected token at top level");
                self.bump();
                self.skip_newlines();
            }
        }
        Module {
            items,
            span: self.span_from(start),
        }
    }

    fn parse_top_level(&mut self, items: &mut Vec<Item>) {
        let decos = self.parse_decorators();
        match self.peek() {
            TokenKind::Keyword(Keyword::Def) => {
                items.push(Item::Function(self.parse_function(decos)))
            }
            TokenKind::Keyword(Keyword::Struct) => {
                items.push(Item::Struct(self.parse_struct(decos, false)))
            }
            // Contextual `value struct` modifier (D-0051): `value` is an
            // identifier, not a keyword, so only treat it as the value-type
            // modifier when it immediately precedes `struct`.
            TokenKind::Ident(n)
                if n == "value" && matches!(self.peek2(), TokenKind::Keyword(Keyword::Struct)) =>
            {
                self.bump(); // consume `value`
                items.push(Item::Struct(self.parse_struct(decos, true)))
            }
            TokenKind::Keyword(Keyword::Enum) => items.push(Item::Enum(self.parse_enum(decos))),
            // Transparent type alias `type Name = <type>` (#606, D-0105). `type`
            // is a SOFT keyword: it is not reserved today, so it is treated as
            // the alias introducer only at item start and only when immediately
            // followed by an identifier (the alias name). `type ident ident` is
            // not a valid statement, so this never shadows an existing use of
            // `type` as a plain identifier (`type = 5`, `type(x)`, `type.foo`).
            TokenKind::Ident(n) if n == "type" && matches!(self.peek2(), TokenKind::Ident(_)) => {
                if !decos.is_empty() {
                    self.error_here(ErrorCode::ZAP2003, "decorators must precede a declaration");
                }
                items.push(Item::TypeAlias(self.parse_type_alias()))
            }
            TokenKind::Keyword(Keyword::Interface) => {
                items.push(Item::Interface(self.parse_interface()))
            }
            TokenKind::Keyword(Keyword::Annotation) => {
                items.push(Item::Annotation(self.parse_annotation()))
            }
            TokenKind::Keyword(Keyword::Decorator) => {
                items.push(Item::Decorator(self.parse_decorator_def()))
            }
            TokenKind::Keyword(Keyword::Import) => {
                // Multi-import and grouped forms (D-0052) desugar to one
                // `Item::Import` per listed module, so each resolves exactly
                // like a standalone `import`.
                items.extend(self.parse_import().into_iter().map(Item::Import))
            }
            TokenKind::Keyword(Keyword::When) => {
                if !decos.is_empty() {
                    self.error_here(ErrorCode::ZAP2003, "decorators must precede a declaration");
                }
                items.push(Item::Cfg(self.parse_cfg_items()))
            }
            _ => {
                if !decos.is_empty() {
                    self.error_here(ErrorCode::ZAP2003, "decorators must precede a declaration");
                }
                let mut stmts = Vec::new();
                self.parse_line(&mut stmts);
                items.extend(stmts.into_iter().map(Item::Stmt));
            }
        }
    }

    fn parse_decorators(&mut self) -> Vec<Decorator> {
        let mut v = Vec::new();
        while self.is(&TokenKind::At) {
            let start = self.peek_span().start;
            self.bump();
            let name = self.decorator_name();
            let args = if self.eat(&TokenKind::LParen) {
                self.parse_args()
            } else {
                Vec::new()
            };
            self.expect(&TokenKind::Newline, "newline after decorator");
            v.push(Decorator {
                name,
                args,
                span: self.span_from(start),
            });
        }
        v
    }

    fn dotted_name(&mut self) -> String {
        let mut name = self.ident("name");
        while self.is(&TokenKind::Dot) {
            self.bump();
            name.push('.');
            name.push_str(&self.ident("name"));
        }
        name
    }

    /// A (possibly dotted) decorator name. Identical to [`Self::dotted_name`]
    /// except the leading segment may be the `range` keyword: the built-in
    /// `@range(...)` field decorator (D-0036) collides with the `range` keyword,
    /// so it would otherwise fail to parse. Only this one keyword is accepted as
    /// a decorator name; every other keyword stays reserved.
    fn decorator_name(&mut self) -> String {
        let mut name = if self.is_kw(Keyword::Range) {
            self.bump();
            "range".to_string()
        } else {
            self.ident("name")
        };
        while self.is(&TokenKind::Dot) {
            self.bump();
            name.push('.');
            name.push_str(&self.ident("name"));
        }
        name
    }

    fn parse_generics(&mut self) -> Vec<TypeParam> {
        let mut v = Vec::new();
        if self.eat(&TokenKind::LBracket) {
            if !self.is(&TokenKind::RBracket) {
                loop {
                    let start = self.peek_span().start;
                    let name = self.ident("type parameter");
                    // Optional interface bound (`[T: Ordered]`, D-0017/I#154 Fork
                    // B). Parsed and carried so the grammar round-trips; bound
                    // enforcement is deferred to a later slice.
                    let bound = if self.eat(&TokenKind::Colon) {
                        Some(self.parse_type())
                    } else {
                        None
                    };
                    v.push(TypeParam {
                        name,
                        bound,
                        span: self.span_from(start),
                    });
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RBracket, "']'");
        }
        v
    }

    fn parse_function(&mut self, decorators: Vec<Decorator>) -> Function {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Def, "'def'");
        let name = self.ident("function name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::LParen, "'('");
        let params = self.parse_params();
        self.expect(&TokenKind::RParen, "')'");
        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_block();
        Function {
            decorators,
            name,
            generics,
            params,
            ret,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if self.is(&TokenKind::RParen) {
            return params;
        }
        // A bare `*` separator makes every following parameter keyword-only
        // (I#496, Python semantics). Track whether we have passed it and how
        // many parameters followed, so a trailing `*` with nothing after it is
        // reported rather than silently accepted.
        let mut keyword_only = false;
        let mut star_span: Option<Span> = None;
        let mut params_after_star = 0usize;
        // A bare `/` separator makes every *preceding* parameter positional-only
        // (#602, mirror of the `*` rule). It may appear at most once and must come
        // before any `*`.
        let mut positional_only_seen = false;
        loop {
            if self.is(&TokenKind::Slash) {
                let span = self.peek_span();
                self.bump();
                if positional_only_seen {
                    self.error_at(
                        span,
                        ErrorCode::ZAP2019,
                        "a `/` positional-only separator may appear at most once in a parameter list",
                    );
                } else if keyword_only {
                    self.error_at(
                        span,
                        ErrorCode::ZAP2019,
                        "a `/` positional-only separator must appear before the `*` keyword-only separator",
                    );
                } else if params.is_empty() {
                    self.error_at(
                        span,
                        ErrorCode::ZAP2019,
                        "a `/` positional-only separator must follow at least one parameter",
                    );
                } else {
                    for p in &mut params {
                        p.positional_only = true;
                    }
                }
                positional_only_seen = true;
                // The separator may end the list (`/)` or `/,)`) or be followed by
                // `, <param>...`; those following params stay positional-or-keyword.
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.is(&TokenKind::RParen) {
                    break; // trailing comma after `/`
                }
                continue;
            }
            if self.is(&TokenKind::Star) {
                let span = self.peek_span();
                self.bump();
                if keyword_only {
                    self.error_at(
                        span,
                        ErrorCode::ZAP2018,
                        "a `*` keyword-only separator may appear at most once in a parameter list",
                    );
                }
                keyword_only = true;
                star_span = Some(span);
                // The separator must be followed by `, <param>...`; anything
                // else (`*)` or `*,)`) leaves nothing to make keyword-only.
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.is(&TokenKind::RParen) {
                    break; // trailing comma after `*`
                }
                continue;
            }
            let mut p = self.parse_param();
            p.keyword_only = keyword_only;
            if keyword_only {
                params_after_star += 1;
            }
            params.push(p);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.is(&TokenKind::RParen) {
                break; // trailing comma
            }
        }
        if let Some(span) = star_span {
            if params_after_star == 0 {
                self.error_at(
                    span,
                    ErrorCode::ZAP2018,
                    "a `*` keyword-only separator must be followed by at least one parameter",
                );
            }
        }
        params
    }

    fn parse_param(&mut self) -> Param {
        let start = self.peek_span().start;
        // Inline parameter decorators/annotations (I#458, D-0089): `@Inject` in
        // `def f(@Inject Svc dep)`. Unlike item-level decorators these sit inline
        // with the type/name, so no trailing newline is consumed. A *declared*
        // annotation/decorator here is reflectable metadata; anything else is
        // rejected downstream (sema ZAP5032 / IR deferral).
        let decorators = self.parse_inline_param_decorators();
        // `self` receiver (untyped).
        if matches!(self.peek(), TokenKind::Ident(n) if n == "self") {
            self.bump();
            return Param {
                decorators,
                ty: None,
                name: "self".to_string(),
                default: None,
                variadic: false,
                kwargs: false,
                keyword_only: false,
                positional_only: false,
                span: self.span_from(start),
            };
        }
        let ty = Some(self.parse_type());
        // A `TYPE... name` variadic (D-0038) collects extra *positional*
        // arguments; a `TYPE **name` keyword-collector (#601) collects extra
        // *keyword* arguments into a `dict[str, TYPE]`. The two markers are
        // mutually exclusive on one parameter.
        let variadic = self.eat(&TokenKind::Ellipsis);
        let kwargs = !variadic && self.eat(&TokenKind::StarStar);
        let name = self.ident("parameter name");
        let default = if self.eat(&TokenKind::Assign) {
            Some(self.parse_expr())
        } else {
            None
        };
        Param {
            decorators,
            ty,
            name,
            default,
            variadic,
            kwargs,
            keyword_only: false,
            positional_only: false,
            span: self.span_from(start),
        }
    }

    /// Parse zero or more inline parameter decorators `@name(args)` (I#458). No
    /// trailing newline is consumed — the decorators immediately precede the
    /// parameter's type.
    fn parse_inline_param_decorators(&mut self) -> Vec<Decorator> {
        let mut v = Vec::new();
        while self.is(&TokenKind::At) {
            let start = self.peek_span().start;
            self.bump();
            let name = self.decorator_name();
            let args = if self.eat(&TokenKind::LParen) {
                self.parse_args()
            } else {
                Vec::new()
            };
            v.push(Decorator {
                name,
                args,
                span: self.span_from(start),
            });
        }
        v
    }

    /// Parse a transparent type alias (#606, D-0105):
    /// `type IDENT [ generics ] "=" type`. The leading `type` soft keyword has
    /// already been confirmed (but not consumed) by the caller.
    fn parse_type_alias(&mut self) -> TypeAliasDef {
        let start = self.peek_span().start;
        self.bump(); // consume the `type` soft keyword
        let name = self.ident("type alias name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::Assign, "'=' in type alias");
        let target = self.parse_type();
        // A type alias is a single-line declaration; terminate on the newline.
        if !self.at_eof() {
            self.expect(&TokenKind::Newline, "newline after type alias");
        }
        TypeAliasDef {
            name,
            generics,
            target,
            span: self.span_from(start),
        }
    }

    fn parse_struct(&mut self, decorators: Vec<Decorator>, is_value: bool) -> StructDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Struct, "'struct'");
        let name = self.ident("struct name");
        let generics = self.parse_generics();
        let (fields, methods) = self.parse_struct_body();
        StructDef {
            decorators,
            name,
            generics,
            fields,
            methods,
            is_value,
            span: self.span_from(start),
        }
    }

    fn parse_struct_body(&mut self) -> (Vec<Field>, Vec<Function>) {
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            // A bare `pass` is the explicit spelling of an empty struct body
            // (#591), mirroring the empty-enum case; it declares no field.
            if matches!(self.peek(), TokenKind::Ident(name) if name == "pass")
                && matches!(
                    self.peek2(),
                    TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
                )
            {
                self.bump();
                self.expect(&TokenKind::Newline, "newline after 'pass'");
                continue;
            }
            let decos = self.parse_decorators();
            if self.is_kw(Keyword::Def) {
                methods.push(self.parse_function(decos));
            } else {
                let fstart = self.peek_span().start;
                let ty = self.parse_type();
                let fname = self.ident("field name");
                // C-style bitfield width (I#173, D-0068): `type IDENT : N`. The
                // colon never introduces a block on a field line (fields are
                // single-line), so it is unambiguously a bit-width annotation.
                let bits = if self.eat(&TokenKind::Colon) {
                    self.parse_bit_width()
                } else {
                    None
                };
                let default = if self.eat(&TokenKind::Assign) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                self.expect(&TokenKind::Newline, "newline after field");
                fields.push(Field {
                    decorators: decos,
                    ty,
                    name: fname,
                    bits,
                    default,
                    span: self.span_from(fstart),
                });
            }
            if self.pos == before {
                self.error_here(ErrorCode::ZAP2004, "unexpected token in struct body");
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        (fields, methods)
    }

    /// Parse the bit width after a field's `:` (C-style bitfield, I#173). The
    /// width is a plain decimal integer literal; the fit-to-type and 1..=64
    /// range checks are sema's (D-0068), so this only rejects a non-decimal or
    /// out-of-`u32` literal.
    fn parse_bit_width(&mut self) -> Option<u32> {
        if let TokenKind::Int(s) = self.peek().clone() {
            self.bump();
            match s.replace('_', "").parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => {
                    self.error_here(
                        ErrorCode::ZAP2005,
                        "bitfield width must be a decimal integer",
                    );
                    None
                }
            }
        } else {
            self.error_here(
                ErrorCode::ZAP2006,
                "expected a bitfield width (an integer) after ':'",
            );
            None
        }
    }

    fn parse_enum(&mut self, decorators: Vec<Decorator>) -> EnumDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Enum, "'enum'");
        let name = self.ident("enum name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut variants = Vec::new();
        let mut methods = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            // A pass-only body is the explicit spelling of an empty enum.
            if matches!(self.peek(), TokenKind::Ident(name) if name == "pass") {
                self.bump();
                self.expect(&TokenKind::Newline, "newline after 'pass'");
                continue;
            }
            // Methods on the enum (D-0046): a `def` (possibly decorated) in the
            // body, parsed exactly like a struct method.
            let decos = self.parse_decorators();
            if self.is_kw(Keyword::Def) {
                methods.push(self.parse_function(decos));
                if self.pos == before {
                    self.bump();
                }
                continue;
            }
            let vstart = self.peek_span().start;
            // `None` is a keyword (the null literal) but is also the conventional
            // empty-variant name for `Optional[T]` and similar sum types (I#154
            // Fork O); accept it here as a variant name.
            let vname = if self.is_kw(Keyword::None) {
                self.bump();
                "None".to_string()
            } else {
                self.ident("variant name")
            };
            // A data-carrying variant: `Circle(f64 r, f64 g, ...)` (D-0046).
            // Type-first named payload fields in parentheses.
            let mut fields = Vec::new();
            let mut value = None;
            if self.eat(&TokenKind::LParen) {
                if !self.is(&TokenKind::RParen) {
                    loop {
                        let fstart = self.peek_span().start;
                        let fty = self.parse_type();
                        let fname = self.ident("payload field name");
                        fields.push(VariantField {
                            ty: fty,
                            name: fname,
                            span: self.span_from(fstart),
                        });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen, "')'");
            } else if self.eat(&TokenKind::Assign) {
                // A tier-1 constant with an explicit discriminant (`GREEN = 10`).
                value = Some(self.parse_expr());
            }
            self.expect(&TokenKind::Newline, "newline after variant");
            variants.push(EnumVariant {
                name: vname,
                value,
                fields,
                span: self.span_from(vstart),
            });
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        EnumDef {
            decorators,
            name,
            generics,
            variants,
            methods,
            span: self.span_from(start),
        }
    }

    fn parse_interface(&mut self) -> InterfaceDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Interface, "'interface'");
        let name = self.ident("interface name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::Colon, "':'");
        // Optional refined-supertype list (D-0139/#806):
        // `interface Hashable: Equatable` or `interface X: A, B`. The `:` already
        // introduces the body block, so the supertype list is whatever types sit
        // between that `:` and the end-of-header NEWLINE; an immediate NEWLINE
        // (`interface Equatable:`) means no supertypes. This is unambiguous because
        // the body is INDENT-delimited, not `:`-delimited.
        let mut supertypes = Vec::new();
        if !self.is(&TokenKind::Newline) {
            supertypes.push(self.parse_type());
            while self.eat(&TokenKind::Comma) {
                supertypes.push(self.parse_type());
            }
        }
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut methods = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            let mstart = self.peek_span().start;
            self.expect_kw(Keyword::Def, "'def'");
            let mname = self.ident("method name");
            self.expect(&TokenKind::LParen, "'('");
            let params = self.parse_params();
            self.expect(&TokenKind::RParen, "')'");
            let ret = if self.eat(&TokenKind::Arrow) {
                Some(self.parse_type())
            } else {
                None
            };
            self.expect(&TokenKind::Newline, "newline after method signature");
            methods.push(MethodSig {
                name: mname,
                params,
                ret,
                span: self.span_from(mstart),
            });
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        InterfaceDef {
            name,
            generics,
            supertypes,
            methods,
            span: self.span_from(start),
        }
    }

    fn parse_annotation(&mut self) -> AnnotationDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Annotation, "'annotation'");
        let name = self.ident("annotation name");
        let (fields, _methods) = self.parse_struct_body();
        AnnotationDef {
            name,
            fields,
            span: self.span_from(start),
        }
    }

    fn parse_decorator_def(&mut self) -> DecoratorDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Decorator, "'decorator'");
        let name = self.ident("decorator name");
        let params = if self.eat(&TokenKind::LParen) {
            let ps = self.parse_params();
            self.expect(&TokenKind::RParen, "')'");
            ps
        } else {
            Vec::new()
        };
        // The body is either `pass` (a declared-only metadata decorator, D-0087)
        // or a sequence of `before`/`after`/`wrap` member hooks (D-0036). Unlike
        // a struct body these hooks take untyped macro bindings, so they are not
        // parsed as ordinary `def`s — see `parse_decorator_member`.
        let body_start = self.peek_span().start;
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut members = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            // A bare `pass` is the explicit spelling of an empty (declared-only)
            // decorator body, mirroring the empty-struct case (#591).
            if matches!(self.peek(), TokenKind::Ident(n) if n == "pass")
                && matches!(
                    self.peek2(),
                    TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
                )
            {
                self.bump();
                self.expect(&TokenKind::Newline, "newline after 'pass'");
                continue;
            }
            if self.is_kw(Keyword::Def) {
                members.push(self.parse_decorator_member());
            } else {
                self.error_here(
                    ErrorCode::ZAP2008,
                    "expected a `before`/`after`/`wrap` hook (a `def`) or `pass` in a decorator body",
                );
                self.bump();
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        DecoratorDef {
            name,
            params,
            members,
            body_span: self.span_from(body_start),
            span: self.span_from(start),
        }
    }

    /// Parse one `before`/`after`/`wrap` member hook of a behavioral decorator.
    /// The hook's parameters are untyped macro bindings (`ctx`, `result`,
    /// `call`), not type-first Zap parameters, so they are read as plain names.
    ///
    /// A hook whose body is a nested `def` (the `wrap(call)` → `def inner(...)`
    /// form, #653) is parsed into its structural shape: the inner signature and
    /// the trailing `return inner` are captured in [`DecoratorMember::wrap`] and
    /// the inner function's body in [`DecoratorMember::body`]. This is confined
    /// to the decorator-body parser — it is *not* a general nested-`def`
    /// language feature. The shape (single `*args` forwarder, `return inner`
    /// trailer) is validated during decorator expansion, not here.
    fn parse_decorator_member(&mut self) -> DecoratorMember {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Def, "'def'");
        let name = self.ident("decorator hook name");
        self.expect(&TokenKind::LParen, "'('");
        let mut params = Vec::new();
        if !self.is(&TokenKind::RParen) {
            loop {
                // Tolerate `*name` / `**name` so a `wrap` inner-forwarding shape
                // (`def inner(*args)`) still parses its parameter name.
                let _ = self.eat(&TokenKind::Star) || self.eat(&TokenKind::StarStar);
                params.push(self.ident("decorator hook parameter"));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.is(&TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "')'");
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        // A nested-`def` body is the `wrap(call)` form (#653): parse the inner
        // `def inner(...)` signature + body, then the `return inner` trailer.
        if self.is_kw(Keyword::Def) {
            let (wrap, body) = self.parse_wrap_body();
            self.expect(&TokenKind::Dedent, "end of block");
            return DecoratorMember {
                name,
                params,
                body: Some(body),
                wrap: Some(wrap),
                span: self.span_from(start),
            };
        }
        let body_start = self.peek_span().start;
        let mut stmts = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            self.parse_line(&mut stmts);
            if self.pos == before {
                self.error_here(
                    ErrorCode::ZAP2008,
                    "unexpected token in decorator hook body",
                );
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        DecoratorMember {
            name,
            params,
            body: Some(Block {
                stmts,
                span: self.span_from(body_start),
            }),
            wrap: None,
            span: self.span_from(start),
        }
    }

    /// Parse the body of a `wrap(call)` hook (#653): a nested
    /// `def inner(...):` with its own indented body, followed by the remaining
    /// statements of the `wrap` body (expected to be `return inner`). The
    /// opening `Indent` of the `wrap` body has already been consumed; the
    /// closing `Dedent` is left for the caller. Returns the structural
    /// [`WrapForm`] (inner signature + trailer) and the inner function's body.
    fn parse_wrap_body(&mut self) -> (WrapForm, Block) {
        let wrap_start = self.peek_span().start;
        self.expect_kw(Keyword::Def, "'def'");
        let inner_name = self.ident("nested `inner` function name");
        self.expect(&TokenKind::LParen, "'('");
        let mut inner_params = Vec::new();
        if !self.is(&TokenKind::RParen) {
            loop {
                let p_start = self.peek_span().start;
                let variadic = self.eat(&TokenKind::Star);
                let kwargs = !variadic && self.eat(&TokenKind::StarStar);
                let p_name = self.ident("`inner` parameter");
                inner_params.push(WrapParam {
                    name: p_name,
                    variadic,
                    kwargs,
                    span: self.span_from(p_start),
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.is(&TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "')'");
        let inner_body = self.parse_block();
        // The statements following the nested `def` in the `wrap` body — the
        // `return inner` trailer. Parsed as ordinary statements and validated
        // during expansion.
        let mut trailer = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            self.parse_line(&mut trailer);
            if self.pos == before {
                self.error_here(
                    ErrorCode::ZAP2008,
                    "unexpected token in `wrap` body after `def inner(...)`",
                );
                self.bump();
            }
        }
        (
            WrapForm {
                inner_name,
                inner_params,
                trailer,
                span: self.span_from(wrap_start),
            },
            inner_body,
        )
    }

    /// Parse an `import` statement. Returns one [`Import`] per imported module
    /// so the multi-import and grouped forms (D-0052) desugar to a flat list of
    /// single imports. Forms (D-0028 / D-0052):
    ///
    /// ```text
    /// import python "numpy" as np      # CPython interop (D-0012), single only
    /// import a                         # single logical module
    /// import a as b
    /// import a, b.c, d as e            # comma list
    /// import (a, b.c, d as e,)         # grouped; trailing comma allowed
    /// ```
    fn parse_import(&mut self) -> Vec<Import> {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Import, "'import'");
        // CPython interop (D-0012) is a single-module form only; the list and
        // grouped forms (D-0052) apply to logical-name modules.
        if self.eat_kw(Keyword::Python) {
            let module = self.string_lit();
            let alias = if self.eat_kw(Keyword::As) {
                Some(self.ident("alias"))
            } else {
                None
            };
            self.expect(&TokenKind::Newline, "newline");
            return vec![Import::Python {
                module,
                alias,
                span: self.span_from(start),
            }];
        }
        // Logical-name module import(s). The grouped form `import (...)` allows
        // newlines inside the parens (lexer implicit continuation) and a single
        // trailing comma; the bare comma-list form does not.
        let grouped = self.eat(&TokenKind::LParen);
        let mut imports = Vec::new();
        loop {
            imports.push(self.parse_import_item());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            // A trailing comma is permitted only in the grouped form.
            if grouped && self.is(&TokenKind::RParen) {
                break;
            }
        }
        if grouped {
            self.expect(&TokenKind::RParen, "')'");
        }
        self.expect(&TokenKind::Newline, "newline");
        imports
    }

    /// Parse a single `import_item` — `dotted_name [ "as" IDENT ]` (D-0052).
    fn parse_import_item(&mut self) -> Import {
        let start = self.peek_span().start;
        let path = self.dotted_name();
        let alias = if self.eat_kw(Keyword::As) {
            Some(self.ident("alias"))
        } else {
            None
        };
        Import::Module {
            path,
            alias,
            span: self.span_from(start),
        }
    }

    fn string_lit(&mut self) -> String {
        if let TokenKind::Str { value, .. } = self.peek() {
            let v = value.clone();
            self.bump();
            v
        } else {
            self.error_here(ErrorCode::ZAP2007, "expected a string literal");
            String::new()
        }
    }

    // --- blocks & statements ---

    fn parse_block(&mut self) -> Block {
        let start = self.peek_span().start;
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut stmts = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            self.parse_line(&mut stmts);
            if self.pos == before {
                self.error_here(ErrorCode::ZAP2008, "unexpected token in block");
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        Block {
            stmts,
            span: self.span_from(start),
        }
    }

    /// Parse one logical line into `out`: a compound statement, or one or more
    /// `;`-separated simple statements terminated by a newline.
    fn parse_line(&mut self, out: &mut Vec<Stmt>) {
        if self.is_compound_start() {
            out.push(self.parse_compound());
            return;
        }
        loop {
            out.push(self.parse_simple_stmt());
            if self.eat(&TokenKind::Semicolon) {
                if self.is(&TokenKind::Newline) || self.is(&TokenKind::Dedent) || self.at_eof() {
                    break;
                }
                continue;
            }
            break;
        }
        if !self.eat(&TokenKind::Newline) && !self.at_eof() && !self.is(&TokenKind::Dedent) {
            self.error_here(ErrorCode::ZAP2009, "expected newline after statement");
        }
    }

    fn is_compound_start(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Keyword(
                Keyword::If
                    | Keyword::While
                    | Keyword::Do
                    | Keyword::For
                    | Keyword::Match
                    | Keyword::Try
                    | Keyword::With
                    | Keyword::Scope
                    | Keyword::When
            )
        )
    }

    fn parse_compound(&mut self) -> Stmt {
        match self.peek() {
            TokenKind::Keyword(Keyword::If) => self.parse_if(),
            TokenKind::Keyword(Keyword::While) => self.parse_while(),
            TokenKind::Keyword(Keyword::Do) => self.parse_do_while(),
            TokenKind::Keyword(Keyword::For) => self.parse_for(),
            TokenKind::Keyword(Keyword::Match) => self.parse_match(),
            TokenKind::Keyword(Keyword::Try) => self.parse_try(),
            TokenKind::Keyword(Keyword::With) => self.parse_with(),
            TokenKind::Keyword(Keyword::Scope) => self.parse_scope(),
            TokenKind::Keyword(Keyword::When) => Stmt::Cfg(self.parse_cfg_stmt()),
            _ => unreachable!("parse_compound called on non-compound token"),
        }
    }

    // --- conditional compilation (`when target.*`, I#139 / D-0079) ---

    /// `when target.*` in item position: each arm's body is a block of items
    /// (imports, declarations, bare statements). The dead arms are parsed so
    /// they must be well-formed, but cfg resolution ([`crate::cfg`]) keeps only
    /// the matching arm before any later phase runs.
    fn parse_cfg_items(&mut self) -> CfgItemBlock {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::When, "'when'");
        let mut arms = Vec::new();
        let pred = self.parse_cfg_predicate();
        let body = self.parse_item_block();
        arms.push(CfgItemArm {
            predicate: pred,
            body,
            span: self.span_from(start),
        });
        while self.is_kw(Keyword::Elif) {
            let arm_start = self.peek_span().start;
            self.bump();
            let pred = self.parse_cfg_predicate();
            let body = self.parse_item_block();
            arms.push(CfgItemArm {
                predicate: pred,
                body,
                span: self.span_from(arm_start),
            });
        }
        let (else_body, else_span) = if self.is_kw(Keyword::Else) {
            let es = self.peek_span();
            self.bump();
            (Some(self.parse_item_block()), Some(es))
        } else {
            (None, None)
        };
        CfgItemBlock {
            arms,
            else_body,
            else_span,
            span: self.span_from(start),
        }
    }

    /// `when target.*` in statement position: each arm's body is an ordinary
    /// statement block, reusing [`Self::parse_block`].
    fn parse_cfg_stmt(&mut self) -> CfgStmtBlock {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::When, "'when'");
        let mut arms = Vec::new();
        let pred = self.parse_cfg_predicate();
        let body = self.parse_block();
        arms.push(CfgStmtArm {
            predicate: pred,
            body,
            span: self.span_from(start),
        });
        while self.is_kw(Keyword::Elif) {
            let arm_start = self.peek_span().start;
            self.bump();
            let pred = self.parse_cfg_predicate();
            let body = self.parse_block();
            arms.push(CfgStmtArm {
                predicate: pred,
                body,
                span: self.span_from(arm_start),
            });
        }
        let (else_body, else_span) = if self.is_kw(Keyword::Else) {
            let es = self.peek_span();
            self.bump();
            (Some(self.parse_block()), Some(es))
        } else {
            (None, None)
        };
        CfgStmtBlock {
            arms,
            else_body,
            else_span,
            span: self.span_from(start),
        }
    }

    /// `: NEWLINE INDENT { item } DEDENT` — the item-list counterpart of
    /// [`Self::parse_block`], used by item-position `when` arms.
    fn parse_item_block(&mut self) -> Vec<Item> {
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            self.parse_top_level(&mut items);
            self.skip_newlines();
            if self.pos == before {
                self.error_here(ErrorCode::ZAP2008, "unexpected token in `when` block");
                self.bump();
                self.skip_newlines();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        items
    }

    /// Parse one atomic `when`/`elif` target predicate (D-0079 §4), one of:
    /// `target.arch == .<arch>`, `target.os == .<os>`, `target.has(.<feature>)`.
    fn parse_cfg_predicate(&mut self) -> CfgPredicate {
        let start = self.peek_span().start;
        // `target`
        match self.ident_opt() {
            Some((name, _)) if name == "target" => {}
            _ => {
                self.error_here(
                    ErrorCode::ZAP2015,
                    "a `when` predicate must start with `target`",
                );
                return CfgPredicate::Arch(CfgArch::X86_64);
            }
        }
        self.expect(&TokenKind::Dot, "'.' after `target`");
        let field = self.ident("`arch`, `os`, or `has`");
        match field.as_str() {
            "arch" => {
                self.expect(&TokenKind::EqEq, "'==' after `target.arch`");
                let (val, span) = self.parse_dot_name();
                match crate::cfg::arch_from_str(&val) {
                    Some(a) => CfgPredicate::Arch(a),
                    None => {
                        self.error_at(
                            span,
                            ErrorCode::ZAP2016,
                            format!(
                                "unknown target arch `.{val}` (expected `.x86_64` or `.aarch64`)"
                            ),
                        );
                        CfgPredicate::Arch(CfgArch::X86_64)
                    }
                }
            }
            "os" => {
                self.expect(&TokenKind::EqEq, "'==' after `target.os`");
                let (val, span) = self.parse_dot_name();
                match crate::cfg::os_from_str(&val) {
                    Some(o) => CfgPredicate::Os(o),
                    None => {
                        self.error_at(
                            span,
                            ErrorCode::ZAP2016,
                            format!(
                                "unknown target os `.{val}` (expected `.linux`, `.windows`, or `.macos`)"
                            ),
                        );
                        CfgPredicate::Os(CfgOs::Linux)
                    }
                }
            }
            "has" => {
                self.expect(&TokenKind::LParen, "'(' after `target.has`");
                let (val, _) = self.parse_dot_name();
                self.expect(&TokenKind::RParen, "')' to close `target.has(...)`");
                CfgPredicate::HasFeature(val)
            }
            _ => {
                self.error_at(
                    self.span_from(start),
                    ErrorCode::ZAP2015,
                    "a `when` predicate tests `target.arch`, `target.os`, or `target.has(...)`",
                );
                CfgPredicate::Arch(CfgArch::X86_64)
            }
        }
    }

    /// Parse a leading-dot enum-style name `.ident` and return the identifier
    /// plus its span.
    fn parse_dot_name(&mut self) -> (String, Span) {
        let start = self.peek_span().start;
        self.expect(&TokenKind::Dot, "'.' before a target value");
        let name = self.ident("a target value name");
        (name, self.span_from(start))
    }

    /// `scope [ ( opts ) ]: BLOCK` (D-0044/D-0045). The optional parenthesized
    /// `opts` configure the failure policy / deadline (`on_error=`, `timeout=`);
    /// the block owns every `spawn` lexically inside it.
    fn parse_scope(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Scope, "'scope'");
        let opts = if self.eat(&TokenKind::LParen) {
            self.parse_args()
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        Stmt::Scope {
            opts,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_if(&mut self) -> Stmt {
        let start = self.peek_span().start;
        let mut branches = Vec::new();
        self.expect_kw(Keyword::If, "'if'");
        let cond = self.parse_expr();
        let body = self.parse_block();
        branches.push((cond, body));
        while self.is_kw(Keyword::Elif) {
            self.bump();
            let c = self.parse_expr();
            let b = self.parse_block();
            branches.push((c, b));
        }
        let else_block = if self.eat_kw(Keyword::Else) {
            Some(self.parse_block())
        } else {
            None
        };
        Stmt::If {
            branches,
            else_block,
            span: self.span_from(start),
        }
    }

    fn parse_while(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::While, "'while'");
        let cond = self.parse_expr();
        let body = self.parse_block();
        Stmt::While {
            cond,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_do_while(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Do, "'do'");
        let body = self.parse_block();
        self.expect_kw(Keyword::While, "'while'");
        let cond = self.parse_expr();
        if !self.eat(&TokenKind::Newline) && !self.at_eof() && !self.is(&TokenKind::Dedent) {
            self.error_here(ErrorCode::ZAP2009, "expected newline after 'do'-'while'");
        }
        Stmt::DoWhile {
            body,
            cond,
            span: self.span_from(start),
        }
    }

    fn parse_for(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::For, "'for'");
        let target = self.parse_target();
        self.expect_kw(Keyword::In, "'in'");
        let iter = self.parse_expr();
        let body = self.parse_block();
        Stmt::For {
            target,
            iter,
            body,
            span: self.span_from(start),
        }
    }

    /// A for-loop / assignment target: a postfix expression, or a tuple of them.
    /// Does not consume binary operators (notably `in`).
    fn parse_target(&mut self) -> Expr {
        let start = self.peek_span().start;
        let first = self.parse_postfix();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.is_kw(Keyword::In) || self.is(&TokenKind::Assign) {
                    break;
                }
                elems.push(self.parse_postfix());
            }
            Expr::Tuple(elems, self.span_from(start))
        } else {
            first
        }
    }

    fn parse_match(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Match, "'match'");
        let scrutinee = self.parse_expr();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut arms = Vec::new();
        while self.is_kw(Keyword::Case) {
            let astart = self.peek_span().start;
            self.bump();
            let pattern = self.parse_pattern();
            let guard = if self.eat_kw(Keyword::If) {
                Some(self.parse_expr())
            } else {
                None
            };
            let body = self.parse_block();
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span: self.span_from(astart),
            });
        }
        self.expect(&TokenKind::Dedent, "end of match block");
        Stmt::Match {
            scrutinee,
            arms,
            span: self.span_from(start),
        }
    }

    fn parse_try(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Try, "'try'");
        let body = self.parse_block();
        let mut excepts = Vec::new();
        while self.is_kw(Keyword::Except) {
            let estart = self.peek_span().start;
            self.bump();
            // `except*` (#605, D-0129): a group-handling clause. The `*` binds
            // tightly to `except` (Python spelling), with or without surrounding
            // whitespace (`except* E`, `except *E`). The all-`except` /
            // all-`except*` rule and the "a type is required" rule are enforced
            // in sema, not here, so the parser stays permissive.
            let is_star = self.eat(&TokenKind::Star);
            let ty = if self.is(&TokenKind::Colon) {
                None
            } else {
                Some(self.parse_type())
            };
            let binding = if self.eat_kw(Keyword::As) {
                Some(self.ident("binding name"))
            } else {
                None
            };
            let eb = self.parse_block();
            excepts.push(ExceptClause {
                ty,
                binding,
                body: eb,
                is_star,
                span: self.span_from(estart),
            });
        }
        let finally = if self.eat_kw(Keyword::Finally) {
            Some(self.parse_block())
        } else {
            None
        };
        Stmt::Try {
            body,
            excepts,
            finally,
            span: self.span_from(start),
        }
    }

    fn parse_with(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::With, "'with'");
        let mut items = Vec::new();
        loop {
            let istart = self.peek_span().start;
            let expr = self.parse_expr();
            let alias = if self.eat_kw(Keyword::As) {
                Some(self.ident("binding name"))
            } else {
                None
            };
            items.push(WithItem {
                expr,
                alias,
                span: self.span_from(istart),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let body = self.parse_block();
        Stmt::With {
            items,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_simple_stmt(&mut self) -> Stmt {
        let start = self.peek_span().start;
        // `pass` is a contextual no-op statement (#591), not a reserved keyword,
        // so it stays usable as an identifier everywhere else. It is recognized
        // only when it stands alone as a statement: the bare word `pass`
        // immediately followed by a statement terminator. In any other position
        // (`pass = x`, `pass(...)`, `pass.foo`, `f(pass)`) it falls through and
        // parses as an ordinary identifier.
        if matches!(self.peek(), TokenKind::Ident(n) if n == "pass")
            && matches!(
                self.peek2(),
                TokenKind::Newline | TokenKind::Semicolon | TokenKind::Dedent | TokenKind::Eof
            )
        {
            self.bump();
            return Stmt::Pass(self.span_from(start));
        }
        // `del d[k]` / `del xs[i]` — element removal (#595). Like `pass`, `del`
        // is a contextual statement-position word, not a reserved keyword, so it
        // stays usable as an ordinary identifier everywhere else. It is taken as
        // the `del` statement only when the bare word `del` is immediately
        // followed by another identifier (`del d...`): that two-identifier start
        // is never a valid expression, assignment (`del = x`), call (`del(x)`),
        // index (`del[0]`) or attribute (`del.x`) on a binding named `del`, so
        // those uses are left untouched. The target must be a subscript; a
        // non-subscript target (binding deletion) is rejected below.
        if matches!(self.peek(), TokenKind::Ident(n) if n == "del")
            && matches!(self.peek2(), TokenKind::Ident(_))
        {
            self.bump();
            let target = self.parse_expr();
            if !matches!(target, Expr::Index { .. }) {
                self.error_at(
                    self.span_from(start),
                    ErrorCode::ZAP2013,
                    "`del` removes a collection element and requires a subscript \
                     target (`del d[k]` or `del xs[i]`); deleting a binding is not \
                     supported",
                );
            }
            return Stmt::Del {
                target,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Return) {
            let value = if self.at_line_end() {
                None
            } else {
                // A bare comma-list is a tuple return (D-0049): `return a, b`
                // builds a tuple the caller destructures — there is no separate
                // multiple-return mechanism.
                let first = self.parse_expr();
                if self.is(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while self.eat(&TokenKind::Comma) {
                        if self.at_line_end() {
                            break; // trailing comma
                        }
                        elems.push(self.parse_expr());
                    }
                    Some(Expr::Tuple(elems, self.span_from(start)))
                } else {
                    Some(first)
                }
            };
            return Stmt::Return {
                value,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Raise) {
            let value = if self.at_line_end() {
                None
            } else {
                Some(self.parse_expr())
            };
            // `raise <exc> from <cause>` (#604, D-0103). `from` is a contextual
            // word here (not a reserved keyword, like `pass`/`del`): it is the
            // cause clause only when it follows a raised value.
            let cause =
                if value.is_some() && matches!(self.peek(), TokenKind::Ident(n) if n == "from") {
                    self.bump();
                    Some(self.parse_expr())
                } else {
                    None
                };
            return Stmt::Raise {
                value,
                cause,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Assert) {
            // `assert cond [, msg]` (D-0054). The condition's source span is
            // captured so power-assert can echo the exact expression text.
            let cond_start = self.peek_span().start;
            let cond = self.parse_expr();
            let cond_src = self.source_slice(Span::new(cond_start, self.prev_end()));
            let cond_src = cond_src.trim().to_string();
            let msg = if self.eat(&TokenKind::Comma) {
                Some(self.parse_expr())
            } else {
                None
            };
            return Stmt::Assert {
                cond,
                msg,
                cond_src,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Break) {
            return Stmt::Break(self.span_from(start));
        }
        if self.eat_kw(Keyword::Continue) {
            return Stmt::Continue(self.span_from(start));
        }
        if self.eat_kw(Keyword::Spawn) {
            // `detached` is a contextual modifier (D-0044), not a reserved word:
            // valid only immediately after `spawn` and only when it introduces a
            // call (`spawn detached f(...)`). A bare `detached` identifier that is
            // itself the callee (`spawn detached()`) stays an ordinary call.
            let detached = matches!(self.peek(), TokenKind::Ident(n) if n == "detached")
                && !matches!(self.peek2(), TokenKind::LParen);
            if detached {
                self.bump();
            }
            let call = self.parse_expr();
            return Stmt::Spawn {
                call,
                detached,
                span: self.span_from(start),
            };
        }
        if let Some(decl) = self.try_declaration(start) {
            return decl;
        }
        // assignment / aug-assignment / substitution / expression statement
        let target = self.parse_expr();
        // A bare `a := expr` statement is rejected (#600 non-goal): `:=` is the
        // walrus assignment-expression, valid only inside parentheses (as a
        // condition or comprehension filter), never as a declaration shorthand.
        // Steer to the declaration form and recover by consuming the RHS.
        if self.is(&TokenKind::ColonEq) {
            let name = match &target {
                Expr::Name(n, _) => n.clone(),
                _ => "name".to_string(),
            };
            self.error_at(
                self.peek_span(),
                ErrorCode::ZAP2020,
                format!(
                    "`:=` cannot declare a variable here; write `auto {name} = ...` \
                     (the walrus `:=` is only valid in a parenthesized condition or \
                     comprehension filter, e.g. `if (x := f()) > 0:`)"
                ),
            );
            self.bump(); // ':='
            let _ = self.parse_expr();
            return Stmt::Expr(target);
        }
        // Bare comma-list destructuring target (D-0049): `a, b = ...`,
        // `first, *rest = ...`. The first element was already parsed as an
        // expression; collect the rest as target elements (which may carry `*`).
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![BindTarget::Elem(target)];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::Assign) || self.at_line_end() {
                    break; // trailing comma
                }
                elems.push(self.parse_target_elem());
            }
            let bt = BindTarget::Tuple(elems, self.span_from(start));
            if self.eat(&TokenKind::Assign) {
                let value = self.parse_rhs();
                return Stmt::AssignTuple {
                    target: bt,
                    value,
                    span: self.span_from(start),
                };
            }
            // Not an assignment: a bare tuple expression statement.
            return Stmt::Expr(bind_target_to_expr(bt));
        }
        if self.eat(&TokenKind::Assign) {
            // A parenthesized tuple on the left is a destructuring target
            // (`(x, y) = (y, x)`); anything else is an ordinary lvalue.
            if matches!(target, Expr::Tuple(..)) {
                let bt = expr_to_bind_target(target);
                let value = self.parse_rhs();
                return Stmt::AssignTuple {
                    target: bt,
                    value,
                    span: self.span_from(start),
                };
            }
            let value = self.parse_rhs();
            // Chained assignment `t0 = t1 = ... = value` (#594). The first `=`
            // has been consumed and `value` parsed; while another `=` follows,
            // the value parsed so far is actually the next target and a further
            // right-hand side is parsed. Python semantics: the final RHS is
            // evaluated once and bound to every target left to right.
            if self.eat(&TokenKind::Assign) {
                let mut targets = vec![target, value];
                let mut rhs = self.parse_rhs();
                while self.eat(&TokenKind::Assign) {
                    targets.push(rhs);
                    rhs = self.parse_rhs();
                }
                return Stmt::AssignChain {
                    targets,
                    value: rhs,
                    span: self.span_from(start),
                };
            }
            return Stmt::Assign {
                target,
                value,
                span: self.span_from(start),
            };
        }
        if let Some(op) = self.aug_assign_op() {
            self.bump();
            let value = self.parse_expr();
            return Stmt::AugAssign {
                target,
                op,
                value,
                span: self.span_from(start),
            };
        }
        if self.is(&TokenKind::SubstAssign) {
            self.bump();
            let sub = self.parse_subst_literal(target.clone());
            return Stmt::Assign {
                target,
                value: sub,
                span: self.span_from(start),
            };
        }
        Stmt::Expr(target)
    }

    fn at_line_end(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Newline | TokenKind::Semicolon | TokenKind::Dedent | TokenKind::Eof
        )
    }

    fn aug_assign_op(&self) -> Option<BinOp> {
        Some(match self.peek() {
            TokenKind::PlusEq => BinOp::Add,
            TokenKind::MinusEq => BinOp::Sub,
            TokenKind::StarEq => BinOp::Mul,
            TokenKind::SlashEq => BinOp::Div,
            TokenKind::SlashSlashEq => BinOp::FloorDiv,
            TokenKind::PercentEq => BinOp::Mod,
            TokenKind::StarStarEq => BinOp::Pow,
            TokenKind::AmpEq => BinOp::BitAnd,
            TokenKind::PipeEq => BinOp::BitOr,
            TokenKind::CaretEq => BinOp::BitXor,
            TokenKind::ShlEq => BinOp::Shl,
            TokenKind::ShrEq => BinOp::Shr,
            _ => return None,
        })
    }

    fn try_declaration(&mut self, start: u32) -> Option<Stmt> {
        let cp = (self.pos, self.errors.len());
        let is_const = self.eat_kw(Keyword::Const);
        // `auto bind_target = expr` — a single name stays a plain [`Stmt::Let`];
        // a tuple target (`auto (a, b) = t`, `auto first, *rest = xs`) becomes a
        // [`Stmt::LetTuple`] (D-0049).
        if self.eat_kw(Keyword::Auto) {
            let target = self.parse_bind_target();
            if self.eat(&TokenKind::Assign) {
                let value = self.parse_rhs();
                return Some(self.make_decl(is_const, None, target, value, start));
            }
            self.restore(cp);
            return None;
        }
        // `TYPE bind_target = expr`
        if self.looks_like_type_start() {
            let ty = self.parse_type();
            let target = self.parse_bind_target();
            // Commit only on a *clean* parse that reaches the declaration `=`.
            // A bare expression statement that merely starts like a type — e.g.
            // a call with keyword arguments, `xs.sort(reverse=True)` /
            // `f(x=1)` — makes the speculative type/target parse over-consume and
            // emit errors, and the kwarg's own `=` then looks like the
            // declaration's. Rolling back when errors were produced lets the
            // expression-statement parser (below) handle it instead (I#336).
            if self.errors.len() == cp.1 && self.eat(&TokenKind::Assign) {
                let value = self.parse_rhs();
                return Some(self.make_decl(is_const, Some(ty), target, value, start));
            }
        }
        self.restore(cp);
        None
    }

    /// Build a declaration from a parsed bind target: a single `name` target is
    /// a plain [`Stmt::Let`]; any tuple/rest target is a [`Stmt::LetTuple`].
    fn make_decl(
        &self,
        is_const: bool,
        ty: Option<Type>,
        target: BindTarget,
        value: Expr,
        start: u32,
    ) -> Stmt {
        let span = self.span_from(start);
        if let BindTarget::Elem(Expr::Name(name, _)) = &target {
            return Stmt::Let {
                is_const,
                ty,
                name: name.clone(),
                value,
                span,
            };
        }
        Stmt::LetTuple {
            is_const,
            ty,
            target,
            value,
            span,
        }
    }

    /// The right-hand side of a declaration or assignment. A bare comma-list is a
    /// tuple value (D-0049), mirroring the bare comma-list tuple return — so
    /// `a, b = b, a` and `auto t = 1, 2` build tuples. A parenthesized tuple is
    /// handled by the ordinary expression parser.
    fn parse_rhs(&mut self) -> Expr {
        let start = self.peek_span().start;
        let first = self.parse_expr();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.at_line_end() {
                    break; // trailing comma
                }
                elems.push(self.parse_expr());
            }
            Expr::Tuple(elems, self.span_from(start))
        } else {
            first
        }
    }

    /// A `bind_target` (D-0049): a single target, or a comma-list / parenthesized
    /// tuple of target elements. Used after `auto`/`TYPE` in a declaration.
    fn parse_bind_target(&mut self) -> BindTarget {
        let start = self.peek_span().start;
        let first = self.parse_target_elem();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::Assign) || self.at_line_end() {
                    break; // trailing comma
                }
                elems.push(self.parse_target_elem());
            }
            BindTarget::Tuple(elems, self.span_from(start))
        } else {
            first
        }
    }

    /// A `target_elem` (D-0049): `[*] target`, or a parenthesized nested tuple.
    fn parse_target_elem(&mut self) -> BindTarget {
        let start = self.peek_span().start;
        if self.eat(&TokenKind::Star) {
            let inner = self.parse_target_elem();
            return BindTarget::Rest(Box::new(inner), self.span_from(start));
        }
        if self.is(&TokenKind::LParen) {
            self.bump(); // '('
            let mut elems = Vec::new();
            let mut saw_comma = false;
            if !self.is(&TokenKind::RParen) {
                loop {
                    elems.push(self.parse_target_elem());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    saw_comma = true;
                    if self.is(&TokenKind::RParen) {
                        break; // trailing comma
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')'");
            // A single parenthesized target with no comma is just grouping;
            // `(x,)` (one element with a trailing comma) is a 1-tuple.
            if elems.len() == 1 && !saw_comma && !matches!(elems[0], BindTarget::Rest(..)) {
                return elems.into_iter().next().unwrap();
            }
            return BindTarget::Tuple(elems, self.span_from(start));
        }
        BindTarget::Elem(self.parse_postfix())
    }

    fn restore(&mut self, cp: (usize, usize)) {
        self.pos = cp.0;
        self.errors.truncate(cp.1);
    }

    fn looks_like_type_start(&self) -> bool {
        match self.peek() {
            TokenKind::Keyword(k) => k.is_type(),
            TokenKind::Ident(_) => true,
            _ => false,
        }
    }

    // --- types ---

    fn parse_type(&mut self) -> Type {
        let start = self.peek_span().start;
        // Function value type `fn(T, U) -> R` (I#163, grammar `func_type`). `fn`
        // is a soft keyword, recognized only here in type position (and only when
        // followed by `(`), so it stays a usable identifier everywhere else.
        if matches!(self.peek(), TokenKind::Ident(n) if n == "fn")
            && *self.peek2() == TokenKind::LParen
        {
            self.bump(); // `fn`
            self.expect(&TokenKind::LParen, "'('");
            let mut params = Vec::new();
            if !self.is(&TokenKind::RParen) {
                loop {
                    params.push(self.parse_type());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    if self.is(&TokenKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')'");
            let ret = if self.eat(&TokenKind::Arrow) {
                Some(Box::new(self.parse_type()))
            } else {
                None
            };
            return Type::Func {
                params,
                ret,
                span: self.span_from(start),
            };
        }
        if self.eat(&TokenKind::LParen) {
            let mut elems = Vec::new();
            if !self.is(&TokenKind::RParen) {
                loop {
                    elems.push(self.parse_type());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    if self.is(&TokenKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')'");
            return Type::Tuple {
                elems,
                span: self.span_from(start),
            };
        }
        let (mut name, dotted_ok) = match self.peek() {
            TokenKind::Keyword(k) if k.is_type() => {
                let s = k.as_str().to_string();
                self.bump();
                (s, false)
            }
            TokenKind::Ident(n) => {
                let s = n.clone();
                self.bump();
                (s, true)
            }
            _ => {
                self.error_here(ErrorCode::ZAP2010, "expected a type");
                ("<error>".to_string(), false)
            }
        };
        // A module-qualified type name, `mod.Type` (I#283/D-0081): an identifier
        // base may be followed by `.ident` segments. The qualifier is resolved to
        // the imported module's type during module linking. Keyword types
        // (`int64`, …) are never dotted.
        if dotted_ok {
            while self.is(&TokenKind::Dot) {
                self.bump();
                name.push('.');
                name.push_str(&self.ident("type name"));
            }
        }
        let mut args = Vec::new();
        if self.eat(&TokenKind::LBracket) {
            if !self.is(&TokenKind::RBracket) {
                loop {
                    let before = self.pos;
                    args.push(self.parse_type());
                    if self.pos == before {
                        self.bump(); // ensure progress
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RBracket, "']'");
        }
        Type::Named {
            name,
            args,
            span: self.span_from(start),
        }
    }

    // --- expressions ---

    fn parse_expr(&mut self) -> Expr {
        if self.is_kw(Keyword::Lambda) {
            return self.parse_lambda();
        }
        let start = self.peek_span().start;
        let value = self.parse_or_expr();
        if self.eat_kw(Keyword::If) {
            let cond = self.parse_or_expr();
            self.expect_kw(Keyword::Else, "'else'");
            let els = self.parse_expr();
            return Expr::IfElse {
                cond: Box::new(cond),
                then: Box::new(value),
                els: Box::new(els),
                span: self.span_from(start),
            };
        }
        value
    }

    fn parse_lambda(&mut self) -> Expr {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Lambda, "'lambda'");
        let mut params = Vec::new();
        if !self.is(&TokenKind::Colon) {
            loop {
                params.push(self.ident("lambda parameter"));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::Colon, "':'");
        let body = self.parse_expr();
        Expr::Lambda {
            params,
            body: Box::new(body),
            span: self.span_from(start),
        }
    }

    fn parse_or_expr(&mut self) -> Expr {
        let start = self.peek_span().start;
        let mut lhs = self.parse_and_expr();
        while self.is_kw(Keyword::Or) {
            self.bump();
            let rhs = self.parse_and_expr();
            lhs = Expr::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        lhs
    }

    fn parse_and_expr(&mut self) -> Expr {
        let start = self.peek_span().start;
        let mut lhs = self.parse_not_expr();
        while self.is_kw(Keyword::And) {
            self.bump();
            let rhs = self.parse_not_expr();
            lhs = Expr::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        lhs
    }

    fn parse_not_expr(&mut self) -> Expr {
        if self.is_kw(Keyword::Not) {
            let start = self.peek_span().start;
            self.bump();
            let expr = self.parse_not_expr();
            return Expr::Unary {
                op: UnOp::Not,
                expr: Box::new(expr),
                span: self.span_from(start),
            };
        }
        self.parse_bin(0)
    }

    /// Precedence-climbing over comparison/bitwise/shift/arithmetic operators
    /// and the regex-bind operators. `min_bp` is the minimum left binding power.
    ///
    /// Comparison operators are additionally *chained* rather than left-folded
    /// (see [`is_chain_cmp`] and [`Expr::Compare`]).
    fn parse_bin(&mut self, min_bp: u8) -> Expr {
        let start = self.peek_span().start;
        let mut lhs = self.parse_operand();
        loop {
            // Regex bind (`=~` / `!~`) at comparison precedence.
            if matches!(self.peek(), TokenKind::RegexBind | TokenKind::RegexNotBind) {
                if 30 < min_bp {
                    break;
                }
                let negated = matches!(self.peek(), TokenKind::RegexNotBind);
                self.bump();
                lhs = self.parse_regex_rhs(lhs, negated, start);
                continue;
            }
            // `not in` membership (#593): the two-token negated-membership
            // operator, at the same precedence as `in` (30). It type-checks
            // (operand rules of `in`, result `bool`) and lowers identically to
            // the negation of `in`. `not` stays a prefix operator and `in` an
            // infix operator in every other position. Unlike `in`, it does not
            // chain, so it is emitted as a lone `Binary` rather than joining an
            // `Expr::Compare` run.
            if self.is_kw(Keyword::Not) && matches!(self.peek2(), TokenKind::Keyword(Keyword::In)) {
                if 30 < min_bp {
                    break;
                }
                self.bump(); // `not`
                self.bump(); // `in`
                let rhs = self.parse_bin(31);
                lhs = Expr::Binary {
                    op: BinOp::NotIn,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span: self.span_from(start),
                };
                continue;
            }
            // Chained comparison (Python semantics, I#386): a run of two or more
            // comparison operators at the same level — `a < b < c`, `x is b == c`
            // — is NOT left-associative `(a < b) < c`; it desugars to `(a < b)
            // and (b < c)` with each operand evaluated once. Collect the whole
            // run here so the parser can emit a single [`Expr::Compare`]; a lone
            // comparison becomes an ordinary left-associative `Expr::Binary`.
            // The two-token `is`/`is not` identity operators (#611, D18) chain
            // like the others; [`peek_chain_cmp`] reports how many tokens each
            // consumes so `is not` is bumped as a unit.
            if let Some((bp, op, ntok)) = self.peek_chain_cmp() {
                if bp < min_bp {
                    break;
                }
                self.bump_n(ntok);
                let mut rest = vec![(op, self.parse_bin(bp + 1))];
                while let Some((bp2, op2, ntok2)) = self.peek_chain_cmp() {
                    self.bump_n(ntok2);
                    rest.push((op2, self.parse_bin(bp2 + 1)));
                }
                lhs = if rest.len() == 1 {
                    let (op1, rhs) = rest.pop().expect("one link");
                    Expr::Binary {
                        op: op1,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        span: self.span_from(start),
                    }
                } else {
                    Expr::Compare {
                        first: Box::new(lhs),
                        rest,
                        span: self.span_from(start),
                    }
                };
                continue;
            }
            let Some((bp, op)) = self.bin_bp() else {
                break;
            };
            if bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_bin(bp + 1);
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        lhs
    }

    /// Whether `op` is a single-token comparison operator that participates in
    /// Python-style comparison chaining (`<`, `<=`, `>`, `>=`, `==`, `!=`,
    /// `in`). The regex binds (`=~`/`!~`), `Send`, and the bitwise/arithmetic
    /// operators do not chain. `not in` (#593, [`BinOp::NotIn`]) is a standalone
    /// negated-membership comparison that does not chain. The identity
    /// operators `is`/`is not` (#611, D18) DO chain, but are two-token forms
    /// recognized by [`peek_chain_cmp`] rather than here.
    fn is_chain_cmp(op: BinOp) -> bool {
        matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::In
        )
    }

    /// Peek the next chaining comparison operator without consuming it,
    /// returning `(binding power, op, token count)`. This unifies the
    /// single-token comparison operators ([`is_chain_cmp`]) with the two-token
    /// `is` / `is not` identity operators (#611, D18): `is not` reports a token
    /// count of 2 so the caller bumps it as one operator. All comparison
    /// operators share precedence 30, so `x is b == c` chains uniformly.
    /// Returns `None` for `not in` (handled before this as a non-chaining
    /// operator) and for every non-comparison operator.
    fn peek_chain_cmp(&self) -> Option<(u8, BinOp, u8)> {
        if self.is_kw(Keyword::Is) {
            if matches!(self.peek2(), TokenKind::Keyword(Keyword::Not)) {
                return Some((30, BinOp::IsNot, 2));
            }
            return Some((30, BinOp::Is, 1));
        }
        match self.bin_bp() {
            Some((bp, op)) if Self::is_chain_cmp(op) => Some((bp, op, 1)),
            _ => None,
        }
    }

    /// Consume `n` tokens (used to bump a multi-token operator such as
    /// `is not` as a unit).
    fn bump_n(&mut self, n: u8) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn bin_bp(&self) -> Option<(u8, BinOp)> {
        Some(match self.peek() {
            TokenKind::LArrow => (25, BinOp::Send),
            TokenKind::EqEq => (30, BinOp::Eq),
            TokenKind::Ne => (30, BinOp::Ne),
            TokenKind::Lt => (30, BinOp::Lt),
            TokenKind::Le => (30, BinOp::Le),
            TokenKind::Gt => (30, BinOp::Gt),
            TokenKind::Ge => (30, BinOp::Ge),
            TokenKind::Keyword(Keyword::In) => (30, BinOp::In),
            TokenKind::Pipe => (40, BinOp::BitOr),
            TokenKind::Caret => (50, BinOp::BitXor),
            TokenKind::Amp => (60, BinOp::BitAnd),
            TokenKind::Shl => (70, BinOp::Shl),
            TokenKind::Shr => (70, BinOp::Shr),
            TokenKind::Plus => (80, BinOp::Add),
            TokenKind::Minus => (80, BinOp::Sub),
            TokenKind::Star => (90, BinOp::Mul),
            TokenKind::Slash => (90, BinOp::Div),
            TokenKind::SlashSlash => (90, BinOp::FloorDiv),
            TokenKind::Percent => (90, BinOp::Mod),
            _ => return None,
        })
    }

    fn parse_regex_rhs(&mut self, subject: Expr, negated: bool, start: u32) -> Expr {
        match self.peek().clone() {
            TokenKind::RegexMatch { pattern, flags } => {
                self.bump();
                Expr::RegexMatch {
                    subject: Box::new(subject),
                    pattern,
                    flags,
                    negated,
                    span: self.span_from(start),
                }
            }
            TokenKind::RegexSubst {
                pattern,
                replacement,
                flags,
            } if !negated => {
                self.bump();
                Expr::RegexSubst {
                    subject: Box::new(subject),
                    pattern,
                    replacement,
                    flags,
                    span: self.span_from(start),
                }
            }
            _ => {
                self.error_here(
                    ErrorCode::ZAP2011,
                    "expected a regex literal after '=~'/'!~'",
                );
                subject
            }
        }
    }

    fn parse_subst_literal(&mut self, subject: Expr) -> Expr {
        let start = self.peek_span().start;
        match self.peek().clone() {
            TokenKind::RegexSubst {
                pattern,
                replacement,
                flags,
            } => {
                self.bump();
                Expr::RegexSubst {
                    subject: Box::new(subject),
                    pattern,
                    replacement,
                    flags,
                    span: self.span_from(start),
                }
            }
            _ => {
                self.error_here(
                    ErrorCode::ZAP2012,
                    "expected a substitution literal after '~='",
                );
                subject
            }
        }
    }

    /// Unary (`u_expr`): prefix `-`/`+`/`~`, prefix `<-` (receive), `yield`,
    /// else a `power`.
    fn parse_operand(&mut self) -> Expr {
        let start = self.peek_span().start;
        match self.peek() {
            TokenKind::Minus => {
                self.bump();
                Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::Plus => {
                self.bump();
                Expr::Unary {
                    op: UnOp::Pos,
                    expr: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::Tilde => {
                self.bump();
                Expr::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::LArrow => {
                self.bump();
                Expr::ChanRecv {
                    chan: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::Keyword(Keyword::Yield) => {
                self.bump();
                let value = if self.at_line_end() || self.is(&TokenKind::RParen) {
                    None
                } else {
                    Some(Box::new(self.parse_expr()))
                };
                Expr::Yield {
                    value,
                    span: self.span_from(start),
                }
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Expr {
        let start = self.peek_span().start;
        let base = self.parse_postfix();
        if self.eat(&TokenKind::StarStar) {
            let exp = self.parse_operand(); // right-assoc, allows unary on RHS
            Expr::Binary {
                op: BinOp::Pow,
                lhs: Box::new(base),
                rhs: Box::new(exp),
                span: self.span_from(start),
            }
        } else {
            base
        }
    }

    fn parse_postfix(&mut self) -> Expr {
        let start = self.peek_span().start;
        let mut e = self.parse_atom();
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_args();
                    e = Expr::Call {
                        callee: Box::new(e),
                        args,
                        span: self.span_from(start),
                    };
                }
                TokenKind::LBracket => {
                    e = self.parse_subscript(e, start);
                }
                TokenKind::Dot => {
                    self.bump();
                    let name = self.member_name("attribute name");
                    e = Expr::Attr {
                        base: Box::new(e),
                        name,
                        span: self.span_from(start),
                    };
                }
                _ => break,
            }
        }
        e
    }

    fn parse_args(&mut self) -> Vec<Arg> {
        let mut args = Vec::new();
        if self.eat(&TokenKind::RParen) {
            return args;
        }
        loop {
            let astart = self.peek_span().start;
            // Spread arguments (#597): `**expr` unpacks a `dict[str, V]` into
            // keyword arguments; `*expr` unpacks an iterable/list/tuple into
            // positional arguments. Checked before the keyword-argument form so a
            // leading `*`/`**` is never mistaken for a bare expression.
            let spread = if self.eat(&TokenKind::StarStar) {
                ArgSpread::Mapping
            } else if self.eat(&TokenKind::Star) {
                ArgSpread::Iterable
            } else {
                ArgSpread::None
            };
            // keyword argument: IDENT '=' (but not '=='). A spread argument is
            // never named.
            let name = if spread == ArgSpread::None
                && matches!(self.peek(), TokenKind::Ident(_))
                && matches!(self.peek2(), TokenKind::Assign)
            {
                let (n, _) = self.ident_opt().unwrap();
                self.bump(); // '='
                Some(n)
            } else {
                None
            };
            let value = self.parse_expr();
            // A bare generator expression as the SOLE call argument needs no
            // parentheses of its own (#596): `sum(x for x in xs)`. The call's
            // own parentheses bound it. It is only allowed as the one and only
            // argument — with any other argument it must be written with
            // explicit parentheses `f((x for x in xs), y)` — and never carries a
            // name or spread.
            if self.is_kw(Keyword::For)
                && args.is_empty()
                && spread == ArgSpread::None
                && name.is_none()
            {
                let clauses = self.parse_comp_clauses();
                let span = self.span_from(astart);
                args.push(Arg {
                    name: None,
                    value: Expr::GenExp {
                        elem: Box::new(value),
                        clauses,
                        span,
                    },
                    spread: ArgSpread::None,
                    span,
                });
                self.expect(&TokenKind::RParen, "')'");
                return args;
            }
            args.push(Arg {
                name,
                value,
                spread,
                span: self.span_from(astart),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.is(&TokenKind::RParen) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')'");
        args
    }

    fn parse_subscript(&mut self, base: Expr, start: u32) -> Expr {
        self.expect(&TokenKind::LBracket, "'['");
        let lo = if self.is(&TokenKind::Colon) || self.is(&TokenKind::RBracket) {
            None
        } else {
            Some(Box::new(self.parse_expr()))
        };
        if self.is(&TokenKind::Colon) {
            self.bump();
            let hi = if self.is(&TokenKind::Colon) || self.is(&TokenKind::RBracket) {
                None
            } else {
                Some(Box::new(self.parse_expr()))
            };
            let step = if self.eat(&TokenKind::Colon) {
                if self.is(&TokenKind::RBracket) {
                    None
                } else {
                    Some(Box::new(self.parse_expr()))
                }
            } else {
                None
            };
            self.expect(&TokenKind::RBracket, "']'");
            Expr::Slice {
                base: Box::new(base),
                lo,
                hi,
                step,
                span: self.span_from(start),
            }
        } else if self.is(&TokenKind::Comma) {
            // A comma-separated postfix `base[a, b, ...]` is a multi-element
            // subscript. Classified in sema/monomorphization: when `base` names a
            // type or generic callee it is call-site type arguments
            // (`Pair[int64, str]`, `map[int64, str]`, I#154 Fork S); otherwise it
            // is a tuple value subscript. The parser produces a uniform node: the
            // index is a tuple of the elements.
            let tstart = self.peek_span().start;
            let mut elems = vec![*lo.expect("lo parsed before a comma")];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::RBracket) {
                    break; // trailing comma
                }
                elems.push(self.parse_expr());
            }
            self.expect(&TokenKind::RBracket, "']'");
            Expr::Index {
                base: Box::new(base),
                index: Box::new(Expr::Tuple(elems, self.span_from(tstart))),
                span: self.span_from(start),
            }
        } else {
            self.expect(&TokenKind::RBracket, "']'");
            let index = lo.unwrap_or_else(|| {
                self.error_here(ErrorCode::ZAP2013, "expected an index expression");
                Box::new(Expr::None(self.peek_span()))
            });
            Expr::Index {
                base: Box::new(base),
                index,
                span: self.span_from(start),
            }
        }
    }

    fn parse_atom(&mut self) -> Expr {
        let start = self.peek_span().start;
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Int(s) => {
                self.bump();
                Expr::Int(s, span)
            }
            TokenKind::Float(s) => {
                self.bump();
                Expr::Float(s, span)
            }
            TokenKind::Imaginary(s) => {
                self.bump();
                Expr::Imaginary(s, span)
            }
            TokenKind::Str { value, flags } => {
                self.bump();
                Expr::Str {
                    value,
                    raw: flags.raw,
                    bytes: flags.bytes,
                    fstring: flags.fstring,
                    span,
                    fstring_fields: Vec::new(),
                }
            }
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Expr::Bool(true, span)
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Expr::Bool(false, span)
            }
            TokenKind::Keyword(Keyword::None) => {
                self.bump();
                Expr::None(span)
            }
            TokenKind::Ident(name) => {
                self.bump();
                Expr::Name(name, span)
            }
            // Type/builtin keywords usable in value position (constructors,
            // conversions, built-in functions): list/dict/array/int64(...)/
            // range/channel/close/... `close` is reserved (SPEC §16) but is an
            // ordinary built-in function (design/0005-concurrency.md), so it
            // reads as a callable name here.
            TokenKind::Keyword(k)
                if k.is_type()
                    || k == Keyword::Range
                    || k == Keyword::Channel
                    || k == Keyword::Close =>
            {
                self.bump();
                Expr::Name(k.as_str().to_string(), span)
            }
            TokenKind::LParen => self.parse_paren_or_tuple(start),
            TokenKind::LBracket => self.parse_list(start),
            TokenKind::LBrace => self.parse_brace(start),
            _ => {
                self.error_here(ErrorCode::ZAP2014, "expected an expression");
                self.bump();
                Expr::None(span)
            }
        }
    }

    fn parse_paren_or_tuple(&mut self, start: u32) -> Expr {
        self.expect(&TokenKind::LParen, "'('");
        if self.eat(&TokenKind::RParen) {
            return Expr::Tuple(Vec::new(), self.span_from(start));
        }
        // `(name := expr)` — a named (walrus) assignment-expression (#600). The
        // walrus is only legal in a parenthesized position, so it is recognized
        // here at the head of a parenthesized group. The surrounding operators
        // apply to the whole named expression (`(n := len(xs)) > 10` parses as
        // `(n := len(xs))` then `> 10`, not `n := (len(xs) > 10)`).
        if matches!(self.peek(), TokenKind::Ident(_)) && *self.peek2() == TokenKind::ColonEq {
            let named = self.parse_named_expr();
            self.expect(&TokenKind::RParen, "')'");
            return named;
        }
        let first = self.parse_display_elem();
        // `(elem for CLAUSES)` — a bare generator expression (#596). A `for`
        // right after the first element (which must be plain, never a `*expr`
        // splat) makes this a lazy genexpr rather than a grouping paren or a
        // tuple. It shares the comprehension clause grammar (#599).
        if self.is_kw(Keyword::For) && !matches!(first, Expr::Starred { .. }) {
            let clauses = self.parse_comp_clauses();
            self.expect(&TokenKind::RParen, "')'");
            return Expr::GenExp {
                elem: Box::new(first),
                clauses,
                span: self.span_from(start),
            };
        }
        // A leading `*rest` forces a tuple display (#598): `(*rest)` cannot be a
        // grouping parenthesis.
        if self.is(&TokenKind::Comma) || matches!(first, Expr::Starred { .. }) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::RParen) {
                    break;
                }
                elems.push(self.parse_display_elem());
            }
            self.expect(&TokenKind::RParen, "')'");
            Expr::Tuple(elems, self.span_from(start))
        } else {
            self.expect(&TokenKind::RParen, "')'");
            first
        }
    }

    /// Parse a named (walrus) assignment-expression `name := expr` (#600). The
    /// caller has already confirmed the `NAME :=` head (and consumes the
    /// surrounding parentheses). `value` is a full expression; because the
    /// walrus always lives inside parentheses, the closing `)` bounds it.
    fn parse_named_expr(&mut self) -> Expr {
        let start = self.peek_span().start;
        let name_span = self.peek_span();
        let name = match self.peek().clone() {
            TokenKind::Ident(n) => {
                self.bump();
                n
            }
            _ => {
                // Unreachable via the caller's guard, but recover defensively.
                self.error_here(ErrorCode::ZAP2001, "expected a name before `:=`");
                String::new()
            }
        };
        self.expect(&TokenKind::ColonEq, "':='");
        let value = self.parse_expr();
        Expr::Named {
            name,
            name_span,
            value: Box::new(value),
            span: self.span_from(start),
        }
    }

    fn parse_list(&mut self, start: u32) -> Expr {
        self.expect(&TokenKind::LBracket, "'['");
        if self.eat(&TokenKind::RBracket) {
            return Expr::List(Vec::new(), self.span_from(start));
        }
        let first = self.parse_display_elem();
        // A comprehension needs a plain first element; `[*xs for ...]` is not a
        // comprehension (#598/#599).
        if self.is_kw(Keyword::For) && !matches!(first, Expr::Starred { .. }) {
            let clauses = self.parse_comp_clauses();
            self.expect(&TokenKind::RBracket, "']'");
            return Expr::ListComp {
                elem: Box::new(first),
                clauses,
                span: self.span_from(start),
            };
        }
        let mut elems = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.is(&TokenKind::RBracket) {
                break;
            }
            elems.push(self.parse_display_elem());
        }
        self.expect(&TokenKind::RBracket, "']'");
        Expr::List(elems, self.span_from(start))
    }

    /// Parse one element of a list/set/tuple display, which may be a `*expr`
    /// splat (#598) that splices an iterable's elements in place.
    fn parse_display_elem(&mut self) -> Expr {
        if self.is(&TokenKind::Star) {
            let start = self.peek_span().start;
            self.bump(); // '*'
            let value = self.parse_expr();
            Expr::Starred {
                value: Box::new(value),
                span: self.span_from(start),
            }
        } else {
            self.parse_expr()
        }
    }

    /// A brace literal: a **dict** when it has `key: value` pairs, a **set** when
    /// it has comma-separated elements and no colon. Empty `{}` stays a dict
    /// (the conventional default; an empty set has no literal form yet).
    fn parse_brace(&mut self, start: u32) -> Expr {
        self.expect(&TokenKind::LBrace, "'{'");
        if self.eat(&TokenKind::RBrace) {
            return Expr::Dict(Vec::new(), self.span_from(start));
        }
        // `**expr` unambiguously begins a dict display: a merge entry (#598).
        if self.is(&TokenKind::StarStar) {
            let first = self.parse_dict_item();
            return self.parse_dict_items_rest(start, first);
        }
        // `*expr` unambiguously begins a set display: a splat element (#598).
        if self.is(&TokenKind::Star) {
            let first = self.parse_display_elem();
            return self.parse_set_rest(start, first);
        }
        let first = self.parse_expr();
        // A `:` after the first element means this is a dict; anything else
        // (`,`, `}`, or a `for`) means a set.
        if self.is(&TokenKind::Colon) {
            return self.parse_dict_rest(start, first);
        }
        self.parse_set_rest(start, first)
    }

    /// Parse one dict display entry: `**expr` (a merge, #598) or `key: value`.
    fn parse_dict_item(&mut self) -> DictItem {
        if self.eat(&TokenKind::StarStar) {
            DictItem::Spread(self.parse_expr())
        } else {
            let k = self.parse_expr();
            self.expect(&TokenKind::Colon, "':'");
            let v = self.parse_expr();
            DictItem::Pair(k, v)
        }
    }

    /// Finish a dict literal (or dict comprehension) whose first key is `first`.
    fn parse_dict_rest(&mut self, start: u32, first_key: Expr) -> Expr {
        self.expect(&TokenKind::Colon, "':'");
        let first_val = self.parse_expr();
        if self.is_kw(Keyword::For) {
            let clauses = self.parse_comp_clauses();
            self.expect(&TokenKind::RBrace, "'}'");
            return Expr::DictComp {
                key: Box::new(first_key),
                value: Box::new(first_val),
                clauses,
                span: self.span_from(start),
            };
        }
        self.parse_dict_items_rest(start, DictItem::Pair(first_key, first_val))
    }

    /// Finish a dict literal given its first entry; reads the remaining
    /// comma-separated entries (`key: value` or `**expr`).
    fn parse_dict_items_rest(&mut self, start: u32, first: DictItem) -> Expr {
        let mut items = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.is(&TokenKind::RBrace) {
                break;
            }
            items.push(self.parse_dict_item());
        }
        self.expect(&TokenKind::RBrace, "'}'");
        Expr::Dict(items, self.span_from(start))
    }

    /// Finish a set literal (or set comprehension) whose first element is `first`.
    fn parse_set_rest(&mut self, start: u32, first: Expr) -> Expr {
        // A comprehension needs a plain first element; `{*xs for ...}` is not a
        // comprehension (#598/#599).
        if self.is_kw(Keyword::For) && !matches!(first, Expr::Starred { .. }) {
            let clauses = self.parse_comp_clauses();
            self.expect(&TokenKind::RBrace, "'}'");
            return Expr::SetComp {
                elem: Box::new(first),
                clauses,
                span: self.span_from(start),
            };
        }
        let mut elems = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.is(&TokenKind::RBrace) {
                break;
            }
            elems.push(self.parse_display_elem());
        }
        self.expect(&TokenKind::RBrace, "'}'");
        Expr::Set(elems, self.span_from(start))
    }

    /// Parse the clause list of a comprehension: one or more `for TARGET in
    /// ITER` clauses, each optionally followed by one or more `if COND` filters
    /// (#599). The leading `for` must already be the next token. Iterables and
    /// filters are parsed without a trailing ternary (so `if` reads as a filter,
    /// not a conditional expression).
    fn parse_comp_clauses(&mut self) -> Vec<CompClause> {
        let mut clauses = Vec::new();
        while self.is_kw(Keyword::For) {
            let cstart = self.peek_span().start;
            self.expect_kw(Keyword::For, "'for'");
            let var = self.ident("comprehension variable");
            self.expect_kw(Keyword::In, "'in'");
            let iter = self.parse_or_expr();
            let mut ifs = Vec::new();
            while self.eat_kw(Keyword::If) {
                ifs.push(self.parse_or_expr());
            }
            clauses.push(CompClause {
                var,
                iter: Box::new(iter),
                ifs,
                span: self.span_from(cstart),
            });
        }
        clauses
    }

    // --- patterns (match-case) ---

    /// A full pattern: an or-pattern with an optional `as name` capture suffix
    /// (`pattern ::= or_pattern [ "as" IDENT ]`, D11/#603). The `as` binds the
    /// whole matched value and wraps the entire or-pattern, so `1 | 2 as n`
    /// parses as `(1 | 2) as n`.
    fn parse_pattern(&mut self) -> Pattern {
        let start = self.peek_span().start;
        let inner = self.parse_or_pattern();
        if self.eat_kw(Keyword::As) {
            let name = self.pattern_capture_name();
            Pattern::As {
                pattern: Box::new(inner),
                name,
                span: self.span_from(start),
            }
        } else {
            inner
        }
    }

    /// `closed_pattern ("|" closed_pattern)*` — the alternatives of an
    /// or-pattern, without the trailing `as` capture (that is handled one level
    /// up in [`parse_pattern`]).
    fn parse_or_pattern(&mut self) -> Pattern {
        let first = self.parse_closed_pattern();
        if self.is(&TokenKind::Pipe) {
            let start = self.peek_span().start;
            let mut alts = vec![first];
            while self.eat(&TokenKind::Pipe) {
                alts.push(self.parse_closed_pattern());
            }
            Pattern::Or(alts, self.span_from(start))
        } else {
            first
        }
    }

    /// The binding name after `as` in a pattern. It must be a single plain
    /// identifier: `_` (wildcard), a dotted/qualified name, or any other token
    /// is rejected with [`ErrorCode::ZAP2021`] (D11/#603).
    fn pattern_capture_name(&mut self) -> String {
        match self.ident_opt() {
            Some((name, _)) if name == "_" => {
                self.error_here(
                    ErrorCode::ZAP2021,
                    "`as` capture name cannot be `_`; drop the `as` to discard the value",
                );
                "<error>".to_string()
            }
            Some((name, _)) => {
                // A dotted name (`as a.b`) is not a binding target.
                if self.is(&TokenKind::Dot) {
                    self.error_here(
                        ErrorCode::ZAP2021,
                        "`as` capture must be a single plain name, not a dotted name",
                    );
                    while self.eat(&TokenKind::Dot) {
                        self.ident_opt();
                    }
                }
                name
            }
            None => {
                self.error_here(
                    ErrorCode::ZAP2021,
                    "expected a binding name after `as` in the pattern",
                );
                "<error>".to_string()
            }
        }
    }

    fn parse_closed_pattern(&mut self) -> Pattern {
        let start = self.peek_span().start;
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                Pattern::Wildcard(span)
            }
            TokenKind::Ident(name) => {
                self.bump();
                // A dotted/qualified name: `Color.RED`, `Shape.Circle`, ...
                // (D-0018/D-0046). Enum members/variants are matched qualified.
                let mut path = vec![name.clone()];
                while self.eat(&TokenKind::Dot) {
                    path.push(self.member_name("qualified pattern name"));
                }
                // Struct / enum-variant pattern: `Path(pat_args)`. Arguments are
                // positional (`r`) or by name (`h=h`), developer's choice
                // (D-0040/D-0046).
                if self.is(&TokenKind::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    if !self.is(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_pat_arg());
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(&TokenKind::RParen, "')'");
                    Pattern::Struct {
                        path,
                        args: Some(args),
                        span: self.span_from(start),
                    }
                } else if path.len() == 1 {
                    // A bare name binds (capture, Python semantics, D-0040).
                    Pattern::Capture { name, span }
                } else {
                    // A qualified constant or payload-free variant with no
                    // parens: `Color.RED`, `Shape.Unit`.
                    Pattern::Struct {
                        path,
                        args: None,
                        span: self.span_from(start),
                    }
                }
            }
            TokenKind::Star => {
                self.bump();
                let name = self.ident_opt().map(|(n, _)| n);
                Pattern::Rest {
                    name,
                    span: self.span_from(start),
                }
            }
            TokenKind::LBracket => {
                self.bump();
                let mut pats = Vec::new();
                if !self.is(&TokenKind::RBracket) {
                    loop {
                        pats.push(self.parse_pattern());
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RBracket, "']'");
                Pattern::Sequence(pats, self.span_from(start))
            }
            TokenKind::LParen => {
                self.bump();
                let mut pats = Vec::new();
                if !self.is(&TokenKind::RParen) {
                    loop {
                        pats.push(self.parse_pattern());
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen, "')'");
                Pattern::Tuple(pats, self.span_from(start))
            }
            _ => {
                // Literal pattern (numbers, strings, True/False/None).
                let e = self.parse_operand();
                Pattern::Literal(e)
            }
        }
    }

    /// One argument of a struct/variant pattern: `IDENT "=" pattern` (by name)
    /// or `pattern` (positional, declaration order; D-0040/D-0046).
    fn parse_pat_arg(&mut self) -> PatArg {
        let start = self.peek_span().start;
        let named =
            matches!(self.peek(), TokenKind::Ident(_)) && matches!(self.peek2(), TokenKind::Assign);
        if named {
            let name = self.ident("field name");
            self.expect(&TokenKind::Assign, "'='");
            let pattern = self.parse_pattern();
            PatArg {
                name: Some(name),
                pattern,
                span: self.span_from(start),
            }
        } else {
            let pattern = self.parse_pattern();
            PatArg {
                name: None,
                pattern,
                span: self.span_from(start),
            }
        }
    }
}

/// Convert a parsed expression used as an assignment target into a
/// [`BindTarget`] (D-0049). A tuple expression destructures positionally; any
/// other expression is a single lvalue leaf. (`*rest` cannot appear in a parsed
/// expression, so parenthesized rest is only reachable via a declaration's
/// `parse_bind_target`.)
fn expr_to_bind_target(e: Expr) -> BindTarget {
    match e {
        Expr::Tuple(elems, span) => {
            BindTarget::Tuple(elems.into_iter().map(expr_to_bind_target).collect(), span)
        }
        other => BindTarget::Elem(other),
    }
}

/// Reconstruct a tuple expression from a bind target that turned out not to be
/// an assignment (a bare tuple expression statement, e.g. `a, b`).
fn bind_target_to_expr(bt: BindTarget) -> Expr {
    match bt {
        BindTarget::Elem(e) => e,
        BindTarget::Tuple(elems, span) => {
            Expr::Tuple(elems.into_iter().map(bind_target_to_expr).collect(), span)
        }
        // A `*rest` in expression position is not representable; fall back to the
        // inner expression (this only arises from malformed input).
        BindTarget::Rest(inner, _) => bind_target_to_expr(*inner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Module {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        module
    }

    #[test]
    fn empty_module() {
        let m = parse_ok("");
        assert!(m.items.is_empty());
    }

    fn interface_of(m: &Module) -> &InterfaceDef {
        m.items
            .iter()
            .find_map(|i| match i {
                Item::Interface(x) => Some(x),
                _ => None,
            })
            .expect("an interface item")
    }

    fn supertype_names(i: &InterfaceDef) -> Vec<String> {
        i.supertypes
            .iter()
            .map(|t| match t {
                Type::Named { name, .. } => name.clone(),
                _ => "<non-named>".to_string(),
            })
            .collect()
    }

    #[test]
    fn interface_without_supertypes_parses_empty_list() {
        // A plain interface (the original form) carries no supertypes.
        let m = parse_ok("interface Equatable:\n    def eq(self) -> bool\n");
        let i = interface_of(&m);
        assert!(i.supertypes.is_empty());
        assert_eq!(i.methods.len(), 1);
    }

    #[test]
    fn interface_with_single_supertype_parses() {
        // `interface Hashable: Equatable` records one supertype and still parses
        // its method block (D-0139/#806).
        let m = parse_ok("interface Hashable: Equatable\n    def hash(self) -> int64\n");
        let i = interface_of(&m);
        assert_eq!(supertype_names(i), vec!["Equatable".to_string()]);
        assert_eq!(i.methods.len(), 1);
        assert_eq!(i.methods[0].name, "hash");
    }

    #[test]
    fn interface_with_multiple_supertypes_parses() {
        // A comma-separated supertype list is recorded in order.
        let m = parse_ok("interface C: A, B\n    def c(self) -> int64\n");
        let i = interface_of(&m);
        assert_eq!(supertype_names(i), vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn interface_with_supertype_and_empty_body_parses() {
        // Refinement composes with a body that is only `pass`-free but present;
        // the header `:` + supertype + NEWLINE + INDENT block round-trips.
        let m = parse_ok(
            "interface A:\n    def a(self) -> int64\n\ninterface B: A\n    def b(self) -> int64\n",
        );
        // The second interface is the refining one.
        let b = m
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Interface(x) => Some(x),
                _ => None,
            })
            .find(|x| x.name == "B")
            .expect("interface B");
        assert_eq!(supertype_names(b), vec!["A".to_string()]);
    }

    #[test]
    fn wrap_decorator_member_parses_structure() {
        // The `wrap(call)` form (#653): the member captures the `call` macro
        // param, the nested `def inner(*args)` signature + body, and the
        // `return inner` trailer.
        let m = parse_ok(
            "decorator retry(int64 times):\n    def wrap(call):\n        def inner(*args):\n            return call(*args)\n        return inner\n",
        );
        let Item::Decorator(d) = &m.items[0] else {
            panic!("expected decorator");
        };
        assert_eq!(d.members.len(), 1);
        let mem = &d.members[0];
        assert_eq!(mem.name, "wrap");
        assert_eq!(mem.params, vec!["call".to_string()]);
        let w = mem.wrap.as_ref().expect("wrap form captured");
        assert_eq!(w.inner_name, "inner");
        assert_eq!(w.inner_params.len(), 1);
        assert_eq!(w.inner_params[0].name, "args");
        assert!(w.inner_params[0].variadic);
        assert!(!w.inner_params[0].kwargs);
        // The inner body (the `return call(*args)`) lives on `body`.
        assert_eq!(mem.body.as_ref().expect("inner body").stmts.len(), 1);
        // The trailer is exactly `return inner`.
        assert_eq!(w.trailer.len(), 1);
        assert!(matches!(
            &w.trailer[0],
            Stmt::Return { value: Some(Expr::Name(n, _)), .. } if n == "inner"
        ));
    }

    #[test]
    fn before_after_member_has_no_wrap_form() {
        let m = parse_ok("decorator traced:\n    def before(ctx):\n        print(ctx.name)\n");
        let Item::Decorator(d) = &m.items[0] else {
            panic!("expected decorator");
        };
        assert!(d.members[0].wrap.is_none(), "before/after has no wrap form");
        assert!(d.members[0].body.is_some());
    }

    #[test]
    fn fib_fixture_parses() {
        let src = include_str!("../../../tests/golden/fib.zap");
        let m = parse_ok(src);
        assert_eq!(m.items.len(), 2);
        match &m.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "fib");
                assert_eq!(f.params.len(), 1);
                assert_eq!(f.params[0].name, "n");
                assert!(matches!(&f.ret, Some(Type::Named { name, .. }) if name == "int64"));
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn assert_captures_condition_source() {
        let m = parse_ok("def main():\n    assert x + 1 == y, \"msg\"\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        match &f.body.stmts[0] {
            Stmt::Assert { cond_src, msg, .. } => {
                assert_eq!(cond_src, "x + 1 == y");
                assert!(msg.is_some(), "message should be parsed");
            }
            other => panic!("expected an assert, got {other:?}"),
        }
    }

    #[test]
    fn assert_without_message_parses() {
        let m = parse_ok("def main():\n    assert ok\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        assert!(matches!(&f.body.stmts[0], Stmt::Assert { msg: None, .. }));
    }

    #[test]
    fn scope_block_and_spawn_detached_parse() {
        // `scope(...)` parses as a compound statement owning a `spawn`; the inner
        // `spawn` is not detached, while `spawn detached` sets the modifier and
        // `spawn detached()` (a call to a fn literally named `detached`) does not
        // (ticket #50, D-0044).
        let m = parse_ok(
            "def main():\n    scope(on_error=OnError.Wait):\n        spawn f()\n        spawn detached g()\n    spawn detached h()\n    spawn detached()\n",
        );
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let stmts = &f.body.stmts;
        match &stmts[0] {
            Stmt::Scope { opts, body, .. } => {
                assert_eq!(opts.len(), 1);
                assert_eq!(opts[0].name.as_deref(), Some("on_error"));
                assert!(matches!(
                    &body.stmts[0],
                    Stmt::Spawn {
                        detached: false,
                        ..
                    }
                ));
                assert!(matches!(&body.stmts[1], Stmt::Spawn { detached: true, .. }));
            }
            other => panic!("expected a scope statement, got {other:?}"),
        }
        assert!(matches!(&stmts[1], Stmt::Spawn { detached: true, .. }));
        // `spawn detached()` — `detached` is the callee, not the modifier.
        assert!(matches!(
            &stmts[2],
            Stmt::Spawn {
                detached: false,
                ..
            }
        ));
    }

    #[test]
    fn loop_sum_fixture_parses() {
        let src = include_str!("../../../tests/golden/loop_sum.zap");
        let m = parse_ok(src);
        assert_eq!(m.items.len(), 1);
        match &m.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "main");
                // auto total = 0 ; for ...: ... ; print(total)
                assert!(f.body.stmts.len() >= 3);
                assert!(matches!(f.body.stmts[0], Stmt::Let { .. }));
                assert!(matches!(f.body.stmts[1], Stmt::For { .. }));
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn declaration_forms() {
        let m = parse_ok("int64 count = 0\nauto name = \"zap\"\nconst f64 PI = 3.14\n");
        assert_eq!(m.items.len(), 3);
        assert!(matches!(
            &m.items[0],
            Item::Stmt(Stmt::Let {
                ty: Some(_),
                is_const: false,
                ..
            })
        ));
        assert!(matches!(
            &m.items[1],
            Item::Stmt(Stmt::Let { ty: None, .. })
        ));
        assert!(matches!(
            &m.items[2],
            Item::Stmt(Stmt::Let { is_const: true, .. })
        ));
    }

    #[test]
    fn assignment_vs_declaration() {
        // `x = 1` is an assignment, not a declaration.
        let m = parse_ok("x = 1\nx.y = 2\narr[0] = 3\n");
        assert!(matches!(&m.items[0], Item::Stmt(Stmt::Assign { .. })));
        assert!(matches!(&m.items[1], Item::Stmt(Stmt::Assign { .. })));
        assert!(matches!(&m.items[2], Item::Stmt(Stmt::Assign { .. })));
    }

    #[test]
    fn precedence_mul_over_add() {
        // 1 + 2 * 3  ->  1 + (2 * 3)
        let m = parse_ok("x = 1 + 2 * 3\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Binary { op, rhs, .. } = value else {
            panic!("binary");
        };
        assert_eq!(*op, BinOp::Add);
        assert!(matches!(rhs.as_ref(), Expr::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn chained_comparison_builds_compare_node() {
        // `a < b < c` is a Python-chained comparison, NOT left-associative
        // `(a < b) < c` (I#386): the parser emits a single `Compare`.
        let m = parse_ok("x = a < b < c\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Compare { first, rest, .. } = value else {
            panic!("expected Compare, got {value:?}");
        };
        assert!(matches!(first.as_ref(), Expr::Name(n, _) if n == "a"));
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].0, BinOp::Lt);
        assert_eq!(rest[1].0, BinOp::Lt);
        assert!(matches!(&rest[0].1, Expr::Name(n, _) if n == "b"));
        assert!(matches!(&rest[1].1, Expr::Name(n, _) if n == "c"));
    }

    #[test]
    fn chained_comparison_mixed_ops_and_length() {
        // A four-operand chain with mixed operators: `1 < a <= b == c`.
        let m = parse_ok("x = 1 < a <= b == c\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Compare { rest, .. } = value else {
            panic!("expected Compare, got {value:?}");
        };
        let ops: Vec<BinOp> = rest.iter().map(|(op, _)| *op).collect();
        assert_eq!(ops, vec![BinOp::Lt, BinOp::Le, BinOp::Eq]);
    }

    #[test]
    fn single_comparison_stays_binary() {
        // A lone comparison is unchanged — still an `Expr::Binary`.
        let m = parse_ok("x = a < b\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        assert!(matches!(value, Expr::Binary { op: BinOp::Lt, .. }));
    }

    #[test]
    fn parenthesized_comparison_is_not_chained() {
        // Explicit parens defeat chaining: `(a < b) < c` really does compare a
        // bool against `c`, so it is nested `Binary`, not a `Compare`.
        let m = parse_ok("x = (a < b) < c\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Binary { op, lhs, .. } = value else {
            panic!("expected Binary, got {value:?}");
        };
        assert_eq!(*op, BinOp::Lt);
        assert!(matches!(lhs.as_ref(), Expr::Binary { op: BinOp::Lt, .. }));
    }

    #[test]
    fn is_operator_parses_as_binary() {
        // `a is b` is a lone comparison -> `Expr::Binary` with `BinOp::Is`.
        let m = parse_ok("x = a is b\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Binary { op, lhs, rhs, .. } = value else {
            panic!("expected Binary, got {value:?}");
        };
        assert_eq!(*op, BinOp::Is);
        assert!(matches!(lhs.as_ref(), Expr::Name(n, _) if n == "a"));
        assert!(matches!(rhs.as_ref(), Expr::Name(n, _) if n == "b"));
    }

    #[test]
    fn is_not_operator_parses_as_one_binary() {
        // The two-token `is not` is a single `BinOp::IsNot`, not `is` applied to
        // a `not`-prefixed operand. `x is not None` -> `IsNot(x, None)`.
        let m = parse_ok("x = v is not None\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Binary { op, lhs, rhs, .. } = value else {
            panic!("expected Binary, got {value:?}");
        };
        assert_eq!(*op, BinOp::IsNot);
        assert!(matches!(lhs.as_ref(), Expr::Name(n, _) if n == "v"));
        assert!(matches!(rhs.as_ref(), Expr::None(_)));
    }

    #[test]
    fn is_chains_like_other_comparisons() {
        // `a is b is c` is a Python-chained comparison (I#386), NOT the
        // left-associative `(a is b) is c`: the parser emits a single `Compare`
        // with two `Is` links.
        let m = parse_ok("x = a is b is c\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Compare { first, rest, .. } = value else {
            panic!("expected Compare, got {value:?}");
        };
        assert!(matches!(first.as_ref(), Expr::Name(n, _) if n == "a"));
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].0, BinOp::Is);
        assert_eq!(rest[1].0, BinOp::Is);
    }

    #[test]
    fn is_mixes_in_a_comparison_chain() {
        // `is`/`is not` share the comparison precedence level, so they chain
        // alongside `==` etc.: `a is b == c` and `x is not None == y` each build
        // one `Compare` carrying the identity and equality links in order.
        let m = parse_ok("r = a is b == c\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Compare { rest, .. } = value else {
            panic!("expected Compare, got {value:?}");
        };
        let ops: Vec<BinOp> = rest.iter().map(|(op, _)| *op).collect();
        assert_eq!(ops, vec![BinOp::Is, BinOp::Eq]);

        let m = parse_ok("r = x is not None == y\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Compare { rest, .. } = value else {
            panic!("expected Compare, got {value:?}");
        };
        let ops: Vec<BinOp> = rest.iter().map(|(op, _)| *op).collect();
        assert_eq!(ops, vec![BinOp::IsNot, BinOp::Eq]);
    }

    #[test]
    fn is_binds_at_comparison_precedence() {
        // `is` sits at comparison precedence, below `+`: `a + b is c` parses as
        // `(a + b) is c`, i.e. the left operand of `is` is the whole `a + b`.
        let m = parse_ok("r = a + b is c\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Binary { op, lhs, .. } = value else {
            panic!("expected Binary, got {value:?}");
        };
        assert_eq!(*op, BinOp::Is);
        assert!(matches!(lhs.as_ref(), Expr::Binary { op: BinOp::Add, .. }));
    }

    #[test]
    fn power_is_right_associative() {
        // 2 ** 3 ** 2 -> 2 ** (3 ** 2)
        let m = parse_ok("x = 2 ** 3 ** 2\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::Binary { op, rhs, .. } = value else {
            panic!();
        };
        assert_eq!(*op, BinOp::Pow);
        assert!(matches!(rhs.as_ref(), Expr::Binary { op: BinOp::Pow, .. }));
    }

    #[test]
    fn aug_assign_and_call() {
        let m = parse_ok("total += f(x, y=2)\n");
        let Item::Stmt(Stmt::AugAssign { op, value, .. }) = &m.items[0] else {
            panic!("aug assign");
        };
        assert_eq!(*op, BinOp::Add);
        let Expr::Call { args, .. } = value else {
            panic!("call");
        };
        assert_eq!(args.len(), 2);
        assert_eq!(args[1].name.as_deref(), Some("y"));
    }

    #[test]
    fn comprehension_filter_not_ternary() {
        let m = parse_ok("xs = [x * x for x in range(10) if x > 1]\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(
            value,
            Expr::ListComp { clauses, .. } if clauses.len() == 1 && clauses[0].ifs.len() == 1
        ));
    }

    #[test]
    fn comprehension_multiple_for_and_if_clauses() {
        // Two `for` clauses with a trailing filter (#599).
        let m = parse_ok("xs = [x * y for x in a for y in b if x != y]\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::ListComp { clauses, .. } = value else {
            panic!("expected a list comprehension");
        };
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].var, "x");
        assert!(clauses[0].ifs.is_empty());
        assert_eq!(clauses[1].var, "y");
        assert_eq!(clauses[1].ifs.len(), 1);

        // A filter on the first clause, then a second `for`: the `if` attaches
        // to the clause it follows.
        let m = parse_ok("xs = [x for x in a if x > 0 for y in b]\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::ListComp { clauses, .. } = value else {
            panic!("expected a list comprehension");
        };
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].ifs.len(), 1);
        assert!(clauses[1].ifs.is_empty());
    }

    #[test]
    fn genexpr_parses() {
        // A bare generator expression `(x for x in xs)` (#596) — distinct from a
        // parenthesized group and a tuple.
        let m = parse_ok("xs = (x * x for x in a)\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::GenExp { elem, clauses, .. } = value else {
            panic!("expected a generator expression, got {value:?}");
        };
        assert!(matches!(elem.as_ref(), Expr::Binary { .. }));
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].var, "x");
        assert!(clauses[0].ifs.is_empty());
    }

    #[test]
    fn genexpr_multi_clause_parses() {
        // Multiple `for`/`if` clauses in a genexpr, same grammar as a
        // comprehension (#596 pairs with #599).
        let m = parse_ok("xs = (x for x in a if x > 0 for y in b)\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::GenExp { clauses, .. } = value else {
            panic!("expected a generator expression");
        };
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].var, "x");
        assert_eq!(clauses[0].ifs.len(), 1);
        assert_eq!(clauses[1].var, "y");
        assert!(clauses[1].ifs.is_empty());
    }

    #[test]
    fn paren_group_is_not_a_genexpr() {
        // A parenthesized expression with no `for` stays a grouping paren.
        let m = parse_ok("xs = (a + b)\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(
            matches!(value, Expr::Binary { .. }),
            "a plain parenthesized expression must not become a genexpr, got {value:?}"
        );
    }

    #[test]
    fn paren_tuple_is_not_a_genexpr() {
        // A comma-separated parenthesized list stays a tuple.
        let m = parse_ok("xs = (a, b)\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(
            matches!(value, Expr::Tuple(elems, _) if elems.len() == 2),
            "a parenthesized tuple must not become a genexpr, got {value:?}"
        );
    }

    #[test]
    fn genexpr_as_sole_call_argument_needs_no_parens() {
        // `sum(x for x in a)` — the call's own parentheses bound the genexpr
        // (#596). The single argument is itself a `GenExp`.
        let m = parse_ok("xs = sum(x * x for x in a)\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::Call { args, .. } = value else {
            panic!("expected a call");
        };
        assert_eq!(args.len(), 1);
        assert!(
            matches!(&args[0].value, Expr::GenExp { clauses, .. } if clauses.len() == 1),
            "the sole call argument must parse as a genexpr, got {:?}",
            args[0].value
        );
    }

    #[test]
    fn named_expr_parses_in_condition_and_comprehension() {
        // `while` condition: `(line := read()) != ""` is `Binary(Ne, Named, Str)`.
        let m = parse_ok("def main():\n    while (line := read()) != \"\":\n        print(line)\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let Stmt::While { cond, .. } = &f.body.stmts[0] else {
            panic!("expected while");
        };
        let Expr::Binary {
            op: BinOp::Ne, lhs, ..
        } = cond
        else {
            panic!("expected `!=` at the condition top");
        };
        let Expr::Named { name, value, .. } = lhs.as_ref() else {
            panic!("expected a walrus on the left of `!=`");
        };
        assert_eq!(name, "line");
        assert!(matches!(value.as_ref(), Expr::Call { .. }));

        // `if` condition: `(n := len(xs)) > 10`.
        let m = parse_ok("def main():\n    if (n := len(xs)) > 10:\n        print(n)\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let Stmt::If { branches, .. } = &f.body.stmts[0] else {
            panic!("expected if");
        };
        let Expr::Binary {
            op: BinOp::Gt, lhs, ..
        } = &branches[0].0
        else {
            panic!("expected `>` at the condition top");
        };
        assert!(matches!(lhs.as_ref(), Expr::Named { name, .. } if name == "n"));

        // Comprehension filter: `[y for x in xs if (y := f(x)) > 0]`.
        let m = parse_ok("ys = [y for x in xs if (y := f(x)) > 0]\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("expected assignment");
        };
        let Expr::ListComp { clauses, .. } = value else {
            panic!("expected a list comprehension");
        };
        assert_eq!(clauses[0].ifs.len(), 1);
        let Expr::Binary { lhs, .. } = &clauses[0].ifs[0] else {
            panic!("expected a comparison filter");
        };
        assert!(matches!(lhs.as_ref(), Expr::Named { name, .. } if name == "y"));

        // A bare parenthesized named expression is a valid expression statement.
        let m = parse_ok("def main():\n    (total := 0)\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        assert!(matches!(&f.body.stmts[0], Stmt::Expr(Expr::Named { .. })));
    }

    #[test]
    fn bare_walrus_statement_is_rejected_steering_to_auto() {
        // `a := 10` as a bare statement is not a declaration shorthand (#600
        // non-goal); it is rejected with ZAP2020, which steers to `auto`.
        let (_, perrs, lerrs) = parse_str("def main():\n    a := 10\n");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2020),
            "expected ZAP2020 (steer to `auto`), got {perrs:?}"
        );
        assert!(
            perrs
                .iter()
                .any(|e| e.code == ErrorCode::ZAP2020 && e.message.contains("auto a =")),
            "diagnostic should suggest `auto a = ...`, got {perrs:?}"
        );
    }

    #[test]
    fn starred_display_elements_parse() {
        // `*expr` in a list display (#598).
        let m = parse_ok("xs = [1, *a, 2, *b]\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::List(elems, _) = value else {
            panic!("expected a list display");
        };
        assert_eq!(elems.len(), 4);
        assert!(matches!(elems[1], Expr::Starred { .. }));
        assert!(matches!(elems[3], Expr::Starred { .. }));

        // `*expr` in a set display, and `(x, *rest)` in a tuple display.
        let m = parse_ok("s = {*a, *b}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(
            matches!(value, Expr::Set(e, _) if e.len() == 2 && e.iter().all(|x| matches!(x, Expr::Starred { .. })))
        );

        let m = parse_ok("t = (x, *rest)\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(
            matches!(value, Expr::Tuple(e, _) if e.len() == 2 && matches!(e[1], Expr::Starred { .. }))
        );
    }

    #[test]
    fn dict_display_double_star_merge_parses() {
        // `**expr` merges in a dict display (#598), mixed with pairs.
        let m = parse_ok("d = {\"a\": 1, **other, \"b\": 2}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::Dict(items, _) = value else {
            panic!("expected a dict display");
        };
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], DictItem::Pair(..)));
        assert!(matches!(items[1], DictItem::Spread(_)));
        assert!(matches!(items[2], DictItem::Pair(..)));

        // A brace starting with `**` is a dict, not a set.
        let m = parse_ok("d = {**other}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(
            matches!(value, Expr::Dict(items, _) if items.len() == 1 && matches!(items[0], DictItem::Spread(_)))
        );
    }

    #[test]
    fn brace_literal_set_vs_dict() {
        // Comma-separated, no colon -> set.
        let m = parse_ok("s = {1, 2, 3}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::Set(elems, _) if elems.len() == 3));
        // Key: value pairs -> dict.
        let m = parse_ok("d = {1: 2, 3: 4}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::Dict(items, _) if items.len() == 2));
        // Empty braces stay a dict.
        let m = parse_ok("e = {}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::Dict(items, _) if items.is_empty()));
        // A `for` with no colon -> set comprehension.
        let m = parse_ok("g = {x for x in range(3)}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::SetComp { .. }));
    }

    #[test]
    fn ternary_expression() {
        let m = parse_ok("y = a if c else b\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::IfElse { .. }));
    }

    #[test]
    fn if_elif_else() {
        let src = "def f(int64 x):\n    if x > 0:\n        return 1\n    elif x == 0:\n        return 0\n    else:\n        return -1\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::If {
            branches,
            else_block,
            ..
        } = &f.body.stmts[0]
        else {
            panic!("if");
        };
        assert_eq!(branches.len(), 2);
        assert!(else_block.is_some());
    }

    #[test]
    fn do_while_and_match() {
        let src = "def f():\n    do:\n        step()\n    while go\n    match x:\n        case 0:\n            a()\n        case _:\n            b()\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(f.body.stmts[0], Stmt::DoWhile { .. }));
        let Stmt::Match { arms, .. } = &f.body.stmts[1] else {
            panic!("match");
        };
        assert_eq!(arms.len(), 2);
        assert!(matches!(arms[1].pattern, Pattern::Wildcard(_)));
    }

    #[test]
    fn struct_with_fields_and_method() {
        let src = "struct Point:\n    f64 x\n    f64 y\n    def norm(self) -> f64:\n        return self.x\n";
        let m = parse_ok(src);
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert_eq!(s.name, "Point");
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.methods.len(), 1);
        assert_eq!(s.methods[0].params[0].name, "self");
        assert!(!s.is_value, "a plain struct is a reference type (D-0023)");
    }

    #[test]
    fn value_struct_modifier_parses() {
        // The contextual `value` modifier (D-0051) sets the flag; `value` stays a
        // usable identifier elsewhere.
        let m = parse_ok("value struct Vec2:\n    f64 x\n    f64 y\n");
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert_eq!(s.name, "Vec2");
        assert!(s.is_value, "`value struct` should set is_value");

        // `value` not immediately before `struct` is an ordinary identifier.
        let m2 = parse_ok("def f():\n    auto value = 3\n    print(value)\n");
        assert!(matches!(&m2.items[0], Item::Function(_)));
    }

    #[test]
    fn bitfield_width_parses() {
        // A C-style bit width `: N` after the field name (I#173, D-0068) is
        // captured in `Field::bits`; a width-less field stays `None`.
        let m =
            parse_ok("value struct Flags:\n    int64 kind : 3\n    int64 level : 5\n    bool on\n");
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert_eq!(s.fields[0].bits, Some(3));
        assert_eq!(s.fields[1].bits, Some(5));
        assert_eq!(s.fields[2].bits, None);
    }

    #[test]
    fn value_struct_with_decorator_parses() {
        // Decorators precede the `value` modifier (`decorators [ "value" ]
        // "struct"`, grammar).
        let m = parse_ok("@derive(Eq)\nvalue struct Key:\n    int64 a\n");
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert!(s.is_value);
        assert_eq!(s.decorators.len(), 1);
    }

    #[test]
    fn generic_declarations_parse_with_params_and_bounds() {
        // Type params on struct/enum/fn, and an optional interface bound that is
        // parsed and carried now (I#154 Fork B); enforcement is deferred.
        let m = parse_ok(
            "struct Box[T]:\n    T value\nenum Optional[T]:\n    Some(T value)\n    None\n\
             def max_of[T: Ordered, U](T a, U b) -> T:\n    return a\n",
        );
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert_eq!(s.generics.len(), 1);
        assert_eq!(s.generics[0].name, "T");
        assert!(s.generics[0].bound.is_none());
        let Item::Enum(e) = &m.items[1] else {
            panic!("enum");
        };
        assert_eq!(e.generics.len(), 1);
        assert_eq!(e.generics[0].name, "T");
        assert_eq!(e.variants.len(), 2);
        let Item::Function(f) = &m.items[2] else {
            panic!("fn");
        };
        assert_eq!(f.generics.len(), 2);
        assert_eq!(f.generics[0].name, "T");
        // `T: Ordered` carries the bound.
        assert!(
            matches!(&f.generics[0].bound, Some(Type::Named { name, .. }) if name == "Ordered")
        );
        assert!(f.generics[1].bound.is_none());
    }

    #[test]
    fn type_alias_plain_parses() {
        // `type Name = <type>` (#606): a transparent alias item with no generics.
        let m = parse_ok("type Row = (int64, str)\n");
        let Item::TypeAlias(a) = &m.items[0] else {
            panic!("expected a type alias, got {:?}", m.items[0]);
        };
        assert_eq!(a.name, "Row");
        assert!(a.generics.is_empty());
        assert!(matches!(&a.target, Type::Tuple { elems, .. } if elems.len() == 2));
    }

    #[test]
    fn type_alias_generic_parses() {
        // The generic form `type Name[T] = <type>` (#606) carries the type
        // parameters, which the target may reference.
        let m = parse_ok("type Pair[T] = (T, T)\n");
        let Item::TypeAlias(a) = &m.items[0] else {
            panic!("expected a type alias");
        };
        assert_eq!(a.name, "Pair");
        assert_eq!(a.generics.len(), 1);
        assert_eq!(a.generics[0].name, "T");
        assert!(matches!(&a.target, Type::Tuple { elems, .. } if elems.len() == 2));
    }

    #[test]
    fn type_alias_to_named_and_collection_parse() {
        // The target may be a plain named type, or a built-in collection whose
        // element is itself an alias-eligible named type.
        let m =
            parse_ok("type Id = int64\ntype Ids = list[int64]\ntype Table = dict[str, int64]\n");
        assert_eq!(m.items.len(), 3);
        for (i, name) in [(0, "Id"), (1, "Ids"), (2, "Table")] {
            let Item::TypeAlias(a) = &m.items[i] else {
                panic!("expected a type alias at {i}");
            };
            assert_eq!(a.name, name);
        }
    }

    #[test]
    fn type_alias_to_func_type_parses() {
        // A function-value type (I#163) is a valid alias target.
        let m = parse_ok("type Pred = fn(int64) -> bool\n");
        let Item::TypeAlias(a) = &m.items[0] else {
            panic!("expected a type alias");
        };
        assert!(matches!(&a.target, Type::Func { .. }));
    }

    #[test]
    fn type_is_still_a_usable_identifier() {
        // `type` is a SOFT keyword recognized only at item start when followed by
        // an identifier; every other use stays a plain identifier (no alias).
        let m = parse_ok("def f():\n    auto type = 3\n    print(type)\n    type = type + 1\n");
        assert!(matches!(&m.items[0], Item::Function(_)));
        assert!(
            !m.items.iter().any(|it| matches!(it, Item::TypeAlias(_))),
            "`type` as a local identifier must not parse as an alias"
        );
    }

    #[test]
    fn call_site_type_args_parse_as_postfix_subscript() {
        // `Pair[int64, str](1, "x")`: the callee is a postfix `[...]` with a
        // comma-separated element list, produced as a uniform tuple-index node and
        // classified downstream (I#154 Fork S).
        let m = parse_ok("def main():\n    auto p = Pair[int64, str](1, \"x\")\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::Let { value, .. } = &f.body.stmts[0] else {
            panic!("let");
        };
        let Expr::Call { callee, .. } = value else {
            panic!("call");
        };
        let Expr::Index { base, index, .. } = callee.as_ref() else {
            panic!("expected postfix subscript callee");
        };
        assert!(matches!(base.as_ref(), Expr::Name(n, _) if n == "Pair"));
        let Expr::Tuple(elems, _) = index.as_ref() else {
            panic!("multi-arg index is a tuple node");
        };
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn enum_and_import() {
        let m = parse_ok(
            "enum Color:\n    RED\n    GREEN = 10\n    BLUE\nimport python \"numpy\" as np\n",
        );
        let Item::Enum(e) = &m.items[0] else {
            panic!("enum");
        };
        assert_eq!(e.variants.len(), 3);
        assert!(e.variants[1].value.is_some());
        assert!(e.variants.iter().all(|v| v.fields.is_empty()));
        assert!(matches!(&m.items[1], Item::Import(Import::Python { .. })));
    }

    #[test]
    fn pass_declares_an_empty_enum() {
        let m = parse_ok("enum Empty:\n    pass\n");
        let Item::Enum(e) = &m.items[0] else {
            panic!("enum");
        };
        assert!(e.variants.is_empty());
    }

    #[test]
    fn pass_is_a_no_op_statement(/* #591 */) {
        // `pass` is accepted anywhere a statement is expected and parses to a
        // single `Stmt::Pass` — a function body, every branch of a compound
        // statement, and `scope`.
        let src = "\
def demo(int64 n):
    if n > 0:
        pass
    else:
        pass
    for i in range(n):
        pass
    while False:
        pass
    try:
        pass
    except ValueError as e:
        pass
    finally:
        pass
    scope:
        pass
";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        // if / for / while / try / scope
        let Stmt::If {
            branches,
            else_block,
            ..
        } = &f.body.stmts[0]
        else {
            panic!("if");
        };
        assert!(matches!(branches[0].1.stmts[0], Stmt::Pass(_)));
        assert!(matches!(
            else_block.as_ref().unwrap().stmts[0],
            Stmt::Pass(_)
        ));
        let Stmt::For { body, .. } = &f.body.stmts[1] else {
            panic!("for");
        };
        assert!(matches!(body.stmts[0], Stmt::Pass(_)));
        let Stmt::While { body, .. } = &f.body.stmts[2] else {
            panic!("while");
        };
        assert!(matches!(body.stmts[0], Stmt::Pass(_)));
        let Stmt::Try {
            body,
            excepts,
            finally,
            ..
        } = &f.body.stmts[3]
        else {
            panic!("try");
        };
        assert!(matches!(body.stmts[0], Stmt::Pass(_)));
        assert!(matches!(excepts[0].body.stmts[0], Stmt::Pass(_)));
        assert!(matches!(finally.as_ref().unwrap().stmts[0], Stmt::Pass(_)));
        let Stmt::Scope { body, .. } = &f.body.stmts[4] else {
            panic!("scope");
        };
        assert!(matches!(body.stmts[0], Stmt::Pass(_)));
    }

    #[test]
    fn parses_except_star_clauses() {
        // `except*` (#605, D-0129): the `*` sets `is_star`, the type and `as`
        // binding parse as usual, and plain `except` stays `is_star == false`.
        // Both spacings (`except* E` and `except *E`) are accepted.
        let src = "\
def main():
    try:
        pass
    except* TimeoutError as t:
        pass
    except *ValueError as v:
        pass
";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::Try { excepts, .. } = &f.body.stmts[0] else {
            panic!("try");
        };
        assert_eq!(excepts.len(), 2);
        assert!(excepts[0].is_star);
        assert_eq!(excepts[0].binding.as_deref(), Some("t"));
        assert!(excepts[1].is_star);
        assert_eq!(excepts[1].binding.as_deref(), Some("v"));

        // A plain `except` clause is not starred.
        let plain =
            parse_ok("def main():\n    try:\n        pass\n    except ValueError:\n        pass\n");
        let Item::Function(pf) = &plain.items[0] else {
            panic!("fn");
        };
        let Stmt::Try { excepts, .. } = &pf.body.stmts[0] else {
            panic!("try");
        };
        assert!(!excepts[0].is_star);
    }

    #[test]
    fn pass_declares_an_empty_struct(/* #591 */) {
        let m = parse_ok("struct Empty:\n    pass\n");
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert!(s.fields.is_empty());
        assert!(s.methods.is_empty());
    }

    #[test]
    fn pass_stays_an_identifier_outside_statement_position(/* #591 */) {
        // `pass` is a contextual statement-position word, not a reserved
        // keyword, so it remains usable as an ordinary identifier.
        let m = parse_ok("def main():\n    int64 pass = 5\n    pass = pass + 1\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        assert!(matches!(&f.body.stmts[0], Stmt::Let { name, .. } if name == "pass"));
        // `pass = pass + 1` is an assignment, not a no-op statement.
        assert!(matches!(&f.body.stmts[1], Stmt::Assign { .. }));
    }

    #[test]
    fn chained_assignment_parses(/* #594 */) {
        // `a = b = c = 5` -> a single `AssignChain` with the three targets in
        // order and one shared right-hand side (evaluated once at lowering).
        let m = parse_ok("def main():\n    a = b = c = 5\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::AssignChain { targets, value, .. } = &f.body.stmts[0] else {
            panic!("expected AssignChain, got {:?}", f.body.stmts[0]);
        };
        assert_eq!(targets.len(), 3);
        assert!(matches!(&targets[0], Expr::Name(n, _) if n == "a"));
        assert!(matches!(&targets[1], Expr::Name(n, _) if n == "b"));
        assert!(matches!(&targets[2], Expr::Name(n, _) if n == "c"));
        assert!(matches!(value, Expr::Int(n, _) if n == "5"));
    }

    #[test]
    fn chained_assignment_allows_subscript_targets(/* #594 */) {
        let m = parse_ok("def main():\n    xs[0] = d[\"k\"] = y = 9\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::AssignChain { targets, .. } = &f.body.stmts[0] else {
            panic!("expected AssignChain");
        };
        assert_eq!(targets.len(), 3);
        assert!(matches!(&targets[0], Expr::Index { .. }));
        assert!(matches!(&targets[1], Expr::Index { .. }));
        assert!(matches!(&targets[2], Expr::Name(n, _) if n == "y"));
    }

    #[test]
    fn single_assignment_is_not_a_chain(/* #594 */) {
        // A lone `=` still parses to a plain `Assign`, not an `AssignChain`.
        let m = parse_ok("def main():\n    a = 5\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        assert!(matches!(&f.body.stmts[0], Stmt::Assign { .. }));
    }

    #[test]
    fn del_element_parses(/* #595 */) {
        // `del d[k]` / `del xs[i]` parse to `Stmt::Del` holding the subscript.
        let m = parse_ok("def main():\n    del xs[1]\n    del d[\"k\"]\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::Del { target, .. } = &f.body.stmts[0] else {
            panic!("expected Del, got {:?}", f.body.stmts[0]);
        };
        assert!(matches!(target, Expr::Index { .. }));
        assert!(
            matches!(&f.body.stmts[1], Stmt::Del { target, .. } if matches!(target, Expr::Index { .. }))
        );
    }

    #[test]
    fn del_stays_an_identifier_outside_statement_position(/* #595 */) {
        // `del` is a contextual statement-position word, not a reserved keyword:
        // it stays usable as an ordinary identifier (binding, assignment, call,
        // subscript base).
        let m = parse_ok(
            "def main():\n    int64 del = 5\n    del = del + 1\n    list[int64] xs = [1]\n    del[0] = xs\n",
        );
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        assert!(matches!(&f.body.stmts[0], Stmt::Let { name, .. } if name == "del"));
        assert!(matches!(&f.body.stmts[1], Stmt::Assign { .. }));
    }

    #[test]
    fn del_on_a_non_subscript_target_is_rejected(/* #595 */) {
        // Binding deletion (`del name`) is out of scope: the element form
        // requires a subscript target.
        let (_m, perrs, _lerrs) = parse_str("def main():\n    int64 x = 3\n    del x\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2013),
            "expected ZAP2013 for non-subscript del target, got {perrs:?}"
        );
    }

    #[test]
    fn not_in_parses_as_first_class_operator(/* #593 */) {
        // `x not in xs` is a first-class `Binary` with `BinOp::NotIn`.
        let m = parse_ok("def main():\n    bool b = 5 not in xs\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::Let { value, .. } = &f.body.stmts[0] else {
            panic!("let");
        };
        let Expr::Binary { op, .. } = value else {
            panic!("expected a binary `not in`, got {value:?}");
        };
        assert_eq!(*op, BinOp::NotIn);
    }

    #[test]
    fn not_in_mirrors_in_precedence(/* #593 */) {
        // `not in` binds at the same (comparison) precedence as `in`, below the
        // arithmetic operators: `x + 1 not in xs` groups as `(x + 1) not in xs`.
        let m = parse_ok("def main():\n    bool b = x + 1 not in xs\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let Stmt::Let { value, .. } = &f.body.stmts[0] else {
            panic!("let");
        };
        let Expr::Binary { op, lhs, .. } = value else {
            panic!("expected `not in` binary");
        };
        assert_eq!(*op, BinOp::NotIn);
        // The left operand is the whole `x + 1`, not just `1`.
        assert!(matches!(lhs.as_ref(), Expr::Binary { op: BinOp::Add, .. }));
    }

    #[test]
    fn unary_not_and_in_still_parse(/* #593 */) {
        // The `not in` handling must not disturb the standalone prefix `not`
        // nor the infix `in` operator.
        parse_ok("def main():\n    bool a = not done\n    bool b = 3 in xs\n");
    }

    /// Collect every `(path, alias)` from the module's logical-name imports,
    /// in source order.
    fn module_imports(m: &Module) -> Vec<(String, Option<String>)> {
        m.items
            .iter()
            .filter_map(|it| match it {
                Item::Import(Import::Module { path, alias, .. }) => {
                    Some((path.clone(), alias.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn import_single_and_aliased_and_dotted() {
        // The pre-existing single forms (D-0028) still parse to one import each.
        let m = parse_ok("import a\nimport b as c\nimport p.q.r\n");
        assert_eq!(
            module_imports(&m),
            vec![
                ("a".into(), None),
                ("b".into(), Some("c".into())),
                ("p.q.r".into(), None),
            ]
        );
    }

    #[test]
    fn import_comma_list_desugars_to_many() {
        // `import a, b.c, d as e` (D-0052) desugars to one import per module.
        let m = parse_ok("import a, b.c, d as e\n");
        assert_eq!(
            module_imports(&m),
            vec![
                ("a".into(), None),
                ("b.c".into(), None),
                ("d".into(), Some("e".into())),
            ]
        );
    }

    #[test]
    fn import_grouped_single_line() {
        // Grouped form on one line, with a per-item alias (D-0052).
        let m = parse_ok("import (numpy as np, boto, a.b)\n");
        assert_eq!(
            module_imports(&m),
            vec![
                ("numpy".into(), Some("np".into())),
                ("boto".into(), None),
                ("a.b".into(), None),
            ]
        );
    }

    #[test]
    fn import_grouped_multiline_trailing_comma() {
        // Grouped, spanning lines (lexer implicit continuation), with a trailing
        // comma — the only form where a trailing comma is allowed (D-0052).
        let m = parse_ok("import (\n    math,\n    random as rng,\n    time,\n)\n");
        assert_eq!(
            module_imports(&m),
            vec![
                ("math".into(), None),
                ("random".into(), Some("rng".into())),
                ("time".into(), None),
            ]
        );
    }

    #[test]
    fn import_bare_trailing_comma_is_an_error() {
        // A trailing comma is rejected in the bare comma-list form (only the
        // grouped form permits it), matching Python.
        let (_m, perrs, lerrs) = parse_str("import a, b,\n");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(
            !perrs.is_empty(),
            "a bare trailing comma should be a parse error"
        );
    }

    #[test]
    fn tier3_enum_variants_and_methods() {
        // Data-carrying variants with type-first named payload fields, a
        // payload-free variant, and a method in the body (D-0046).
        let src = "enum Shape:\n    Circle(f64 r)\n    Rect(f64 w, f64 h)\n    Unit\n\
                   \x20   def kind(self) -> int64:\n        return 0\n";
        let m = parse_ok(src);
        let Item::Enum(e) = &m.items[0] else {
            panic!("enum");
        };
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].fields.len(), 1);
        assert_eq!(e.variants[0].fields[0].name, "r");
        assert_eq!(e.variants[1].fields.len(), 2);
        assert!(e.variants[2].fields.is_empty());
        assert_eq!(e.methods.len(), 1);
        assert_eq!(e.methods[0].name, "kind");
    }

    #[test]
    fn qualified_and_positional_patterns() {
        // `Color.RED` (dotted constant), `Shape.Circle(r)` (positional), and
        // `Shape.Rect(w=w, h=h)` (named) all parse (D-0040/D-0046).
        let src = "def f(Shape s):\n    match s:\n\
                   \x20       case Shape.Circle(r):\n            a()\n\
                   \x20       case Shape.Rect(w=w, h=h):\n            b()\n\
                   \x20       case Color.RED:\n            c()\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::Match { arms, .. } = &f.body.stmts[0] else {
            panic!("match");
        };
        assert_eq!(arms.len(), 3);
        // Positional: one arg, no name.
        let Pattern::Struct { path, args, .. } = &arms[0].pattern else {
            panic!("variant pattern");
        };
        assert_eq!(path, &["Shape".to_string(), "Circle".to_string()]);
        let args = args.as_ref().expect("has parens");
        assert_eq!(args.len(), 1);
        assert!(args[0].name.is_none());
        // Named: both args carry field names.
        let Pattern::Struct { args, .. } = &arms[1].pattern else {
            panic!("variant pattern");
        };
        let args = args.as_ref().unwrap();
        assert_eq!(args[0].name.as_deref(), Some("w"));
        assert_eq!(args[1].name.as_deref(), Some("h"));
        // Bare qualified constant: no parens.
        let Pattern::Struct { path, args, .. } = &arms[2].pattern else {
            panic!("qualified constant pattern");
        };
        assert_eq!(path, &["Color".to_string(), "RED".to_string()]);
        assert!(args.is_none());
    }

    #[test]
    fn as_pattern_capture_parses() {
        // `pattern ::= or_pattern [ "as" IDENT ]` (D11, #603): the `as` binds
        // the whole matched value and wraps the entire or-pattern, so
        // `1 | 2 | 3 as n` parses as `(1 | 2 | 3) as n`, and `[x, y] as pair`
        // wraps the sequence (the inner captures are still present).
        let src = "def f(int64 v, list[int64] xs):\n    match v:\n\
                   \x20       case 1 | 2 | 3 as n:\n            a()\n\
                   \x20       case other as rest:\n            b()\n    match xs:\n\
                   \x20       case [x, y] as pair:\n            c()\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        // First match: the `as` wraps an or-pattern.
        let Stmt::Match { arms, .. } = &f.body.stmts[0] else {
            panic!("match");
        };
        let Pattern::As { pattern, name, .. } = &arms[0].pattern else {
            panic!("expected an `as` capture over the or-pattern");
        };
        assert_eq!(name, "n");
        assert!(
            matches!(pattern.as_ref(), Pattern::Or(alts, _) if alts.len() == 3),
            "the `as` must wrap the full 3-alternative or-pattern"
        );
        // `other as rest`: a bare capture wrapped by `as` (both names bind).
        let Pattern::As { pattern, name, .. } = &arms[1].pattern else {
            panic!("expected an `as` capture over a bare capture");
        };
        assert_eq!(name, "rest");
        assert!(matches!(pattern.as_ref(), Pattern::Capture { name, .. } if name == "other"));
        // Second match: the `as` wraps a sequence pattern, inner captures kept.
        let Stmt::Match { arms, .. } = &f.body.stmts[1] else {
            panic!("second match");
        };
        let Pattern::As { pattern, name, .. } = &arms[0].pattern else {
            panic!("expected an `as` capture over the sequence");
        };
        assert_eq!(name, "pair");
        assert!(
            matches!(pattern.as_ref(), Pattern::Sequence(ps, _) if ps.len() == 2),
            "the `as` must wrap the 2-element sequence pattern"
        );
    }

    #[test]
    fn nested_as_pattern_parses() {
        // The `as` suffix is available at every sub-pattern position, so a
        // nested capture inside a sequence element parses (`[x as a, y]`).
        let src = "def f(list[int64] xs):\n    match xs:\n\
                   \x20       case [x as a, y]:\n            a()\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::Match { arms, .. } = &f.body.stmts[0] else {
            panic!("match");
        };
        let Pattern::Sequence(ps, _) = &arms[0].pattern else {
            panic!("sequence");
        };
        let Pattern::As { pattern, name, .. } = &ps[0] else {
            panic!("nested `as` in the first element");
        };
        assert_eq!(name, "a");
        assert!(matches!(pattern.as_ref(), Pattern::Capture { name, .. } if name == "x"));
        assert!(matches!(&ps[1], Pattern::Capture { name, .. } if name == "y"));
    }

    #[test]
    fn as_pattern_rejects_bad_capture_name() {
        // `as _` is not a binding (ZAP2021): use the pattern without `as`.
        let (_m, perrs, _l) =
            parse_str("def f(int64 v):\n    match v:\n        case 1 as _:\n            a()\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2021),
            "expected ZAP2021 for `as _`, got {perrs:?}"
        );
        // `as` followed by a non-identifier (a literal) is also rejected.
        let (_m, perrs, _l) =
            parse_str("def f(int64 v):\n    match v:\n        case 1 as 2:\n            a()\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2021),
            "expected ZAP2021 for `as <non-ident>`, got {perrs:?}"
        );
        // A dotted capture target `as a.b` is rejected.
        let (_m, perrs, _l) =
            parse_str("def f(int64 v):\n    match v:\n        case 1 as a.b:\n            a()\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2021),
            "expected ZAP2021 for a dotted `as` target, got {perrs:?}"
        );
    }

    #[test]
    fn decorated_function() {
        let m = parse_ok("@cache\n@retry(times=3)\ndef fetch(str url) -> str:\n    return url\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.decorators.len(), 2);
        assert_eq!(f.decorators[0].name, "cache");
        assert_eq!(f.decorators[1].name, "retry");
        assert_eq!(f.decorators[1].args.len(), 1);
    }

    #[test]
    fn regex_match_and_subst_exprs() {
        let m = parse_ok("def f(str s):\n    if s =~ m/foo/i:\n        s ~= s/a/b/g\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::If { branches, .. } = &f.body.stmts[0] else {
            panic!("if");
        };
        assert!(matches!(
            branches[0].0,
            Expr::RegexMatch { negated: false, .. }
        ));
        assert!(matches!(branches[0].1.stmts[0], Stmt::Assign { .. }));
    }

    #[test]
    fn variadic_and_default_params() {
        let m = parse_ok("def g(int64 a, int64 b = 2, int64... rest) -> int64:\n    return a\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 3);
        assert!(f.params[1].default.is_some());
        assert!(f.params[2].variadic);
    }

    #[test]
    fn kwargs_collector_param_parses() {
        // `V **name` is a trailing keyword-collector parameter (#601): it is not
        // variadic, it carries the `kwargs` flag, and its declared type is the
        // dict value type `V`.
        let m = parse_ok("def f(int64 a, str **opts) -> int64:\n    return a\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 2);
        assert!(!f.params[0].kwargs);
        assert!(f.params[1].kwargs);
        assert!(!f.params[1].variadic);
        assert_eq!(f.params[1].name, "opts");
    }

    #[test]
    fn starred_call_arguments_parse() {
        // `*expr` / `**expr` spread arguments (#597) carry a spread kind and no
        // name; ordinary and keyword arguments keep `ArgSpread::None`.
        let m = parse_ok("def main():\n    f(a, *xs, k=1, **d)\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::Expr(Expr::Call { args, .. }) = &f.body.stmts[0] else {
            panic!("expected a call expression statement");
        };
        assert_eq!(args.len(), 4);
        assert_eq!(args[0].spread, ArgSpread::None);
        assert!(args[0].name.is_none());
        assert_eq!(args[1].spread, ArgSpread::Iterable);
        assert!(args[1].name.is_none());
        assert_eq!(args[2].spread, ArgSpread::None);
        assert_eq!(args[2].name.as_deref(), Some("k"));
        assert_eq!(args[3].spread, ArgSpread::Mapping);
        assert!(args[3].name.is_none());
    }

    #[test]
    fn keyword_only_separator_flags_following_params() {
        // `*` makes every following parameter keyword-only (I#496); params before
        // it stay positional-or-keyword. The `*` is a separator, not a parameter,
        // so it does not appear in `params`.
        let m = parse_ok("def f(int64 a, *, int64 b = 1, int64 c) -> int64:\n    return a\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 3);
        assert!(!f.params[0].keyword_only);
        assert!(f.params[1].keyword_only);
        assert!(f.params[1].default.is_some());
        assert!(f.params[2].keyword_only);
    }

    #[test]
    fn keyword_only_separator_with_no_leading_positional() {
        let m = parse_ok("def f(*, int64 b):\n    return\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 1);
        assert!(f.params[0].keyword_only);
    }

    #[test]
    fn bare_star_with_nothing_after_is_rejected() {
        let (_m, perrs, lerrs) = parse_str("def f(int64 a, *):\n    return\n");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2018),
            "expected ZAP2018, got {perrs:?}"
        );
    }

    #[test]
    fn duplicate_star_is_rejected() {
        let (_m, perrs, _l) = parse_str("def f(int64 a, *, *, int64 b):\n    return\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2018),
            "expected ZAP2018, got {perrs:?}"
        );
    }

    #[test]
    fn positional_only_separator_flags_preceding_params() {
        // `/` makes every preceding parameter positional-only (#602); params after
        // it stay positional-or-keyword. The `/` is a separator, not a parameter,
        // so it does not appear in `params`.
        let m = parse_ok("def f(int64 x, int64 y, /, int64 z) -> int64:\n    return x\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 3);
        assert!(f.params[0].positional_only);
        assert!(f.params[1].positional_only);
        assert!(!f.params[2].positional_only);
    }

    #[test]
    fn positional_only_and_keyword_only_separators_combine() {
        // `/` before `*`: `x` positional-only, `z` positional-or-keyword, `w`
        // keyword-only.
        let m = parse_ok("def f(int64 x, /, int64 z, *, int64 w):\n    return\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 3);
        assert!(f.params[0].positional_only && !f.params[0].keyword_only);
        assert!(!f.params[1].positional_only && !f.params[1].keyword_only);
        assert!(!f.params[2].positional_only && f.params[2].keyword_only);
    }

    #[test]
    fn trailing_positional_only_separator_is_allowed() {
        // Every parameter positional-only: `def f(a, b, /)`.
        let m = parse_ok("def f(int64 a, int64 b, /):\n    return\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 2);
        assert!(f.params[0].positional_only);
        assert!(f.params[1].positional_only);
    }

    #[test]
    fn bare_slash_with_no_leading_param_is_rejected() {
        let (_m, perrs, lerrs) = parse_str("def f(/, int64 a):\n    return\n");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2019),
            "expected ZAP2019, got {perrs:?}"
        );
    }

    #[test]
    fn duplicate_slash_is_rejected() {
        let (_m, perrs, _l) = parse_str("def f(int64 a, /, int64 b, /, int64 c):\n    return\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2019),
            "expected ZAP2019, got {perrs:?}"
        );
    }

    #[test]
    fn slash_after_star_is_rejected() {
        let (_m, perrs, _l) = parse_str("def f(int64 a, *, int64 b, /, int64 c):\n    return\n");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2019),
            "expected ZAP2019, got {perrs:?}"
        );
    }

    #[test]
    fn errors_are_recovered_not_panicked() {
        // Missing ')' — should produce errors, not panic.
        let (_m, perrs, _l) = parse_str("def f(:\n    return 1\n");
        assert!(!perrs.is_empty());
    }

    #[test]
    fn bare_statement_call_with_keyword_args_parses() {
        // A call used as a statement (not a declaration RHS) must still parse
        // keyword arguments. The speculative `TYPE name = ...` declaration probe
        // used to over-consume `xs.sort(reverse=True)` / `f(x=1)` and mistake the
        // kwarg's `=` for the declaration's, corrupting the parse (I#336).
        let (m, perrs, lerrs) =
            parse_str("def f():\n    xs.sort(key=g, reverse=True)\n    h(x=1)\n");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected a function");
        };
        // `xs.sort(key=g, reverse=True)` — a method call with two keyword args.
        let Stmt::Expr(Expr::Call { callee, args, .. }) = &f.body.stmts[0] else {
            panic!(
                "expected an expression-statement call, got {:?}",
                f.body.stmts[0]
            );
        };
        assert!(matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "sort"));
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name.as_deref(), Some("key"));
        assert_eq!(args[1].name.as_deref(), Some("reverse"));
        // `h(x=1)` — a plain function call statement with a keyword arg.
        let Stmt::Expr(Expr::Call { args, .. }) = &f.body.stmts[1] else {
            panic!("expected a call statement, got {:?}", f.body.stmts[1]);
        };
        assert_eq!(args[0].name.as_deref(), Some("x"));
    }

    // --- tuples (ticket #54, D-0049) ---

    fn body(src: &str) -> Vec<Stmt> {
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!("expected a function");
        };
        f.body.stmts.clone()
    }

    #[test]
    fn single_name_decl_stays_plain_let() {
        // A non-destructuring `auto x = ...` is still a plain `Let`, not a tuple.
        let s = body("def f():\n    auto x = 1\n");
        assert!(matches!(&s[0], Stmt::Let { name, .. } if name == "x"));
    }

    #[test]
    fn parenthesized_tuple_decl_is_let_tuple() {
        let s = body("def f():\n    auto (a, b, c) = t\n");
        let Stmt::LetTuple { target, .. } = &s[0] else {
            panic!("expected LetTuple, got {:?}", s[0]);
        };
        let BindTarget::Tuple(elems, _) = target else {
            panic!("expected a tuple target");
        };
        assert_eq!(elems.len(), 3);
        assert!(matches!(&elems[0], BindTarget::Elem(Expr::Name(n, _)) if n == "a"));
    }

    #[test]
    fn bare_comma_decl_is_let_tuple() {
        // `auto a, b = ...` (no parentheses) also destructures.
        let s = body("def f():\n    auto a, b = t\n");
        assert!(matches!(&s[0], Stmt::LetTuple { .. }));
    }

    #[test]
    fn rest_target_parses() {
        let s = body("def f():\n    auto (first, *rest) = xs\n");
        let Stmt::LetTuple { target, .. } = &s[0] else {
            panic!("expected LetTuple");
        };
        let BindTarget::Tuple(elems, _) = target else {
            panic!("tuple target");
        };
        assert_eq!(elems.len(), 2);
        assert!(matches!(&elems[1], BindTarget::Rest(..)));
    }

    #[test]
    fn swap_assignment_is_assign_tuple() {
        let s = body("def f():\n    (x, y) = (y, x)\n");
        assert!(matches!(&s[0], Stmt::AssignTuple { .. }));
    }

    #[test]
    fn single_lvalue_assignment_stays_plain_assign() {
        let s = body("def f():\n    x = 1\n");
        assert!(matches!(&s[0], Stmt::Assign { .. }));
    }

    #[test]
    fn bare_comma_both_sides_swap() {
        // `a, b = b, a` — both the target and the value are bare comma-lists.
        let s = body("def f():\n    a, b = b, a\n");
        let Stmt::AssignTuple { value, .. } = &s[0] else {
            panic!("expected AssignTuple, got {:?}", s[0]);
        };
        assert!(matches!(value, Expr::Tuple(elems, _) if elems.len() == 2));
    }

    #[test]
    fn return_comma_list_is_a_tuple() {
        let s = body("def f():\n    return a, b\n");
        let Stmt::Return { value: Some(e), .. } = &s[0] else {
            panic!("expected return");
        };
        assert!(matches!(e, Expr::Tuple(elems, _) if elems.len() == 2));
    }

    #[test]
    fn one_tuple_needs_trailing_comma() {
        // `(x)` is grouping (a single `Let`); `(x,)` is a 1-tuple target.
        let s = body("def f():\n    auto (x) = t\n");
        assert!(matches!(&s[0], Stmt::Let { name, .. } if name == "x"));
        let s = body("def f():\n    auto (x,) = t\n");
        let Stmt::LetTuple { target, .. } = &s[0] else {
            panic!("expected LetTuple for a 1-tuple target");
        };
        assert!(matches!(target, BindTarget::Tuple(elems, _) if elems.len() == 1));
    }

    // --- prelude injection (ticket #77, #281, #497) ---

    /// The names of the top-level types `with_prelude` leaves in the module.
    fn type_names(m: &Module) -> Vec<String> {
        m.items
            .iter()
            .filter_map(|it| item_type_name(it).map(str::to_string))
            .collect()
    }

    #[test]
    fn plain_program_gets_no_prelude() {
        // A program that touches no prelude surface is byte-for-byte unchanged:
        // nothing is injected (ticket #77, D-0058).
        let m = with_prelude(&parse_ok("def main():\n    print(1)\n"));
        assert!(
            !type_names(&m).iter().any(|n| n == "Optional"),
            "no prelude type should be injected: {:?}",
            type_names(&m)
        );
    }

    #[test]
    fn inline_dict_get_injects_only_optional() {
        // `d.get(k)` returns the prelude `Optional[V]` (#281) even when the
        // program never spells `Optional`, so an inline `d.get(k).unwrap_or(...)`
        // must still get the enum injected (#497) — but ONLY that enum, so a
        // program using `.get` and nothing else stays lean (no `Signal`,
        // `DateTime`, `Thread`, … dragged in).
        let m = with_prelude(&parse_ok(
            "def main():\n    dict[str, str] d = {\"k\": \"v\"}\n    print(d.get(\"k\").unwrap_or(\"x\"))\n",
        ));
        let names = type_names(&m);
        assert!(
            names.iter().any(|n| n == "Optional"),
            "`.get` must pull in the prelude `Optional[T]`: {names:?}"
        );
        for heavy in ["Signal", "MemStats", "DateTime", "Thread", "Atomic"] {
            assert!(
                !names.iter().any(|n| n == heavy),
                "the `Optional`-only path must not inject {heavy}: {names:?}"
            );
        }
    }

    #[test]
    fn naming_optional_injects_full_prelude() {
        // Spelling `Optional` (the annotated-local form) takes the full-prelude
        // path, unchanged by #497: the heavier surface rides along as before.
        let m = with_prelude(&parse_ok(
            "def main():\n    Optional[int64] x = Optional.Some(1)\n    print(x.unwrap_or(0))\n",
        ));
        let names = type_names(&m);
        assert!(names.iter().any(|n| n == "Optional"), "{names:?}");
        assert!(
            names.iter().any(|n| n == "DateTime"),
            "the full prelude should ride along when a needle fires: {names:?}"
        );
    }

    /// Names of the top-level functions `with_prelude` leaves in the module.
    fn fn_names(m: &Module) -> Vec<String> {
        m.items
            .iter()
            .filter_map(|it| match it {
                crate::ast::Item::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn user_struct_shadowing_counter_drops_dependent_prelude_helper() {
        // #689: a user type named `Counter` fires the `"Counter"` needle, pulling
        // in the `concurrent` prelude. The prelude `struct Counter` is shadowed
        // out by the user's (the `Some(n)` filter arm), but the concrete helper
        // `__concurrent_counter` — which builds `Counter(_cell=Atomic(start))` —
        // would otherwise survive and bind `._cell` to the user's unrelated
        // `Counter`, failing at lowering (`no field '_cell' on struct`). It must
        // be dropped alongside the shadowed type.
        let m = with_prelude(&parse_ok(
            "struct Counter:\n    int64 n\ndef main():\n    Counter c = Counter(5)\n    print(c.n)\n",
        ));
        let fns = fn_names(&m);
        assert!(
            !fns.iter().any(|n| n == "__concurrent_counter"),
            "the helper that references the shadowed prelude Counter must be dropped: {fns:?}"
        );
        // Exactly one `Counter` type remains — the user's (field `n`, not `_cell`).
        let counters: Vec<_> = m
            .items
            .iter()
            .filter_map(|it| match it {
                crate::ast::Item::Struct(s) if s.name == "Counter" => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(counters.len(), 1, "only the user's Counter should remain");
        assert_eq!(counters[0].fields[0].name, "n");
    }

    #[test]
    fn concurrent_helper_kept_without_a_user_collision() {
        // Without a user `Counter`, the concrete `__concurrent_counter` helper
        // (and the prelude `Counter` struct) must still be injected so that
        // `concurrent.Counter(...)` lowers — the drop in #689 is scoped strictly
        // to a shadowing collision.
        let m = with_prelude(&parse_ok(
            "import concurrent\ndef main():\n    auto c = concurrent.Counter(5)\n    print(c.get())\n",
        ));
        assert!(
            fn_names(&m).iter().any(|n| n == "__concurrent_counter"),
            "the helper must be present when the user does not shadow Counter"
        );
        assert!(
            type_names(&m).iter().any(|n| n == "Counter"),
            "the prelude Counter struct must be present without a collision"
        );
    }
}

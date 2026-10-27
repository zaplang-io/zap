//! Exhaustive-interleaving model check of the `concurrent.Queue` MPSC protocol
//! (ticket #184, D-0078 §5, testing layer 2).
//!
//! The deterministic-sum goldens (`tests/golden/concurrent_queue_mpsc_*`) test
//! the real Zap lowering + GC integration, but only hit rare interleavings
//! probabilistically. This is the complementary layer: a faithful Rust
//! transcription of the enqueue/dequeue CAS protocol under the `loom` bounded
//! interleaving explorer, which checks *every* reordering of the atomic
//! operations the algorithm performs — the only way to get real coverage of the
//! rare producer/producer and producer/consumer races.
//!
//! The transcription mirrors the prelude `Queue[T]`/`QNode[T]` exactly
//! (`crates/syntax/src/parser.rs`): a permanent stub node keeps `head`/`tail`
//! non-null; `push` is `prev = tail.swap(n); prev.next.store(n)`; the single
//! consumer's `pop` reads `head.next`, advances `head`, and returns the node's
//! value. All operations are sequentially consistent, matching `Atomic[T]`
//! (D-0066). Nodes are leaked rather than freed — faithfully modeling the
//! "lean on the tracing GC, never free manually" reclamation strategy (§4).
//!
//! This test compiles and runs ONLY under `--cfg loom` (loom is a target-cfg
//! dependency), so an ordinary `cargo test` / CI run skips it entirely. Run it
//! with:
//!     RUSTFLAGS="--cfg loom" cargo test -p zapc_ir --test concurrent_loom

#![cfg(loom)]

use loom::sync::atomic::{AtomicPtr, Ordering::SeqCst};
use loom::sync::Arc;
use loom::thread;
use std::ptr;

struct Node {
    val: i64,
    // `next` is the atomic nullable link; null == no successor yet (the prelude
    // models this as `Atomic[Optional[QNode[T]]]`, a niche nullable pointer).
    next: AtomicPtr<Node>,
}

impl Node {
    fn new(val: i64) -> *mut Node {
        Box::into_raw(Box::new(Node {
            val,
            next: AtomicPtr::new(ptr::null_mut()),
        }))
    }
}

struct Queue {
    head: AtomicPtr<Node>, // consumer end: stub / last-consumed (never null)
    tail: AtomicPtr<Node>, // producer end: most recently pushed (never null)
}

impl Queue {
    fn new() -> Self {
        let stub = Node::new(0); // value never read
        Queue {
            head: AtomicPtr::new(stub),
            tail: AtomicPtr::new(stub),
        }
    }

    /// Multi-producer enqueue: the atomic swap on `tail` is the single
    /// serialization point; the follow-up store publishes the link.
    fn push(&self, v: i64) {
        let n = Node::new(v);
        let prev = self.tail.swap(n, SeqCst);
        unsafe { (*prev).next.store(n, SeqCst) };
    }

    /// Single-consumer dequeue: read `head.next`; if present, advance `head`
    /// and return the value; else report empty (the transient window where a
    /// producer has swapped the tail but not yet stored the link).
    fn pop(&self) -> Option<i64> {
        let head = self.head.load(SeqCst);
        let nxt = unsafe { (*head).next.load(SeqCst) };
        if nxt.is_null() {
            return None;
        }
        self.head.store(nxt, SeqCst);
        Some(unsafe { (*nxt).val })
    }
}

/// Two producers push concurrently while the main thread (the SINGLE consumer)
/// opportunistically pops, then joins and drains the remainder. Across every
/// interleaving loom explores, the consumer must observe exactly the multiset
/// {1, 2} — no element lost, none duplicated, none fabricated.
#[test]
fn mpsc_two_producers_one_consumer_no_loss_no_dup() {
    loom::model(|| {
        let q = Arc::new(Queue::new());

        let qa = q.clone();
        let pa = thread::spawn(move || qa.push(1));
        let qb = q.clone();
        let pb = thread::spawn(move || qb.push(2));

        // Consumer: a bounded number of opportunistic pops concurrent with the
        // producers (no unbounded spin, which loom cannot bound).
        let mut got = Vec::new();
        for _ in 0..2 {
            if let Some(v) = q.pop() {
                got.push(v);
            }
        }

        pa.join().unwrap();
        pb.join().unwrap();

        // Drain anything the producers linked after our opportunistic pops.
        while let Some(v) = q.pop() {
            got.push(v);
        }

        got.sort_unstable();
        assert_eq!(
            got,
            vec![1, 2],
            "MPSC must deliver each element exactly once"
        );
    });
}

/// One producer races the single consumer on an initially empty queue: the
/// consumer must either observe `None` (empty / mid-insert) or the exact value
/// — never a torn or fabricated one — and after the producer is joined a final
/// drain must yield the element (no permanent loss).
#[test]
fn mpsc_one_producer_one_consumer_visibility() {
    loom::model(|| {
        let q = Arc::new(Queue::new());

        let qp = q.clone();
        let p = thread::spawn(move || qp.push(42));

        let early = q.pop();
        assert!(
            early.is_none() || early == Some(42),
            "a concurrent pop must return None or the pushed value, never garbage"
        );

        p.join().unwrap();

        // The element is delivered exactly once overall.
        let mut seen = early.into_iter().collect::<Vec<_>>();
        while let Some(v) = q.pop() {
            seen.push(v);
        }
        assert_eq!(
            seen,
            vec![42],
            "the single pushed element must survive exactly once"
        );
    });
}

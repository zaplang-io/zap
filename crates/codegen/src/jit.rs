//! The REPL JIT engine (I#159, design/0025-repl.md).
//!
//! `zap repl` must compile and execute each entry while persisting typed state.
//! Zap is statically typed and compiled AOT through LLVM, so the REPL reuses the
//! real `AST -> ZIR -> LLVM` backend through an in-process **MCJIT** (inkwell
//! 0.5 exposes no safe ORCv2): every entry is translated by the same code
//! `zap run`/`zap build` use, giving identical semantics (monomorphization,
//! value-struct layout, exceptions, Immix GC).
//!
//! Lifetime: a REPL runs for the whole process. The C runtime (`zaprt.c`) is
//! compiled once to a shared library and loaded with `RTLD_GLOBAL`, so its GC
//! constructor runs and MCJIT resolves `zap_*` symbols by `dlsym`. An LLVM
//! `Context` is leaked to `'static` (it lives as long as the session) and each
//! entry adds a fresh module to one long-lived `ExecutionEngine`.
//!
//! This engine is host-only (it needs LLVM), exactly like the rest of codegen;
//! the driver gates it behind the `codegen` feature.

use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::OptimizationLevel;

use zapc_syntax::ast::Module as AstModule;

/// The process-global runtime: the C runtime is compiled and `dlopen`ed exactly
/// once per process (its GC constructor runs once, and a single Immix heap /
/// scheduler backs the whole process), and every [`ReplJit`] shares it. The
/// handle is stored as a `usize` so the `OnceLock` stays `Send`/`Sync`; it is
/// intentionally never `dlclose`d (process-lifetime).
static RUNTIME: OnceLock<Result<usize, String>> = OnceLock::new();

/// Build + load the runtime once; return its `dlopen` handle.
fn runtime_handle() -> Result<*mut c_void, String> {
    let r = RUNTIME.get_or_init(|| {
        let (dir, lib) = build_runtime_dylib()?;
        let handle = unsafe {
            let path = CString::new(lib.to_string_lossy().as_bytes())
                .map_err(|e| format!("runtime path has an interior NUL: {e}"))?;
            libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL)
        };
        if handle.is_null() {
            let _ = std::fs::remove_dir_all(&dir);
            let err = unsafe { libc::dlerror() };
            let detail = if err.is_null() {
                "unknown error".to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(err) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(format!("failed to load the Zap runtime: {detail}"));
        }
        // The runtime lives for the whole process: keep the temp files and the
        // handle (never removed / closed), so its symbols and GC stay live.
        std::mem::forget(dir);
        Ok(handle as usize)
    });
    match r {
        Ok(h) => Ok(*h as *mut c_void),
        Err(e) => Err(e.clone()),
    }
}

/// A live REPL JIT session: a leaked LLVM context and one MCJIT execution engine
/// that accumulates per-entry modules, over the process-global runtime.
pub struct ReplJit {
    /// The leaked LLVM context (lives for the whole session).
    ctx: &'static Context,
    /// The MCJIT engine; every entry module is added here.
    engine: ExecutionEngine<'static>,
    /// `dlopen` handle for the process-global runtime (for `dlsym`; not owned).
    rt_handle: *mut c_void,
    /// Monotonic entry counter, used to give each module a unique name.
    gen: u32,
}

/// The result of running one entry thunk.
#[derive(Debug, Default)]
pub struct EntryOutcome {
    /// `Some(message)` if an uncaught Zap exception was caught (the session
    /// survives); `None` on normal completion.
    pub runtime_error: Option<String>,
}

impl ReplJit {
    /// Start a session over the process-global runtime (loading it on first
    /// use so its GC constructor runs and MCJIT can resolve `zap_*` symbols by
    /// `dlsym`), then create this session's MCJIT engine.
    pub fn new() -> Result<Self, String> {
        let handle = runtime_handle()?;

        // A leaked context lives for the whole session; the engine and every
        // module borrow it for `'static`.
        let ctx: &'static Context = Box::leak(Box::new(Context::create()));
        let seed = ctx.create_module("zap_repl_seed");
        let engine = seed
            .create_jit_execution_engine(OptimizationLevel::None)
            .map_err(|e| format!("failed to create the JIT engine: {}", e.to_string()))?;

        Ok(Self {
            ctx,
            engine,
            rt_handle: handle,
            gen: 0,
        })
    }

    /// Translate a type-checked AST module, add it to the engine, and run its
    /// `entry` function (a `void(void)` thunk) through `zap_repl_call`, which
    /// catches an uncaught Zap exception instead of exiting. Returns the outcome
    /// (ok, or the caught runtime-error message). `entry` must name a function
    /// defined in `ast` that carries the implicit top-level landing pad.
    pub fn run_entry(&mut self, ast: &AstModule, entry: &str) -> Result<EntryOutcome, String> {
        let addr = self.add_and_lookup(ast, entry)?;
        // SAFETY: `zap_repl_call` is defined by the loaded runtime and takes a
        // `void(*)(void)` thunk; `addr` is the JIT'd `entry` of that type.
        let rc = unsafe {
            let call: extern "C" fn(thunk: extern "C" fn()) -> i64 =
                std::mem::transmute(self.resolve("zap_repl_call")?);
            let thunk: extern "C" fn() = std::mem::transmute(addr);
            call(thunk)
        };
        if rc == 0 {
            return Ok(EntryOutcome::default());
        }
        Ok(EntryOutcome {
            runtime_error: Some(self.last_exc_message()),
        })
    }

    /// Translate `ast`, add the module to the engine, and return the JIT'd
    /// address of `entry`. The module is named per generation so names never
    /// clash across entries.
    fn add_and_lookup(&mut self, ast: &AstModule, entry: &str) -> Result<u64, String> {
        self.gen += 1;
        let name = format!("zap_repl_{}", self.gen);
        let builder = self.ctx.create_builder();
        let (module, _tm) = crate::build_llvm_module(self.ctx, &builder, ast, &name, true)?;
        self.engine
            .add_module(&module)
            .map_err(|()| "failed to add the entry module to the JIT engine".to_string())?;
        self.engine
            .get_function_address(entry)
            .map_err(|e| format!("JIT lookup of `{entry}` failed: {e:?}"))
            .map(|a| a as u64)
    }

    /// Run a whole program's `main` under the JIT (used by the engine smoke
    /// tests). Returns `main`'s exit code.
    pub fn run_main(&mut self, ast: &AstModule) -> Result<i32, String> {
        let addr = self.add_and_lookup(ast, "main")?;
        // SAFETY: a compiled Zap `main` has the C signature `int main(void)`.
        let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(addr) };
        Ok(f())
    }

    /// Resolve a runtime symbol by name from the loaded runtime.
    fn resolve(&self, sym: &str) -> Result<*const c_void, String> {
        let c = CString::new(sym).map_err(|e| format!("bad symbol name: {e}"))?;
        let p = unsafe { libc::dlsym(self.rt_handle, c.as_ptr()) };
        if p.is_null() {
            return Err(format!("runtime symbol `{sym}` not found"));
        }
        Ok(p as *const c_void)
    }

    /// Copy out the message of the exception the last `zap_repl_call` caught.
    fn last_exc_message(&self) -> String {
        let f = match self.resolve("zap_repl_last_exc_bytes") {
            Ok(p) => p,
            Err(_) => return String::new(),
        };
        // SAFETY: matches `int64_t zap_repl_last_exc_bytes(const char **)`.
        unsafe {
            let getter: extern "C" fn(*mut *const c_char) -> i64 = std::mem::transmute(f);
            let mut data: *const c_char = std::ptr::null();
            let len = getter(&mut data);
            if data.is_null() || len <= 0 {
                return String::new();
            }
            let bytes = std::slice::from_raw_parts(data as *const u8, len as usize);
            String::from_utf8_lossy(bytes).into_owned()
        }
    }
}

/// Compile the C runtime to a shared library for the session. Returns the temp
/// directory (removed when the session drops) and the library path. POSIX-only:
/// the JIT is a host developer tool and Windows program linking is not yet
/// supported (design/0016, Area A).
fn build_runtime_dylib() -> Result<(PathBuf, PathBuf), String> {
    let triple = inkwell::targets::TargetMachine::get_default_triple();
    if triple.as_str().to_string_lossy().contains("windows") {
        return Err("`zap repl` is not supported on Windows yet (I#159)".to_string());
    }

    let dir = std::env::temp_dir().join(format!("zap-repl-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("failed to create runtime dir: {e}"))?;
    std::fs::write(dir.join("zaprt.c"), crate::RUNTIME_C)
        .map_err(|e| format!("failed to write runtime: {e}"))?;
    std::fs::write(dir.join("zapgc.inc.c"), crate::RUNTIME_GC_C)
        .map_err(|e| format!("failed to write gc runtime: {e}"))?;

    let is_macos = triple.as_str().to_string_lossy().contains("darwin");
    let lib = dir.join(if is_macos {
        "libzaprt.dylib"
    } else {
        "libzaprt.so"
    });

    let cc = std::env::var("ZAP_CC").unwrap_or_else(|_| "cc".to_string());
    let mut cmd = Command::new(&cc);
    cmd.arg("-shared").arg("-fPIC");
    cmd.arg(dir.join("zaprt.c")).arg("-o").arg(&lib);
    // Same collector + system libraries as the executable link (build_executable).
    #[cfg(feature = "gc-immix")]
    cmd.arg("-DZAP_GC_IMMIX");
    #[cfg(not(feature = "gc-immix"))]
    cmd.arg("-lgc");
    cmd.arg("-lstdc++")
        .arg("-lm")
        .arg("-lpcre2-8")
        .arg("-lz")
        .arg("-lssl")
        .arg("-lcrypto")
        .arg("-lpthread");
    // CPython embedding flags (ticket #27): the runtime references libpython.
    cmd.args(crate::python_embed_flags()?);
    if let Ok(extra) = std::env::var("ZAP_CC_EXTRA") {
        for a in extra.split_whitespace() {
            cmd.arg(a);
        }
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run the C compiler (cc): {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("failed to compile the Zap runtime: {status}"));
    }
    Ok((dir, lib))
}

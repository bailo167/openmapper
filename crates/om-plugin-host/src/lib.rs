// SPDX-License-Identifier: Apache-2.0
//! WebAssembly plugin host (ABI v1, see `om-plugin-api`).
//!
//! Safety properties:
//! - **No ambient authority**: only the `openmapper` imports the plugin
//!   declares are linked; anything else (WASI, unknown functions) fails to
//!   load.
//! - **Bounded**: each instance has a memory cap, a fuel budget per call
//!   (deterministic instruction limit) and a wall-clock deadline (epoch
//!   interruption), so an infinite loop is stopped.
//! - **Contained**: traps (crashes) become errors; a trapped instance is
//!   discarded and recreated.
//! - **Off the render thread**: [`runner::PluginRunner`] calls plugins on
//!   its own thread; callers never wait on a plugin.

pub mod runner;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use om_plugin_api::{ABI_VERSION, Capability, HOST_MODULE, Manifest};
use wasmtime::{
    Caller, Config, Engine, ExternType, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap, TypedFunc,
};

/// Default per-instance memory cap.
pub const DEFAULT_MEMORY: usize = 256 << 20;
/// Default instruction budget per call (fuel units ≈ instructions).
pub const DEFAULT_FUEL: u64 = 4_000_000_000;
/// Default wall-clock limit per call.
pub const DEFAULT_DEADLINE: Duration = Duration::from_millis(500);
/// Epoch tick period (deadline resolution).
const TICK: Duration = Duration::from_millis(10);
/// Most log lines kept per instance.
const MAX_LOG: usize = 256;
/// Largest frame accepted (pixels).
const MAX_PIXELS: u64 = 8192 * 8192;

/// Why a plugin could not be loaded or run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    #[error("not a valid WebAssembly module: {0}")]
    Invalid(String),
    #[error("plugin imports {0:?}, which OpenMapper does not provide")]
    ForbiddenImport(String),
    #[error("plugin uses {0:?} without declaring it in its manifest")]
    UndeclaredCapability(String),
    #[error("plugin is missing the export {0:?} (or it has the wrong type)")]
    MissingExport(String),
    #[error("plugin manifest: {0}")]
    Manifest(String),
    #[error("plugin exceeded its time limit and was stopped")]
    Timeout,
    #[error("plugin exceeded its instruction budget and was stopped")]
    OutOfFuel,
    #[error("plugin ran out of memory (limit {0} MB)")]
    OutOfMemory(usize),
    #[error("plugin crashed: {0}")]
    Trap(String),
    #[error("plugin returned error code {0}")]
    Failed(i32),
    #[error("frame is too large or malformed for the plugin")]
    BadFrame,
}

/// Resource limits for instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory: usize,
    pub fuel: u64,
    pub deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory: DEFAULT_MEMORY,
            fuel: DEFAULT_FUEL,
            deadline: DEFAULT_DEADLINE,
        }
    }
}

/// Shared JIT engine plus the epoch ticker that enforces deadlines.
pub struct PluginHost {
    engine: Engine,
    limits: Limits,
    stop: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("limits", &self.limits)
            .finish()
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.ticker.take() {
            let _ = t.join();
        }
    }
}

/// A validated plugin module, ready to instantiate.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: Manifest,
    /// Capabilities it actually imports (⊆ the manifest's).
    pub imports: Vec<Capability>,
    module: Module,
}

struct HostState {
    limits: StoreLimits,
    log: Arc<Mutex<Vec<String>>>,
    time: f64,
}

fn trap_error(e: &wasmtime::Error, limits: &Limits) -> PluginError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::Interrupt) => PluginError::Timeout,
        Some(Trap::OutOfFuel) => PluginError::OutOfFuel,
        Some(t) => PluginError::Trap(t.to_string()),
        None => {
            let text = format!("{e:#}");
            if text.contains("memory") && text.contains("limit") {
                PluginError::OutOfMemory(limits.memory >> 20)
            } else {
                PluginError::Trap(text)
            }
        }
    }
}

impl PluginHost {
    /// A host with default limits.
    pub fn new() -> Result<Self, PluginError> {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Result<Self, PluginError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|e| PluginError::Invalid(e.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = {
            let (engine, stop) = (engine.clone(), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("om-plugin-epoch".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(TICK);
                        engine.increment_epoch();
                    }
                })
                .ok()
        };
        Ok(Self {
            engine,
            limits,
            stop,
            ticker,
        })
    }

    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Validates a plugin (binary `.wasm` or WAT text): imports,
    /// exports, ABI version and manifest.
    pub fn load(&self, bytes: &[u8]) -> Result<Plugin, PluginError> {
        let module =
            Module::new(&self.engine, bytes).map_err(|e| PluginError::Invalid(format!("{e:#}")))?;
        let mut imports = Vec::new();
        for import in module.imports() {
            let cap = (import.module() == HOST_MODULE)
                .then(|| Capability::for_import(import.name()))
                .flatten()
                .ok_or_else(|| {
                    PluginError::ForbiddenImport(format!("{}::{}", import.module(), import.name()))
                })?;
            if !imports.contains(&cap) {
                imports.push(cap);
            }
        }
        for name in om_plugin_api::EXPORTS {
            let ok = match (name, module.get_export(name)) {
                ("memory", Some(ExternType::Memory(_))) => true,
                ("memory", _) | (_, None) => false,
                (_, Some(ExternType::Func(_))) => true,
                _ => false,
            };
            if !ok {
                return Err(PluginError::MissingExport(name.into()));
            }
        }
        let mut probe = Plugin {
            manifest: Manifest {
                name: String::new(),
                version: String::new(),
                abi: ABI_VERSION,
                capabilities: imports.clone(),
                params: Vec::new(),
            },
            imports: imports.clone(),
            module,
        };
        let mut inst = self.instantiate(&probe)?;
        let version = inst.abi_version()?;
        if version != ABI_VERSION {
            return Err(PluginError::Manifest(format!(
                "plugin targets ABI {version}; this OpenMapper supports ABI {ABI_VERSION}"
            )));
        }
        let manifest = Manifest::parse(&inst.manifest_bytes()?)
            .map_err(|e| PluginError::Manifest(e.to_string()))?;
        for cap in &imports {
            if !manifest.capabilities.contains(cap) {
                return Err(PluginError::UndeclaredCapability(cap.import().into()));
            }
        }
        probe.manifest = manifest;
        Ok(probe)
    }

    /// A fresh instance of `plugin` with this host's limits.
    pub fn instantiate(&self, plugin: &Plugin) -> Result<PluginInstance, PluginError> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let state = HostState {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory)
                .memories(1)
                .tables(4)
                .instances(1)
                .build(),
            log: Arc::clone(&log),
            time: 0.0,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.epoch_deadline_trap();
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        if plugin.imports.contains(&Capability::Log) {
            linker
                .func_wrap(
                    HOST_MODULE,
                    "log",
                    |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                        let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory())
                        else {
                            return;
                        };
                        let (Ok(ptr), Ok(len)) = (usize::try_from(ptr), usize::try_from(len))
                        else {
                            return;
                        };
                        let len = len.min(4096);
                        let mut buf = vec![0u8; len];
                        if mem.read(&caller, ptr, &mut buf).is_ok() {
                            let text = String::from_utf8_lossy(&buf).into_owned();
                            if let Ok(mut log) = caller.data().log.lock()
                                && log.len() < MAX_LOG
                            {
                                log.push(text);
                            }
                        }
                    },
                )
                .map_err(|e| PluginError::Invalid(e.to_string()))?;
        }
        if plugin.imports.contains(&Capability::Time) {
            linker
                .func_wrap(HOST_MODULE, "time", |caller: Caller<'_, HostState>| {
                    caller.data().time
                })
                .map_err(|e| PluginError::Invalid(e.to_string()))?;
        }
        store
            .set_fuel(self.limits.fuel)
            .map_err(|e| PluginError::Invalid(e.to_string()))?;
        store.set_epoch_deadline(deadline_ticks(self.limits.deadline));
        let instance = linker
            .instantiate(&mut store, &plugin.module)
            .map_err(|e| trap_error(&e, &self.limits))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| PluginError::MissingExport("memory".into()))?;
        let typed = |name: &str| PluginError::MissingExport(name.into());
        let abi = instance
            .get_typed_func::<(), i32>(&mut store, "om_abi_version")
            .map_err(|_| typed("om_abi_version"))?;
        let manifest = instance
            .get_typed_func::<(), i64>(&mut store, "om_manifest")
            .map_err(|_| typed("om_manifest"))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "om_alloc")
            .map_err(|_| typed("om_alloc"))?;
        let process = instance
            .get_typed_func::<(i32, i32, i32, i32, i32, i32), i32>(&mut store, "om_process")
            .map_err(|_| typed("om_process"))?;
        Ok(PluginInstance {
            store,
            _instance: instance,
            memory,
            abi,
            manifest,
            alloc,
            process,
            buffers: None,
            log,
            limits: self.limits,
            faulted: false,
        })
    }
}

fn deadline_ticks(d: Duration) -> u64 {
    let ticks = d.as_millis() / TICK.as_millis().max(1);
    u64::try_from(ticks).unwrap_or(u64::MAX).max(1) + 1
}

/// One running plugin instance (single-threaded).
pub struct PluginInstance {
    store: Store<HostState>,
    _instance: Instance,
    memory: Memory,
    abi: TypedFunc<(), i32>,
    manifest: TypedFunc<(), i64>,
    alloc: TypedFunc<i32, i32>,
    process: TypedFunc<(i32, i32, i32, i32, i32, i32), i32>,
    /// (pixel count, params count, in, out, params) of the current buffers.
    buffers: Option<(usize, usize, i32, i32, i32)>,
    log: Arc<Mutex<Vec<String>>>,
    limits: Limits,
    faulted: bool,
}

impl std::fmt::Debug for PluginInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginInstance")
            .field("faulted", &self.faulted)
            .finish_non_exhaustive()
    }
}

impl PluginInstance {
    fn arm(&mut self) -> Result<(), PluginError> {
        if self.faulted {
            return Err(PluginError::Trap("instance faulted earlier".into()));
        }
        self.store
            .set_fuel(self.limits.fuel)
            .map_err(|e| PluginError::Trap(e.to_string()))?;
        self.store
            .set_epoch_deadline(deadline_ticks(self.limits.deadline));
        Ok(())
    }

    fn fault(&mut self, e: &wasmtime::Error) -> PluginError {
        self.faulted = true;
        trap_error(e, &self.limits)
    }

    fn abi_version(&mut self) -> Result<i32, PluginError> {
        self.arm()?;
        let f = self.abi.clone();
        f.call(&mut self.store, ()).map_err(|e| self.fault(&e))
    }

    fn manifest_bytes(&mut self) -> Result<Vec<u8>, PluginError> {
        self.arm()?;
        let f = self.manifest.clone();
        let packed = f.call(&mut self.store, ()).map_err(|e| self.fault(&e))?;
        #[allow(clippy::cast_sign_loss)]
        let packed = packed as u64;
        let ptr = usize::try_from(packed >> 32).map_err(|_| PluginError::BadFrame)?;
        let len = usize::try_from(packed & 0xffff_ffff).map_err(|_| PluginError::BadFrame)?;
        if len > 64 * 1024 {
            return Err(PluginError::Manifest(
                "manifest is larger than 64 KB".into(),
            ));
        }
        let mut buf = vec![0u8; len];
        self.memory
            .read(&self.store, ptr, &mut buf)
            .map_err(|_| PluginError::Manifest("manifest lies outside plugin memory".into()))?;
        Ok(buf)
    }

    fn alloc(&mut self, size: usize) -> Result<i32, PluginError> {
        let size = i32::try_from(size).map_err(|_| PluginError::BadFrame)?;
        self.arm()?;
        let f = self.alloc.clone();
        let ptr = f.call(&mut self.store, size).map_err(|e| self.fault(&e))?;
        if ptr <= 0 {
            return Err(PluginError::OutOfMemory(self.limits.memory >> 20));
        }
        Ok(ptr)
    }

    /// Runs the effect on one sRGB RGBA8 frame. After an error other than
    /// [`PluginError::Failed`] the instance is unusable; create a new one.
    pub fn process(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        params: &[f32],
        time: f64,
    ) -> Result<Vec<u8>, PluginError> {
        let pixels = u64::from(width) * u64::from(height);
        if width == 0 || height == 0 || pixels > MAX_PIXELS || rgba.len() as u64 != pixels * 4 {
            return Err(PluginError::BadFrame);
        }
        let n = rgba.len();
        let (inp, out, par) = match self.buffers {
            Some((len, np, i, o, p)) if len == n && np >= params.len() => (i, o, p),
            _ => {
                let i = self.alloc(n)?;
                let o = self.alloc(n)?;
                let p = self.alloc((params.len() * 4).max(4))?;
                self.buffers = Some((n, params.len(), i, o, p));
                (i, o, p)
            }
        };
        let at = |p: i32| usize::try_from(p).map_err(|_| PluginError::BadFrame);
        self.memory
            .write(&mut self.store, at(inp)?, rgba)
            .map_err(|_| PluginError::Trap("input buffer outside plugin memory".into()))?;
        let mut pbytes = Vec::with_capacity(params.len() * 4);
        for p in params {
            pbytes.extend_from_slice(&p.to_le_bytes());
        }
        self.memory
            .write(&mut self.store, at(par)?, &pbytes)
            .map_err(|_| PluginError::Trap("parameter buffer outside plugin memory".into()))?;
        self.store.data_mut().time = time;
        self.arm()?;
        let f = self.process.clone();
        let (w, h) = (
            i32::try_from(width).map_err(|_| PluginError::BadFrame)?,
            i32::try_from(height).map_err(|_| PluginError::BadFrame)?,
        );
        let np = i32::try_from(params.len()).map_err(|_| PluginError::BadFrame)?;
        let code = f
            .call(&mut self.store, (inp, w, h, par, np, out))
            .map_err(|e| self.fault(&e))?;
        if code != 0 {
            return Err(PluginError::Failed(code));
        }
        let mut result = vec![0u8; n];
        self.memory
            .read(&self.store, at(out)?, &mut result)
            .map_err(|_| PluginError::Trap("output buffer outside plugin memory".into()))?;
        Ok(result)
    }

    /// Messages logged so far (bounded), drained.
    pub fn take_log(&self) -> Vec<String> {
        self.log
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }
}

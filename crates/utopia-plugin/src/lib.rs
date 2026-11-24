use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};
use utopia_core::PageId;
use utopia_store::Store;
use wasmi::{Caller, Config, Engine, Linker, Module, StoreLimits, StoreLimitsBuilder};

pub const FUEL_PER_RUN: u64 = 50_000_000;
pub const MEMORY_LIMIT: usize = 16 << 20;
pub const MAX_HOST_CALLS: usize = 1_000;
const MAX_MESSAGE: usize = 4 << 20;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Command {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub commands: Vec<Command>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default = "default_module")]
    pub module: String,
}

fn default_module() -> String {
    "plugin.wasm".into()
}

impl Manifest {
    pub fn can_read(&self) -> bool {
        self.permissions.iter().any(|p| p == "read" || p == "write")
    }

    pub fn can_write(&self) -> bool {
        self.permissions.iter().any(|p| p == "write")
    }
}

#[derive(Debug)]
pub enum PluginError {
    Manifest(String),
    Module(String),
    Run(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::Manifest(m) => write!(f, "manifest: {m}"),
            PluginError::Module(m) => write!(f, "module: {m}"),
            PluginError::Run(m) => write!(f, "run: {m}"),
        }
    }
}

impl std::error::Error for PluginError {}

pub type Result<T, E = PluginError> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunOutput {
    pub insert: Option<String>,
    pub replace: Option<String>,
    pub message: Option<String>,
    pub raw: Value,
}

#[derive(Debug)]
pub struct RunResult {
    pub outcome: Result<RunOutput, PluginError>,
    pub changed: Vec<PageId>,
    pub logs: Vec<String>,
    pub fuel_used: u64,
}

impl RunResult {
    fn not_started(e: PluginError) -> Self {
        Self {
            outcome: Err(e),
            changed: Vec::new(),
            logs: Vec::new(),
            fuel_used: 0,
        }
    }

    pub fn output(&self) -> Option<&RunOutput> {
        self.outcome.as_ref().ok()
    }
}

pub struct Plugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    engine: Engine,
    module: Module,
}

struct Host {
    store: *mut Store,
    manifest: Manifest,
    limits: StoreLimits,
    calls: usize,
    changed: Vec<PageId>,
    logs: Vec<String>,
}

fn pack(ptr: u32, len: u32) -> i64 {
    ((u64::from(ptr) << 32) | u64::from(len)) as i64
}

fn unpack(v: i64) -> (usize, usize) {
    let v = v as u64;
    ((v >> 32) as usize, (v & 0xffff_ffff) as usize)
}

fn read_mem(caller: &Caller<'_, Host>, ptr: i32, len: i32) -> Result<Vec<u8>, wasmi::Error> {
    let (ptr, len) = (ptr as u32 as usize, len as u32 as usize);
    if len > MAX_MESSAGE {
        return Err(wasmi::Error::new("message too large"));
    }
    let mem = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmi::Error::new("plugin exports no memory"))?;
    let data = mem.data(caller);
    data.get(
        ptr..ptr
            .checked_add(len)
            .ok_or_else(|| wasmi::Error::new("bad pointer"))?,
    )
    .map(<[u8]>::to_vec)
    .ok_or_else(|| wasmi::Error::new("pointer out of bounds"))
}

fn write_back(caller: &mut Caller<'_, Host>, bytes: &[u8]) -> Result<i64, wasmi::Error> {
    let alloc = caller
        .get_export("utopia_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmi::Error::new("plugin exports no utopia_alloc"))?
        .typed::<i32, i32>(&caller)?;
    let ptr = alloc.call(&mut *caller, bytes.len() as i32)? as u32 as usize;
    let mem = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmi::Error::new("plugin exports no memory"))?;
    let data = mem.data_mut(&mut *caller);
    data.get_mut(ptr..ptr + bytes.len())
        .ok_or_else(|| wasmi::Error::new("utopia_alloc returned memory out of bounds"))?
        .copy_from_slice(bytes);
    Ok(pack(ptr as u32, bytes.len() as u32))
}

fn host_request(host: &mut Host, request: &[u8]) -> Value {
    host.calls += 1;
    if host.calls > MAX_HOST_CALLS {
        return json!({ "error": "too many host calls" });
    }
    let op: utopia_api::Op = match serde_json::from_slice(request) {
        Ok(op) => op,
        Err(e) => return json!({ "error": format!("bad request: {e}") }),
    };
    let kind = op.kind();
    let allowed = match kind {
        utopia_api::OpKind::Write => host.manifest.can_write(),
        utopia_api::OpKind::Read => host.manifest.can_read(),
    };
    if !allowed {
        let need = if kind == utopia_api::OpKind::Write {
            "write"
        } else {
            "read"
        };
        return json!({ "error": format!("permission denied: the plugin did not ask for {need:?}") });
    }
    // SAFETY: `store` is the `&mut Store` borrowed by `Plugin::run` for the whole run;
    // host functions only execute inside that call, on this thread.
    let store = unsafe { &mut *host.store };
    match utopia_api::handle(store, op) {
        Ok(o) => {
            host.changed.extend(o.changed);
            json!({ "ok": o.body })
        }
        Err(e) => json!({ "error": e.message, "status": e.status }),
    }
}

impl Plugin {
    pub fn load(dir: &Path) -> Result<Self> {
        let raw = std::fs::read(dir.join("plugin.json"))
            .map_err(|e| PluginError::Manifest(format!("{}: {e}", dir.display())))?;
        let manifest: Manifest =
            serde_json::from_slice(&raw).map_err(|e| PluginError::Manifest(e.to_string()))?;
        if manifest.commands.is_empty() {
            return Err(PluginError::Manifest("no commands".into()));
        }
        let rel = Path::new(&manifest.module);
        if rel.is_absolute() || rel.components().any(|c| c.as_os_str() == "..") {
            return Err(PluginError::Manifest(
                "module path escapes the plugin".into(),
            ));
        }
        let bytes = std::fs::read(dir.join(rel))
            .map_err(|e| PluginError::Module(format!("{}: {e}", manifest.module)))?;
        let wasm = if manifest.module.ends_with(".wat") {
            wat::parse_bytes(&bytes)
                .map_err(|e| PluginError::Module(e.to_string()))?
                .into_owned()
        } else {
            bytes
        };
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module =
            Module::new(&engine, &wasm[..]).map_err(|e| PluginError::Module(e.to_string()))?;
        Ok(Self {
            manifest,
            dir: dir.to_path_buf(),
            engine,
            module,
        })
    }

    pub fn run(&self, store: &mut Store, command: &str, input: Value) -> RunResult {
        if !self.manifest.commands.iter().any(|c| c.id == command) {
            return RunResult::not_started(PluginError::Run(format!("no command {command:?}")));
        }
        let host = Host {
            store: store as *mut Store,
            manifest: self.manifest.clone(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT)
                .instances(1)
                .build(),
            calls: 0,
            changed: Vec::new(),
            logs: Vec::new(),
        };
        let mut wstore = wasmi::Store::new(&self.engine, host);
        wstore.limiter(|h| &mut h.limits);
        if let Err(e) = wstore.set_fuel(FUEL_PER_RUN) {
            return RunResult::not_started(PluginError::Run(e.to_string()));
        }
        let mut linker = Linker::<Host>::new(&self.engine);
        if let Err(e) = link(&mut linker) {
            return RunResult::not_started(e);
        }
        let outcome = self.execute(&mut linker, &mut wstore, command, input);
        let fuel_used = fuel_used(wstore.get_fuel().ok());
        let host = wstore.into_data();
        RunResult {
            outcome,
            changed: host.changed,
            logs: host.logs,
            fuel_used,
        }
    }

    fn execute(
        &self,
        linker: &mut Linker<Host>,
        wstore: &mut wasmi::Store<Host>,
        command: &str,
        input: Value,
    ) -> Result<RunOutput> {
        let run_err = |e: wasmi::Error| PluginError::Run(e.to_string());
        let instance = linker
            .instantiate(&mut *wstore, &self.module)
            .and_then(|pre| pre.start(&mut *wstore))
            .map_err(run_err)?;
        let mut payload = json!({ "command": command });
        if let (Some(obj), Value::Object(extra)) = (payload.as_object_mut(), input) {
            obj.extend(extra);
        }
        let input = serde_json::to_vec(&payload).expect("json");
        let alloc = instance
            .get_typed_func::<i32, i32>(&*wstore, "utopia_alloc")
            .map_err(|_| PluginError::Module("missing export utopia_alloc".into()))?;
        let run = instance
            .get_typed_func::<(i32, i32), i64>(&*wstore, "utopia_run")
            .map_err(|_| PluginError::Module("missing export utopia_run".into()))?;
        let memory = instance
            .get_memory(&*wstore, "memory")
            .ok_or_else(|| PluginError::Module("missing export memory".into()))?;
        let ptr = alloc
            .call(&mut *wstore, input.len() as i32)
            .map_err(run_err)? as u32 as usize;
        memory
            .data_mut(&mut *wstore)
            .get_mut(ptr..ptr + input.len())
            .ok_or_else(|| PluginError::Run("utopia_alloc returned memory out of bounds".into()))?
            .copy_from_slice(&input);
        let packed = run
            .call(&mut *wstore, (ptr as i32, input.len() as i32))
            .map_err(run_err)?;
        let (optr, olen) = unpack(packed);
        if olen > MAX_MESSAGE {
            return Err(PluginError::Run("output too large".into()));
        }
        let out_bytes = memory
            .data(&*wstore)
            .get(optr..optr + olen)
            .ok_or_else(|| PluginError::Run("output out of bounds".into()))?
            .to_vec();
        let out: Value = if out_bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&out_bytes)
                .map_err(|e| PluginError::Run(format!("output is not JSON: {e}")))?
        };
        let text = |k: &str| out.get(k).and_then(Value::as_str).map(str::to_string);
        Ok(RunOutput {
            insert: text("insert"),
            replace: text("replace"),
            message: text("message"),
            raw: out,
        })
    }
}

fn fuel_used(left: Option<u64>) -> u64 {
    left.map_or(0, |left| FUEL_PER_RUN - left)
}

fn link(linker: &mut Linker<Host>) -> Result<()> {
    linker
        .func_wrap(
            "utopia",
            "call",
            |mut caller: Caller<'_, Host>, ptr: i32, len: i32| -> Result<i64, wasmi::Error> {
                let req = read_mem(&caller, ptr, len)?;
                let resp = host_request(caller.data_mut(), &req);
                let bytes = serde_json::to_vec(&resp).expect("json");
                write_back(&mut caller, &bytes)
            },
        )
        .map_err(|e| PluginError::Module(e.to_string()))?;
    linker
        .func_wrap(
            "utopia",
            "log",
            |mut caller: Caller<'_, Host>, ptr: i32, len: i32| -> Result<(), wasmi::Error> {
                let line = read_mem(&caller, ptr, len)?;
                let logs = &mut caller.data_mut().logs;
                if logs.len() < 1_000 {
                    logs.push(String::from_utf8_lossy(&line).into_owned());
                }
                Ok(())
            },
        )
        .map_err(|e| PluginError::Module(e.to_string()))?;
    Ok(())
}

pub fn load_all(root: &Path) -> Vec<(PathBuf, Result<Plugin>)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("plugin.json").exists())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| (d.clone(), Plugin::load(&d)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fuel_is_not_a_whole_budget() {
        assert_eq!(fuel_used(None), 0, "unknown is not \"all of it\"");
        assert_eq!(fuel_used(Some(FUEL_PER_RUN)), 0);
        assert_eq!(fuel_used(Some(FUEL_PER_RUN - 1_234)), 1_234);
    }
}

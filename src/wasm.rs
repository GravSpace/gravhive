use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};
use wasmi::{Caller, Engine, Func, Linker, Module, Store};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub path: String,
    pub enabled: bool,
}

pub struct LoadedPlugin {
    pub name: String,
    pub path: PathBuf,
}

pub struct WasmPluginManager {
    plugins: Arc<Mutex<Vec<LoadedPlugin>>>,
    engine: Engine,
}

impl WasmPluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(Mutex::new(Vec::new())),
            engine: Engine::default(),
        }
    }

    pub fn load_directory(&self, dir: &Path) -> Result<usize> {
        if !dir.exists() {
            let _ = fs::create_dir_all(dir);
            return Ok(0);
        }

        let mut loaded = 0;
        let entries = fs::read_dir(dir)?;

        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "wasm" || ext == "wat" {
                match self.load_plugin_file(&path) {
                    Ok(name) => {
                        info!("[WASM] Loaded plugin '{}' from {}", name, path.display());
                        loaded += 1;
                    }
                    Err(e) => {
                        error!("[WASM] Failed to load plugin {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(loaded)
    }

    pub fn load_plugin_file(&self, path: &Path) -> Result<String> {
        let bytes = fs::read(path)?;
        let module = Module::new(&self.engine, &bytes[..])
            .with_context(|| format!("Failed to parse WASM module: {}", path.display()))?;

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
            .to_string();

        let mut store = Store::new(&self.engine, ());
        let mut linker = <Linker<()>>::new(&self.engine);

        // Define a host logging function callable from WASM: host_log(i32, i32)
        let host_log = Func::wrap(&mut store, |_caller: Caller<()>, ptr: i32, len: i32| {
            info!(
                "[WASM-HOST] host_log called with ptr: {}, len: {}",
                ptr, len
            );
        });
        let _ = linker.define("env", "host_log", host_log);

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .with_context(|| "Failed to instantiate WASM plugin")?;

        // Check if plugin exports an init or on_event function
        if let Ok(on_event) = instance.get_typed_func::<i32, i32>(&store, "on_event") {
            let res = on_event.call(&mut store, 1);
            debug!(
                "Invoked on_event(1) on plugin '{}', result: {:?}",
                name, res
            );
        }

        let mut lock = self.plugins.lock().unwrap();
        lock.push(LoadedPlugin {
            name: name.clone(),
            path: path.to_path_buf(),
        });

        Ok(name)
    }

    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        let lock = self.plugins.lock().unwrap();
        lock.iter()
            .map(|p| PluginInfo {
                name: p.name.clone(),
                path: p.path.display().to_string(),
                enabled: true,
            })
            .collect()
    }
}

impl Default for WasmPluginManager {
    fn default() -> Self {
        Self::new()
    }
}

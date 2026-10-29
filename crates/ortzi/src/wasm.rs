use std::{
    io::Write,
    path::{Component as PathPart, Path, PathBuf},
    process::Stdio as ProcessStdio,
};

use anyhow::{Context, bail};
use registry_api::Permissions;
use wasmtime::{
    Engine, Store,
    component::{Component, HasSelf, Linker, ResourceTable},
};
use wasmtime_wasi::{I32Exit, WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    path: "../../packages/ortzi-host/wit",
    world: "ortzi:host/cli",
    imports: {
        "ortzi:host/env.exit": trappable,
    },
    anyhow: true,
});

use ortzi::host::{env, fs, process};

pub struct Sandbox {
    pub args: Vec<String>,
    pub vars: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub exec: Vec<String>,
    pub roots: Vec<PathBuf>,
}

impl Sandbox {
    pub fn new(
        package: &str,
        permissions: &Permissions,
        args: Vec<String>,
        extra_vars: Vec<(String, String)>,
    ) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()?;
        let home = dirs::home_dir();
        let roots = permissions
            .fs
            .iter()
            .filter_map(|root| {
                let path = match root.as_str() {
                    "." => cwd.clone(),
                    "~" => home.clone()?,
                    other => match other.strip_prefix("~/") {
                        Some(rest) => home.as_ref()?.join(rest),
                        None => PathBuf::from(other),
                    },
                };
                path.canonicalize().ok()
            })
            .collect();

        let mut vars: Vec<(String, String)> = permissions
            .env
            .iter()
            .filter_map(|name| Some((name.clone(), std::env::var(name).ok()?)))
            .collect();
        vars.extend(extra_vars);
        vars.push(("ORTZI".to_owned(), "1".to_owned()));
        vars.push(("ORTZI_PACKAGE".to_owned(), package.to_owned()));

        let mut argv = vec![package.to_owned()];
        argv.extend(args);
        Ok(Self {
            args: argv,
            vars,
            cwd,
            exec: permissions.exec.clone(),
            roots,
        })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let raw = match path.strip_prefix("~/") {
            Some(rest) => dirs::home_dir()
                .ok_or("no home directory")?
                .join(rest),
            None => self.cwd.join(path),
        };

        let mut existing = raw.as_path();
        let mut missing = Vec::new();
        let base = loop {
            match existing.canonicalize() {
                Ok(base) => break base,
                Err(_) => {
                    let name = existing
                        .file_name()
                        .ok_or_else(|| format!("{path}: no such directory"))?;
                    missing.push(name.to_owned());
                    existing = existing
                        .parent()
                        .ok_or_else(|| format!("{path}: no such directory"))?;
                }
            }
        };
        let mut resolved = base;
        for part in missing.iter().rev() {
            if Path::new(part)
                .components()
                .any(|c| !matches!(c, PathPart::Normal(_)))
            {
                return Err(format!("{path}: invalid path"));
            }
            resolved.push(part);
        }

        if self.roots.iter().any(|root| resolved.starts_with(root)) {
            Ok(resolved)
        } else {
            Err(format!(
                "{path}: outside the package's permitted paths (permissions.fs)"
            ))
        }
    }

    fn program(&self, program: &str) -> Result<PathBuf, String> {
        if !self.exec.iter().any(|allowed| allowed == program) {
            return Err(format!(
                "{program} is not in the package's permissions.exec"
            ));
        }
        crate::dispatch::which(program).ok_or_else(|| format!("{program}: command not found"))
    }
}

struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    sandbox: Sandbox,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn stdio(mode: process::Stdio) -> ProcessStdio {
    match mode {
        process::Stdio::Inherit => ProcessStdio::inherit(),
        process::Stdio::Piped => ProcessStdio::piped(),
        process::Stdio::Null => ProcessStdio::null(),
    }
}

impl process::Host for State {
    fn run(&mut self, command: process::Command) -> Result<process::Output, String> {
        let program = self.sandbox.program(&command.program)?;
        let cwd = match &command.cwd {
            Some(dir) => self.sandbox.cwd.join(dir),
            None => self.sandbox.cwd.clone(),
        };

        let mut child = std::process::Command::new(&program)
            .args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .env_remove("ORTZI_TOKEN")
            .current_dir(&cwd)
            .stdin(if command.stdin.is_some() {
                ProcessStdio::piped()
            } else {
                ProcessStdio::inherit()
            })
            .stdout(stdio(command.stdout))
            .stderr(stdio(command.stderr))
            .spawn()
            .map_err(|e| format!("{}: {e}", command.program))?;

        if let Some(input) = command.stdin
            && let Some(mut pipe) = child.stdin.take()
        {
            std::thread::spawn(move || {
                let _ = pipe.write_all(&input);
            });
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("{}: {e}", command.program))?;
        Ok(process::Output {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

impl fs::Host for State {
    fn read_file(&mut self, path: String) -> Result<Vec<u8>, String> {
        let resolved = self.sandbox.resolve(&path)?;
        std::fs::read(resolved).map_err(|e| format!("{path}: {e}"))
    }

    fn write_file(&mut self, path: String, contents: Vec<u8>) -> Result<(), String> {
        let resolved = self.sandbox.resolve(&path)?;
        std::fs::write(resolved, contents).map_err(|e| format!("{path}: {e}"))
    }

    fn read_dir(&mut self, path: String) -> Result<Vec<fs::Entry>, String> {
        let resolved = self.sandbox.resolve(&path)?;
        let mut entries = std::fs::read_dir(resolved)
            .and_then(|dir| {
                dir.map(|entry| {
                    let entry = entry?;
                    Ok(fs::Entry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        dir: entry.file_type()?.is_dir(),
                    })
                })
                .collect::<std::io::Result<Vec<_>>>()
            })
            .map_err(|e| format!("{path}: {e}"))?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn exists(&mut self, path: String) -> Result<bool, String> {
        Ok(self.sandbox.resolve(&path)?.exists())
    }

    fn create_dir_all(&mut self, path: String) -> Result<(), String> {
        let resolved = self.sandbox.resolve(&path)?;
        std::fs::create_dir_all(resolved).map_err(|e| format!("{path}: {e}"))
    }

    fn remove(&mut self, path: String) -> Result<(), String> {
        let resolved = self.sandbox.resolve(&path)?;
        if self.sandbox.roots.contains(&resolved) {
            return Err(format!("{path}: refusing to remove a permitted root"));
        }
        let result = if resolved.is_dir() {
            std::fs::remove_dir_all(&resolved)
        } else {
            std::fs::remove_file(&resolved)
        };
        result.map_err(|e| format!("{path}: {e}"))
    }
}

impl env::Host for State {
    fn args(&mut self) -> Vec<String> {
        self.sandbox.args.clone()
    }

    fn vars(&mut self) -> Vec<(String, String)> {
        self.sandbox.vars.clone()
    }

    fn cwd(&mut self) -> String {
        self.sandbox.cwd.display().to_string()
    }

    fn exit(&mut self, code: u8) -> anyhow::Result<()> {
        Err(I32Exit(code.into()).into())
    }
}

fn engine() -> anyhow::Result<Engine> {
    Ok(Engine::new(&wasmtime::Config::new())?)
}

fn precompiled(wasm: &Path) -> PathBuf {
    wasm.with_extension("cwasm")
}

pub fn prepare(wasm: &Path) -> anyhow::Result<()> {
    load(&engine()?, wasm).map(|_| ())
}

fn load(engine: &Engine, wasm: &Path) -> anyhow::Result<Component> {
    let cwasm = precompiled(wasm);
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if modified(&cwasm) >= modified(wasm) && cwasm.exists() {
        // SAFETY: ortzi wrote this file from a verified artifact; wasmtime rejects
        // artifacts from another version or configuration, and we recompile then.
        if let Ok(component) = unsafe { Component::deserialize_file(engine, &cwasm) } {
            return Ok(component);
        }
    }

    let bytes = std::fs::read(wasm).with_context(|| format!("reading {}", wasm.display()))?;
    let compiled = engine
        .precompile_component(&bytes)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("compiling {}", wasm.display()))?;
    let tmp = cwasm.with_extension(format!("cwasm.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &compiled).is_ok() {
        let _ = std::fs::rename(&tmp, &cwasm);
    }
    // SAFETY: produced by this engine just above.
    Ok(unsafe { Component::deserialize(engine, &compiled) }?)
}

pub fn run(wasm: &Path, sandbox: Sandbox) -> anyhow::Result<i32> {
    let engine = engine()?;
    let component = load(&engine, wasm)?;

    let mut linker = Linker::<State>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    Cli::add_to_linker::<_, HasSelf<State>>(&mut linker, |state| state)?;
    linker.define_unknown_imports_as_traps(&component)?;

    let wasi = WasiCtx::builder()
        .inherit_stdio()
        .args(&sandbox.args)
        .envs(&sandbox.vars)
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false)
        .build();
    let mut store = Store::new(
        &engine,
        State {
            wasi,
            table: ResourceTable::new(),
            sandbox,
        },
    );

    let cli = Cli::instantiate(&mut store, &component, &linker)?;
    match cli.wasi_cli_run().call_run(&mut store) {
        Ok(Ok(())) => Ok(0),
        Ok(Err(())) => Ok(1),
        Err(e) => match e.downcast_ref::<I32Exit>() {
            Some(exit) => Ok(exit.0),
            None => bail!("{e:?}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(root: &Path) -> Sandbox {
        Sandbox {
            args: vec![],
            vars: vec![],
            cwd: root.to_owned(),
            exec: vec!["echo".to_owned()],
            roots: vec![root.canonicalize().unwrap()],
        }
    }

    #[test]
    fn paths_stay_inside_roots() {
        let dir = std::env::temp_dir().join(format!("ortzi-wasm-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let sandbox = sandbox(&dir);

        assert!(sandbox.resolve("sub").is_ok());
        assert!(sandbox.resolve("new/deeper/file.txt").is_ok());
        assert!(sandbox.resolve("../escape").is_err());
        assert!(sandbox.resolve("sub/../../escape").is_err());
        assert!(sandbox.resolve("/etc/passwd").is_err());

        std::os::unix::fs::symlink("/etc", dir.join("link")).unwrap();
        assert!(sandbox.resolve("link/passwd").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exec_is_allowlisted() {
        let sandbox = sandbox(&std::env::temp_dir());
        assert!(sandbox.program("echo").is_ok());
        assert!(sandbox.program("sh").is_err());
    }
}

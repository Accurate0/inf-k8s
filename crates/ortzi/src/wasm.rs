use std::{
    collections::HashMap,
    io::{IsTerminal, Write},
    path::{Component as PathPart, Path, PathBuf},
    process::Stdio as ProcessStdio,
};

use anyhow::{Context, bail};
use registry_api::{KubeVerb, PackageAuth, Permissions};
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

use ortzi::host::{auth, env, fs, http as host_http, kube, process, serve};

pub struct Sandbox {
    pub args: Vec<String>,
    pub vars: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub exec: Vec<String>,
    pub roots: Vec<PathBuf>,
    pub kube: Vec<KubeVerb>,
    pub http: Vec<String>,
    pub listen: Vec<u16>,
    pub auth: Option<(String, PackageAuth)>,
}

impl Sandbox {
    pub fn new(
        package: &str,
        invoked: &str,
        permissions: &Permissions,
        auth: Option<PackageAuth>,
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
                canonical(&path).ok()
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

        let mut argv = vec![invoked.to_owned()];
        argv.extend(args);
        Ok(Self {
            args: argv,
            vars,
            cwd,
            exec: permissions.exec.clone(),
            roots,
            kube: permissions.kube.clone(),
            http: permissions.http.clone(),
            listen: permissions.listen.clone(),
            auth: auth.map(|auth| (package.to_owned(), auth)),
        })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let raw = match path.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().ok_or("no home directory")?.join(rest),
            None => self.cwd.join(path),
        };

        let resolved = canonical(&raw).map_err(|e| format!("{path}: {e}"))?;

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

    fn url(&self, raw: &str) -> Result<reqwest::Url, String> {
        let url = reqwest::Url::parse(raw).map_err(|e| format!("http: {raw}: {e}"))?;
        let host = url.host_str().unwrap_or_default();
        let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
        if url.scheme() != "https" && !(url.scheme() == "http" && local) {
            return Err(format!("http: {raw}: only https is allowed"));
        }
        if !self.http.iter().any(|allowed| allowed == host) {
            return Err(format!(
                "http: {host} is not in the package's permissions.http"
            ));
        }
        Ok(url)
    }
}

fn canonical(raw: &Path) -> Result<PathBuf, &'static str> {
    let mut existing = raw;
    let mut missing = Vec::new();
    let base = loop {
        match existing.canonicalize() {
            Ok(base) => break base,
            Err(_) => {
                missing.push(existing.file_name().ok_or("no such directory")?.to_owned());
                existing = existing.parent().ok_or("no such directory")?;
            }
        }
    };

    let mut resolved = base;
    for part in missing.iter().rev() {
        if Path::new(part)
            .components()
            .any(|c| !matches!(c, PathPart::Normal(_)))
        {
            return Err("invalid path");
        }
        resolved.push(part);
    }
    Ok(resolved)
}

struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    sandbox: Sandbox,
    kube: Option<Kube>,
    http: Option<Http>,
    server: Option<Server>,
    tokens: Option<tokio::runtime::Runtime>,
}

const MAX_REQUEST_BYTES: usize = 1 << 20;

/// One request at a time on 127.0.0.1, answered before the next is accepted.
struct Server {
    listener: std::net::TcpListener,
    pending: Option<std::net::TcpStream>,
}

impl Server {
    fn accept(&mut self) -> Result<serve::Request, String> {
        if self.pending.is_some() {
            return Err("serve: respond to the last request before accepting another".to_owned());
        }

        loop {
            let (mut stream, _) = self.listener.accept().map_err(|e| format!("serve: {e}"))?;
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));

            match read_request(&mut stream) {
                Ok(request) => {
                    self.pending = Some(stream);
                    return Ok(request);
                }
                Err(_) => {
                    let _ = write_response(&mut stream, 400, &[], b"bad request\n");
                }
            }
        }
    }

    fn respond(&mut self, response: serve::Response) -> Result<(), String> {
        let mut stream = self
            .pending
            .take()
            .ok_or("serve: no request is waiting for a response")?;

        write_response(
            &mut stream,
            response.status,
            &response.headers,
            &response.body,
        )
        .map_err(|e| format!("serve: {e}"))
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Result<serve::Request, String> {
    use std::io::Read;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let (head, method, path, headers) = loop {
        let read = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("connection closed".to_owned());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err("request too large".to_owned());
        }

        let mut parsed = [httparse::EMPTY_HEADER; 64];
        let mut request = httparse::Request::new(&mut parsed);
        if let httparse::Status::Complete(head) =
            request.parse(&buffer).map_err(|e| e.to_string())?
        {
            let headers: Vec<(String, String)> = request
                .headers
                .iter()
                .map(|h| {
                    (
                        h.name.to_owned(),
                        String::from_utf8_lossy(h.value).into_owned(),
                    )
                })
                .collect();
            break (
                head,
                request.method.unwrap_or_default().to_owned(),
                request.path.unwrap_or_default().to_owned(),
                headers,
            );
        }
    };

    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse().map_err(|_| "invalid content-length"))
        .transpose()?
        .unwrap_or(0);
    if head + length > MAX_REQUEST_BYTES {
        return Err("request too large".to_owned());
    }

    let mut body = buffer.split_off(head);
    while body.len() < length {
        let read = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("connection closed".to_owned());
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);

    Ok(serve::Request {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<()> {
    let reason = http::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("");

    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    ));

    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

struct Http {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl Http {
    fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .user_agent(concat!("ortzi/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { runtime, client })
    }

    fn send(
        &self,
        url: reqwest::Url,
        request: host_http::Request,
    ) -> Result<host_http::Response, String> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| format!("http: invalid method {}", request.method))?;
        let timeout = std::time::Duration::from_millis(request.timeout_ms.unwrap_or(30_000).into());
        let mut builder = self.client.request(method, url).timeout(timeout);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        self.runtime.block_on(async {
            let context = |e: reqwest::Error| format!("{} {}: {e}", request.method, request.url);
            let response = builder.send().await.map_err(context)?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect();
            let body = response.bytes().await.map_err(context)?;
            Ok(host_http::Response {
                status,
                headers,
                body: body.to_vec(),
            })
        })
    }
}

struct Kube {
    runtime: tokio::runtime::Runtime,
    clients: HashMap<Option<String>, ::kube::Client>,
}

impl Kube {
    fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            runtime,
            clients: HashMap::new(),
        })
    }

    fn client(&mut self, context: Option<String>) -> Result<::kube::Client, String> {
        if let Some(client) = self.clients.get(&context) {
            return Ok(client.clone());
        }

        let options = ::kube::config::KubeConfigOptions {
            context: context.clone(),
            ..Default::default()
        };
        let client = self.runtime.block_on(async {
            let config = match &context {
                Some(_) => ::kube::Config::from_kubeconfig(&options)
                    .await
                    .map_err(|e| e.to_string())?,
                None => ::kube::Config::infer().await.map_err(|e| e.to_string())?,
            };
            ::kube::Client::try_from(config).map_err(|e| e.to_string())
        })?;
        self.clients.insert(context, client.clone());
        Ok(client)
    }

    fn send(&mut self, request: kube::Request) -> Result<kube::Response, String> {
        let client = self.client(request.context)?;
        let mut builder = http::Request::builder()
            .method(request.method.as_str())
            .uri(&request.path);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let body = ::kube::client::Body::from(request.body.unwrap_or_default());
        let http_request = builder.body(body).map_err(|e| e.to_string())?;

        self.runtime.block_on(async {
            let response = client
                .send(http_request)
                .await
                .map_err(|e| format!("{} {}: {e}", request.method, request.path))?;
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .collect_bytes()
                .await
                .map_err(|e| e.to_string())?;
            Ok(kube::Response {
                status,
                body: body.to_vec(),
            })
        })
    }
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

impl kube::Host for State {
    fn send(&mut self, request: kube::Request) -> Result<kube::Response, String> {
        let verb = KubeVerb::for_method(&request.method)
            .ok_or_else(|| format!("kube: unsupported method {}", request.method))?;
        if !self.sandbox.kube.contains(&verb) {
            return Err(format!(
                "kube: {} is not in the package's permissions.kube",
                verb.as_str()
            ));
        }
        if !request.path.starts_with('/') {
            return Err(format!("kube: {}: path must start with /", request.path));
        }

        let kube = match &mut self.kube {
            Some(kube) => kube,
            None => self.kube.insert(Kube::new()?),
        };
        kube.send(request)
    }
}

impl host_http::Host for State {
    fn send(&mut self, request: host_http::Request) -> Result<host_http::Response, String> {
        let url = self.sandbox.url(&request.url)?;
        let http = match &mut self.http {
            Some(http) => http,
            None => self.http.insert(Http::new()?),
        };
        http.send(url, request)
    }
}

impl serve::Host for State {
    fn listen(&mut self, port: u16) -> Result<(), String> {
        if !self.sandbox.listen.contains(&port) {
            return Err(format!(
                "serve: port {port} is not in the package's permissions.listen"
            ));
        }
        if self.server.is_some() {
            return Err("serve: already listening".to_owned());
        }

        let listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| format!("serve: 127.0.0.1:{port}: {e}"))?;
        self.server = Some(Server {
            listener,
            pending: None,
        });
        Ok(())
    }

    fn accept(&mut self) -> Result<serve::Request, String> {
        self.server
            .as_mut()
            .ok_or("serve: call listen first")?
            .accept()
    }

    fn respond(&mut self, response: serve::Response) -> Result<(), String> {
        self.server
            .as_mut()
            .ok_or("serve: call listen first")?
            .respond(response)
    }
}

impl auth::Host for State {
    fn token(&mut self) -> Result<String, String> {
        let (package, auth) = self
            .sandbox
            .auth
            .as_ref()
            .ok_or("auth: the package declares no auth")?;

        let runtime = match &mut self.tokens {
            Some(runtime) => runtime,
            None => self.tokens.insert(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?,
            ),
        };

        runtime
            .block_on(crate::package_auth::token(package, auth))
            .map_err(|e| format!("auth: {e:#}"))
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

    fn stdin_is_terminal(&mut self) -> bool {
        std::io::stdin().is_terminal()
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
            kube: None,
            http: None,
            server: None,
            tokens: None,
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
            kube: vec![],
            http: vec!["grafana.ortzi.cloud".to_owned(), "localhost".to_owned()],
            listen: vec![],
            auth: None,
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
    fn roots_that_do_not_exist_yet_still_allow_writes() {
        let dir = std::env::temp_dir().join(format!("ortzi-wasm-root-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut sandbox = sandbox(&dir);
        sandbox.roots = vec![canonical(&dir.join("cache/pkg")).unwrap()];

        assert!(sandbox.resolve("cache/pkg/key.json").is_ok());
        assert!(sandbox.resolve("cache/other").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exec_is_allowlisted() {
        let sandbox = sandbox(&std::env::temp_dir());
        assert!(sandbox.program("echo").is_ok());
        assert!(sandbox.program("sh").is_err());
    }

    #[test]
    fn http_is_allowlisted() {
        let sandbox = sandbox(&std::env::temp_dir());
        assert!(sandbox.url("https://grafana.ortzi.cloud/api/user").is_ok());
        assert!(sandbox.url("http://localhost:3000/x").is_ok());
        assert!(sandbox.url("http://grafana.ortzi.cloud/api/user").is_err());
        assert!(sandbox.url("https://evil.example/api").is_err());
        assert!(
            sandbox
                .url("https://grafana.ortzi.cloud.evil.example/")
                .is_err()
        );
        assert!(sandbox.url("file:///etc/passwd").is_err());
    }

    #[test]
    fn serves_one_request_at_a_time() {
        use std::io::{Read, Write as _};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut server = Server {
            listener,
            pending: None,
        };

        let client = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .write_all(b"POST /token?x=1 HTTP/1.1\r\nOrigin: https://graph.ortzi.cloud\r\nContent-Length: 2\r\n\r\nhi")
                .unwrap();

            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });

        let request = server.accept().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/token?x=1");
        assert_eq!(request.body, b"hi");
        assert!(
            request
                .headers
                .contains(&("Origin".to_owned(), "https://graph.ortzi.cloud".to_owned()))
        );
        assert!(server.accept().is_err());

        server
            .respond(serve::Response {
                status: 200,
                headers: vec![
                    ("content-type".to_owned(), "application/json".to_owned()),
                    ("x-bad".to_owned(), "a\r\nset-cookie: x".to_owned()),
                ],
                body: b"{}".to_vec(),
            })
            .unwrap();
        assert!(
            server
                .respond(serve::Response {
                    status: 200,
                    headers: vec![],
                    body: vec![],
                })
                .is_err()
        );

        let response = client.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("content-type: application/json\r\n"));
        assert!(!response.contains("set-cookie"));
        assert!(response.ends_with("\r\n\r\n{}"));
    }
}

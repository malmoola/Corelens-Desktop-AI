//! The bridge between `corelens-desktop-ai --mcp` and the running app.
//!
//! A named pipe on Windows, a Unix socket elsewhere, one connection per
//! tool call, newline-delimited JSON on it (the stdio framing, as the MCP
//! specification recommends for a custom byte-stream transport).
//!
//! **Both ends prove who they are before anything else is said.** The app
//! writes a fresh random key to a file only this user can read each time
//! it starts listening, and deletes it when it stops. Each side then shows
//! an HMAC over the other's nonce: the proxy checks the app's proof before
//! it sends a request, so a process that took the pipe's name first learns
//! nothing; the app checks the proxy's before it runs a tool, so a process
//! that cannot read the key can do nothing. The key never crosses the pipe.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::server::{Backend, BoxFuture, Outcome};

const HELLO: &str = "corelens-mcp-bridge/1";
/// A line longer than this ends the connection: nothing legitimate is.
const MAX_LINE: u64 = 4 << 20;
const HANDSHAKE: Duration = Duration::from_secs(5);
/// Longer than any tool's own limit, so a slow device reports its own
/// timeout rather than this one.
pub const CALL_LIMIT: Duration = Duration::from_secs(120);
/// For a tool that waits on a person (an approval): longer than the
/// longest wait Settings allows, plus the change itself.
pub const LONG_CALL_LIMIT: Duration = Duration::from_secs(75 * 60);

pub const NOT_LISTENING: &str = "Corelens Desktop AI is not running, or \"Let Claude use Corelens\" is off in its Settings → AI. \
Start the app, open the customer's project, switch it on, and try again.";
pub const BUSY: &str = "Corelens Desktop AI is running, but every connection to it stayed busy for 10 seconds. \
Try again; if it keeps happening, ask for fewer things at once.";
pub const NOT_PROVEN: &str = "Whatever answered on the Corelens bridge could not prove it is this computer's Corelens Desktop AI, \
so nothing was sent to it. Close Corelens Desktop AI, start it again, and retry.";

/// Where the bridge is, and the file holding its key.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The socket path (Unix) or the pipe name (Windows).
    pub address: String,
    pub key_file: PathBuf,
}

/// The endpoint for the app whose data folder is `data_dir`.
pub fn endpoint(data_dir: &Path) -> Endpoint {
    let key_file = data_dir.join("mcp.key");
    #[cfg(windows)]
    {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_default()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        Endpoint { address: format!(r"\\.\pipe\corelens-desktop-ai-mcp-{user}"), key_file }
    }
    #[cfg(not(windows))]
    {
        Endpoint { address: data_dir.join("mcp.sock").to_string_lossy().into_owned(), key_file }
    }
}

type HmacSha256 = Hmac<Sha256>;

fn proof(key: &[u8], role: &str, first: &[u8], second: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(role.as_bytes());
    mac.update(first);
    mac.update(second);
    mac.finalize().into_bytes().to_vec()
}

fn verify(key: &[u8], role: &str, first: &[u8], second: &[u8], shown: &str) -> bool {
    let Ok(shown) = hex::decode(shown) else { return false };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(role.as_bytes());
    mac.update(first);
    mac.update(second);
    mac.verify_slice(&shown).is_ok()
}

fn nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    getrandom::getrandom(&mut n).expect("the operating system's random source");
    n
}

/// A new key, written where only this user can read it.
pub fn write_key(path: &Path) -> std::io::Result<Vec<u8>> {
    let key = nonce().to_vec();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    // On Windows the data folder is under the user's own profile, whose
    // permissions the file inherits.
    use std::io::Write;
    opts.open(path)?.write_all(hex::encode(&key).as_bytes())?;
    Ok(key)
}

pub fn read_key(path: &Path) -> Option<Vec<u8>> {
    let text = std::fs::read_to_string(path).ok()?;
    hex::decode(text.trim()).ok().filter(|k| k.len() == 32)
}

async fn read_line<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Option<Value> {
    let mut buf = Vec::new();
    let mut limited = (&mut *reader).take(MAX_LINE);
    let n = limited.read_until(b'\n', &mut buf).await.ok()?;
    if n == 0 || !buf.ends_with(b"\n") {
        return None;
    }
    serde_json::from_slice(&buf).ok()
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

/// The proxy's half of one call on an open stream.
pub async fn call_over<S: AsyncRead + AsyncWrite + Unpin>(stream: S, key: &[u8], tool: &str, arguments: Value, client: Option<&str>) -> Outcome {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mine = nonce();
    let handshake = async {
        write_line(&mut write, &json!({ "hello": HELLO, "nonce": hex::encode(mine) })).await.ok()?;
        let reply = read_line(&mut read).await?;
        let theirs = hex::decode(reply.get("nonce")?.as_str()?).ok()?;
        if !verify(key, "server", &mine, &theirs, reply.get("proof")?.as_str()?) {
            return Some(false);
        }
        write_line(&mut write, &json!({ "proof": hex::encode(proof(key, "client", &theirs, &mine)) })).await.ok()?;
        let ok = read_line(&mut read).await?;
        Some(ok.get("ok") == Some(&json!(true)))
    };
    match tokio::time::timeout(HANDSHAKE, handshake).await {
        Ok(Some(true)) => {}
        Ok(Some(false)) | Ok(None) | Err(_) => return Outcome::error(NOT_PROVEN),
    }
    if write_line(&mut write, &json!({ "tool": tool, "arguments": arguments, "client": client })).await.is_err() {
        return Outcome::error("The connection to Corelens Desktop AI closed before the request was sent. Try again.");
    }
    let limit = if crate::tools::find(tool).is_some_and(|t| t.long_running) { LONG_CALL_LIMIT } else { CALL_LIMIT };
    match tokio::time::timeout(limit, read_line(&mut read)).await {
        Ok(Some(v)) => serde_json::from_value(v).unwrap_or_else(|_| Outcome::error("Corelens Desktop AI sent an answer this version does not read.")),
        Ok(None) => Outcome::error("Corelens Desktop AI closed the connection without answering (it may have been closed). Try again."),
        Err(_) => Outcome::error(format!("Corelens Desktop AI did not answer within {} seconds.", limit.as_secs())),
    }
}

/// The app's half of one connection: prove, check, run one tool, answer.
pub async fn serve_over<S: AsyncRead + AsyncWrite + Unpin>(stream: S, key: &[u8], backend: &dyn Backend) {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let handshake = async {
        let hello = read_line(&mut read).await?;
        if hello.get("hello")?.as_str()? != HELLO {
            return None;
        }
        let theirs = hex::decode(hello.get("nonce")?.as_str()?).ok().filter(|n| n.len() == 32)?;
        let mine = nonce();
        write_line(&mut write, &json!({ "nonce": hex::encode(mine), "proof": hex::encode(proof(key, "server", &theirs, &mine)) })).await.ok()?;
        let answer = read_line(&mut read).await?;
        Some(verify(key, "client", &mine, &theirs, answer.get("proof")?.as_str()?))
    };
    match tokio::time::timeout(HANDSHAKE, handshake).await {
        Ok(Some(true)) => {}
        _ => return,
    }
    if write_line(&mut write, &json!({ "ok": true })).await.is_err() {
        return;
    }
    let Some(request) = read_line(&mut read).await else { return };
    let tool = request.get("tool").and_then(Value::as_str).unwrap_or("").to_string();
    let arguments = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let client = request.get("client").and_then(Value::as_str).map(|c| c.chars().take(100).collect::<String>());
    let outcome = backend.call(&tool, arguments, client.as_deref()).await;
    let _ = write_line(&mut write, &serde_json::to_value(&outcome).unwrap_or(Value::Null)).await;
}

/// The bridge as the proxy sees it: every tool call goes to the app.
pub struct Client {
    pub endpoint: Endpoint,
}

impl Backend for Client {
    fn call<'a>(&'a self, tool: &'a str, arguments: Value, client: Option<&'a str>) -> BoxFuture<'a, Outcome> {
        Box::pin(async move {
            // Read per call: the app writes a new key each time it starts.
            let Some(key) = read_key(&self.endpoint.key_file) else { return Outcome::error(NOT_LISTENING) };
            match connect(&self.endpoint.address).await {
                Ok(stream) => call_over(stream, &key, tool, arguments, client).await,
                Err(e) => {
                    if crate::server::debug() {
                        eprintln!("corelens-mcp: tools/call {tool}: the bridge did not connect: {e}");
                    }
                    Outcome::error(if is_busy(&e) { BUSY } else { NOT_LISTENING })
                }
            }
        })
    }
}

/// Every instance of the pipe taken: the app is there, only busy.
#[cfg(windows)]
const ERROR_PIPE_BUSY: i32 = 231;

fn is_busy(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(ERROR_PIPE_BUSY)
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

#[cfg(unix)]
async fn connect(address: &str) -> std::io::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(address).await
}

#[cfg(windows)]
async fn connect(address: &str) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    // Every instance busy: the app is there and opens the next one as soon
    // as its accept loop runs, so wait for it. A Unix socket queues the
    // client instead; a pipe turns it away. No pipe at all is answered at
    // once.
    let start = std::time::Instant::now();
    loop {
        match ClientOptions::new().open(address) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && start.elapsed() < Duration::from_secs(10) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            other => return other,
        }
    }
}

/// A listening bridge; stopping it removes the endpoint and the key.
pub struct Listening {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    endpoint: Endpoint,
}

impl Listening {
    pub async fn stop(mut self) {
        self.shut();
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }

    fn shut(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        let _ = std::fs::remove_file(&self.endpoint.key_file);
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint.address);
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.shut();
    }
}

/// Starts listening, with a fresh key. Each connection is served on its
/// own task, so a slow device does not hold up another call.
pub async fn listen(endpoint: Endpoint, backend: Arc<dyn Backend>) -> std::io::Result<Listening> {
    let key = Arc::new(write_key(&endpoint.key_file)?);
    let (stop, mut stopped) = oneshot::channel::<()>();
    let task = spawn_accept(&endpoint.address, key, backend, async move {
        let _ = (&mut stopped).await;
    })?;
    Ok(Listening { stop: Some(stop), task: Some(task), endpoint })
}

#[cfg(unix)]
fn spawn_accept(
    address: &str,
    key: Arc<Vec<u8>>,
    backend: Arc<dyn Backend>,
    stopped: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    use std::os::unix::fs::PermissionsExt;
    // A socket left by an app that did not close cleanly.
    let _ = std::fs::remove_file(address);
    let listener = tokio::net::UnixListener::bind(address)?;
    std::fs::set_permissions(address, std::fs::Permissions::from_mode(0o600))?;
    Ok(tokio::spawn(async move {
        tokio::pin!(stopped);
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let (key, backend) = (Arc::clone(&key), Arc::clone(&backend));
                    tokio::spawn(async move { serve_over(stream, &key, backend.as_ref()).await });
                }
            }
        }
    }))
}

#[cfg(windows)]
fn spawn_accept(
    address: &str,
    key: Arc<Vec<u8>>,
    backend: Arc<dyn Backend>,
    stopped: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let address = address.to_string();
    // The first instance claims the name: if another process holds it,
    // listening fails here rather than sharing the name with it.
    let mut server = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&address)?;
    Ok(tokio::spawn(async move {
        tokio::pin!(stopped);
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                connected = server.connect() => {
                    if connected.is_err() {
                        continue;
                    }
                    let next = match ServerOptions::new().reject_remote_clients(true).create(&address) {
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    let client = std::mem::replace(&mut server, next);
                    let (key, backend) = (Arc::clone(&key), Arc::clone(&backend));
                    tokio::spawn(async move { serve_over(client, &key, backend.as_ref()).await });
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named;
    impl Backend for Named {
        fn call<'a>(&'a self, tool: &'a str, arguments: Value, _client: Option<&'a str>) -> BoxFuture<'a, Outcome> {
            Box::pin(async move { Outcome::ok(format!("ran {tool} with {arguments}")) })
        }
    }

    fn key(b: u8) -> Vec<u8> {
        vec![b; 32]
    }

    #[tokio::test]
    async fn both_ends_with_the_key_run_a_call() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move { serve_over(b, &key(7), &Named).await });
        let out = call_over(a, &key(7), "get_device", json!({ "target": "SW1" }), None).await;
        server.await.unwrap();
        assert_eq!(out, Outcome::ok(r#"ran get_device with {"target":"SW1"}"#));
    }

    struct Who;
    impl Backend for Who {
        fn call<'a>(&'a self, _tool: &'a str, _arguments: Value, client: Option<&'a str>) -> BoxFuture<'a, Outcome> {
            Box::pin(async move { Outcome::ok(format!("client {}", client.unwrap_or("none"))) })
        }
    }

    #[tokio::test]
    async fn the_clients_name_crosses_with_the_call() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move { serve_over(b, &key(7), &Who).await });
        let out = call_over(a, &key(7), "corelens_status", json!({}), Some("Claude Code 2.1")).await;
        server.await.unwrap();
        assert_eq!(out, Outcome::ok("client Claude Code 2.1"));
    }

    #[tokio::test]
    async fn a_server_without_the_key_is_told_nothing() {
        // What a squatter on the pipe name would see.
        let (a, b) = tokio::io::duplex(1 << 16);
        let seen = tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(b);
            let mut r = BufReader::new(r);
            let hello = read_line(&mut r).await.unwrap();
            write_line(&mut w, &json!({ "nonce": hex::encode([1u8; 32]), "proof": hex::encode(proof(&key(9), "server", &hex::decode(hello["nonce"].as_str().unwrap()).unwrap(), &[1u8; 32])) })).await.unwrap();
            let mut rest = String::new();
            let _ = tokio::time::timeout(Duration::from_millis(300), r.read_to_string(&mut rest)).await;
            (hello, rest)
        });
        let out = call_over(a, &key(7), "get_config", json!({ "target": "SW1" }), None).await;
        assert_eq!(out, Outcome::error(NOT_PROVEN));
        let (hello, rest) = seen.await.unwrap();
        assert!(hello.get("tool").is_none() && !rest.contains("get_config") && !rest.contains("SW1"), "the request reached a server that had no key: {rest}");
    }

    #[tokio::test]
    async fn a_client_without_the_key_runs_nothing() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move { serve_over(b, &key(7), &Named).await });
        let out = call_over(a, &key(8), "list_devices", json!({}), None).await;
        server.await.unwrap();
        assert!(out.is_error, "{out:?}");
    }

    #[tokio::test]
    async fn the_key_file_is_this_users_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.key");
        let k = write_key(&path).unwrap();
        assert_eq!(read_key(&path).as_deref(), Some(k.as_slice()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let k2 = write_key(&path).unwrap();
        assert_ne!(k, k2, "a new key each start");
    }

    #[tokio::test]
    async fn listening_serves_and_stopping_removes_the_endpoint_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let ep = endpoint(dir.path());
        let client = Client { endpoint: ep.clone() };
        assert_eq!(client.call("corelens_status", json!({}), None).await, Outcome::error(NOT_LISTENING), "nothing listening yet");
        let running = listen(ep.clone(), Arc::new(Named)).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&ep.address).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let out = client.call("corelens_status", json!({}), None).await;
        assert_eq!(out, Outcome::ok("ran corelens_status with {}"));
        running.stop().await;
        assert!(!ep.key_file.exists());
        let start = std::time::Instant::now();
        assert_eq!(client.call("corelens_status", json!({}), None).await, Outcome::error(NOT_LISTENING));
        assert!(start.elapsed() < Duration::from_secs(1), "an absent app is reported at once, not after a wait");
    }
}

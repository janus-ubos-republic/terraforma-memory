//! The Gate: a remote-capable MCP door onto one estate, with one identity per
//! assistant. Each principal holds its own bearer token (only its SHA-256 is
//! stored), has a scope (`read` or `write`) and can be revoked on its own.
//!
//! The Gate is stateless: the principal file is read on every request, so a
//! revoke takes effect on the very next call, including for a client that is
//! "mid-session". It records who called what (never the content) in an audit
//! log. It binds to loopback only; to reach it from another machine the owner
//! puts their own TLS tunnel or reverse proxy in front. The estate is never
//! reachable without a valid token, and the Gate adds no outbound connections.
use crate::server::{parse, Request};
use crate::{mcp, now_utc, sha256, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_CLIENTS: usize = 4;
const PRINCIPALS: &str = "principals.json";
const AUDIT: &str = "audit.jsonl";
const SCHEMA: &str = "terraforma.gate.principals.v1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Read,
    Write,
}
impl Scope {
    fn parse(s: &str) -> Result<Scope> {
        match s {
            "read" => Ok(Scope::Read),
            "write" => Ok(Scope::Write),
            _ => Err("scope must be read or write".into()),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
        }
    }
}

fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn ensure_dir(dir: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

fn load(dir: &Path) -> Result<Value> {
    let path = dir.join(PRINCIPALS);
    if !path.exists() {
        return Ok(json!({"schema":SCHEMA,"principals":[]}));
    }
    let bytes = crate::read_bounded(&path, 1024 * 1024)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "principals file is damaged")?;
    if value["schema"] != SCHEMA || !value["principals"].is_array() {
        return Err("principals file has an unknown schema".into());
    }
    Ok(value)
}

fn store(dir: &Path, value: &Value) -> Result<()> {
    let tmp = dir.join(format!("{PRINCIPALS}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).unwrap())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(PRINCIPALS)).map_err(|e| e.to_string())
}

/// Create a principal and return its token. The token is shown once.
pub fn add(dir: &Path, name: &str, scope: &str) -> Result<Value> {
    if !valid_name(name) {
        return Err("name must be 1-32 characters of a-z, 0-9, - or _".into());
    }
    let scope = Scope::parse(scope)?;
    ensure_dir(dir)?;
    let mut doc = load(dir)?;
    let list = doc["principals"].as_array_mut().unwrap();
    if list.iter().any(|p| p["name"] == name) {
        return Err("principal exists (revoked names are not reused)".into());
    }
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let token = format!(
        "tfg_{}",
        random.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    list.push(json!({"name":name,"scope":scope.name(),
        "token_sha256":sha256(token.as_bytes()),"created":now_utc(),"revoked_at":null}));
    store(dir, &doc)?;
    audit(dir, name, "principal.add", "", "ok");
    Ok(json!({"status":"created","principal":name,"scope":scope.name(),"token":token,
        "note":"This token is shown once. Store it in the assistant's configuration."}))
}

pub fn revoke(dir: &Path, name: &str) -> Result<Value> {
    let mut doc = load(dir)?;
    let list = doc["principals"].as_array_mut().unwrap();
    let Some(p) = list.iter_mut().find(|p| p["name"] == name) else {
        return Err("no such principal".into());
    };
    if !p["revoked_at"].is_null() {
        return Ok(json!({"status":"already_revoked","principal":name}));
    }
    p["revoked_at"] = json!(now_utc());
    store(dir, &doc)?;
    audit(dir, name, "principal.revoke", "", "ok");
    Ok(json!({"status":"revoked","principal":name}))
}

pub fn list(dir: &Path) -> Result<Value> {
    let doc = load(dir)?;
    let rows: Vec<Value> = doc["principals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            json!({"name":p["name"],"scope":p["scope"],"created":p["created"],
                "revoked_at":p["revoked_at"]})
        })
        .collect();
    Ok(json!({"schema":SCHEMA,"principals":rows}))
}

fn audit(dir: &Path, principal: &str, action: &str, tool: &str, outcome: &str) {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(dir.join(AUDIT)) {
        let line = json!({"at":now_utc(),"principal":principal,"action":action,
            "tool":tool,"outcome":outcome});
        let _ = writeln!(file, "{line}");
    }
}

/// The principal whose live (not revoked) token matches, with its scope.
fn authenticate(dir: &Path, header: Option<&String>) -> Option<(String, Scope)> {
    let token = header?.strip_prefix("Bearer ")?.trim();
    if token.len() > 128 {
        return None;
    }
    let digest = sha256(token.as_bytes());
    let doc = load(dir).ok()?;
    doc["principals"].as_array()?.iter().find_map(|p| {
        let live = p["revoked_at"].is_null() && p["token_sha256"].as_str() == Some(digest.as_str());
        let scope = Scope::parse(p["scope"].as_str()?).ok()?;
        live.then(|| (p["name"].as_str().unwrap_or("").to_string(), scope))
    })
}

fn reply(stream: &mut TcpStream, status: u16, extra: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Service Unavailable",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\n\
Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\n{extra}\r\n",
        body.len()
    );
    let _ = stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body));
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}))
        .unwrap()
}

struct Gate {
    estate: PathBuf,
    dir: PathBuf,
}

fn handle(mut stream: TcpStream, gate: Arc<Gate>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request: Request = match parse(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            reply(&mut stream, 400, "", &rpc_error(&Value::Null, -32600, &e));
            return;
        }
    };
    if request.path != "/mcp" {
        reply(&mut stream, 404, "", &rpc_error(&Value::Null, -32600, "not found"));
        return;
    }
    // Browsers send Origin; an MCP assistant does not. Refusing it closes the
    // door on web pages trying to reach a local Gate.
    if request.headers.contains_key("origin") {
        audit(&gate.dir, "", "http", "", "refused_origin");
        reply(&mut stream, 403, "", &rpc_error(&Value::Null, -32600, "origin not allowed"));
        return;
    }
    let Some((principal, scope)) = authenticate(&gate.dir, request.headers.get("authorization"))
    else {
        audit(&gate.dir, "", "http", "", "unauthorized");
        reply(
            &mut stream,
            401,
            "WWW-Authenticate: Bearer realm=\"terraforma-gate\"\r\n",
            &rpc_error(&Value::Null, -32001, "a valid, unrevoked token is required"),
        );
        return;
    };
    if request.method != "POST" {
        reply(&mut stream, 405, "Allow: POST\r\n", &rpc_error(&Value::Null, -32600, "POST only"));
        return;
    }
    let body = match std::str::from_utf8(&request.body) {
        Ok(b) => b,
        Err(_) => {
            reply(&mut stream, 400, "", &rpc_error(&Value::Null, -32700, "parse error"));
            return;
        }
    };
    let message: Value = match serde_json::from_str::<Value>(body) {
        Ok(m) if m.is_object() => m,
        _ => {
            reply(
                &mut stream,
                400,
                "",
                &rpc_error(&Value::Null, -32600, "one JSON-RPC object per request"),
            );
            return;
        }
    };
    let method = message["method"].as_str().unwrap_or("").to_string();
    let tool = if method == "tools/call" {
        message["params"]["name"].as_str().unwrap_or("").to_string()
    } else {
        String::new()
    };
    if tool == "remember" && scope == Scope::Read {
        audit(&gate.dir, &principal, &method, &tool, "denied_read_only");
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let denied = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text",
            "text":"This assistant has read-only access to this memory."}],"isError":true}});
        reply(&mut stream, 200, "", &serde_json::to_vec(&denied).unwrap());
        return;
    }
    match mcp::handle(&gate.estate, body) {
        None => {
            audit(&gate.dir, &principal, &method, &tool, "ok");
            reply(&mut stream, 202, "", b"");
        }
        Some(value) => {
            let failed = value.get("error").is_some()
                || value["result"]["isError"].as_bool() == Some(true);
            audit(&gate.dir, &principal, &method, &tool, if failed { "error" } else { "ok" });
            reply(&mut stream, 200, "", &serde_json::to_vec(&value).unwrap());
        }
    }
}

/// Serve the Gate on loopback until the process is stopped.
pub fn serve(estate: &Path, dir: &Path, port: u16) -> Result<Value> {
    mcp::ensure_estate(estate)?;
    ensure_dir(dir)?;
    load(dir)?;
    let listener =
        TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let gate = Arc::new(Gate {
        estate: estate.to_path_buf(),
        dir: dir.to_path_buf(),
    });
    println!(
        "{}",
        json!({"status":"listening","url":format!("http://{addr}/mcp"),
            "version":env!("CARGO_PKG_VERSION")})
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut clients: Vec<thread::JoinHandle<()>> = Vec::new();
    loop {
        clients.retain(|c| !c.is_finished());
        if clients.len() >= MAX_CLIENTS {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() {
                    drop(stream);
                    continue;
                }
                let gate = Arc::clone(&gate);
                clients.push(thread::spawn(move || handle(stream, gate)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

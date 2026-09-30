//! An explicit loopback aperture onto one locked estate. No filesystem routes,
//! outbound requests, implicit HOME, background service installation or AI calls.
use crate::{search, Estate, Event, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const MAX_HEADERS: usize = 8192;
const MAX_BODY: usize = 128 * 1024;
const MAX_CLIENTS: usize = 4;
const PAGE: &str = include_str!("../web/index.html");
const SCRIPT: &str = include_str!("../web/app.js");
const STYLE: &str = include_str!("../web/app.css");

struct State {
    estate: Mutex<Estate>,
    token: String,
    host: String,
    stop: AtomicBool,
}
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Api {
    Status {},
    Query {
        query: String,
    },
    Read {
        id: String,
        expected_source_sha256: Option<String>,
    },
    Put {
        id: String,
        title: String,
        text: String,
        expected_head: String,
    },
    Checkpoint {
        expected_head: String,
    },
    Rollback {
        ring: u32,
        expected_head: String,
    },
    Backup {},
    Stop {},
}

fn parse(stream: &mut TcpStream) -> Result<Request> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if Instant::now() >= deadline {
            return Err("request timed out".into());
        }
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&bytes).map_err(|_| "invalid HTTP request")? {
            httparse::Status::Complete(header_len) => {
                if header_len > MAX_HEADERS || req.version != Some(1) {
                    return Err("unsupported HTTP request".into());
                }
                let method = req.method.ok_or("missing method")?.to_string();
                let path = req.path.ok_or("missing path")?.to_string();
                if !matches!(method.as_str(), "GET" | "POST") || path.len() > 1024 {
                    return Err("unsupported request".into());
                }
                let mut map = BTreeMap::new();
                for h in req.headers.iter() {
                    let key = h.name.to_ascii_lowercase();
                    let value = std::str::from_utf8(h.value)
                        .map_err(|_| "invalid header")?
                        .trim()
                        .to_string();
                    if map.insert(key, value).is_some() {
                        return Err("duplicate header".into());
                    }
                }
                if ["transfer-encoding", "upgrade", "expect"]
                    .iter()
                    .any(|h| map.contains_key(*h))
                {
                    return Err("unsupported request framing".into());
                }
                let length = match map.get("content-length") {
                    Some(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
                        s.parse::<usize>().map_err(|_| "invalid content length")?
                    }
                    Some(_) => return Err("invalid content length".into()),
                    None if method == "GET" => 0,
                    None => return Err("content length required".into()),
                };
                if length > MAX_BODY || (method == "GET" && length != 0) {
                    return Err("body byte limit exceeded".into());
                }
                let total = header_len + length;
                while bytes.len() < total {
                    if Instant::now() >= deadline {
                        return Err("request timed out".into());
                    }
                    let n = stream
                        .read(&mut chunk)
                        .map_err(|_| "incomplete request body")?;
                    if n == 0 {
                        return Err("incomplete request body".into());
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                if bytes.len() != total {
                    return Err("extra request bytes".into());
                }
                return Ok(Request {
                    method,
                    path,
                    headers: map,
                    body: bytes[header_len..].to_vec(),
                });
            }
            httparse::Status::Partial if bytes.len() >= MAX_HEADERS => {
                return Err("header byte limit exceeded".into())
            }
            httparse::Status::Partial => {}
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|_| "incomplete request headers")?;
        if n == 0 {
            return Err("incomplete request headers".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}

fn response(stream: &mut TcpStream, status: u16, mime: &str, bytes: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Service Unavailable",
    };
    let header=format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Resource-Policy: same-origin\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'\r\n\r\n",bytes.len());
    let _ = stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(bytes));
}
fn json_response(stream: &mut TcpStream, status: u16, value: Value) {
    response(
        stream,
        status,
        "application/json; charset=utf-8",
        &serde_json::to_vec(&value).unwrap(),
    );
}
fn snapshot(estate: &Estate) -> Result<Value> {
    let world = estate.world()?;
    let records: Vec<_> = world
        .records()
        .values()
        .map(|r| {
            json!({"id":r.id,"title":r.title,
        "bytes":r.text.len(),"source_sha256":r.source_sha256,"ring":r.ring})
        })
        .collect();
    Ok(
        json!({"status":world.status()?,"records":records,"version":env!("CARGO_PKG_VERSION"),
        "limits":{"documents":crate::MAX_RECORDS,"text_bytes":crate::MAX_TEXT_BYTES,"events":crate::MAX_EVENTS},"egress":"none"}),
    )
}
fn api(state: &State, request: Api) -> std::result::Result<Value, (u16, String)> {
    let mut estate = state
        .estate
        .lock()
        .map_err(|_| (503, "workspace lock unavailable".into()))?;
    if state.stop.load(Ordering::Acquire) {
        return Err((503, "workspace stopped".into()));
    }
    let expected = match &request {
        Api::Put { expected_head, .. }
        | Api::Checkpoint { expected_head }
        | Api::Rollback { expected_head, .. } => Some(expected_head),
        _ => None,
    };
    if let Some(expected) = expected {
        if *expected != estate.world().map_err(|e| (503, e))?.journal_head {
            return Err((409,"Workspace changed in another tab. Refresh before saving; your draft has been kept.".into()));
        }
    }
    let result = match request {
        Api::Status {} => snapshot(&estate),
        Api::Query { query } => search::query(estate.world().map_err(|e| (503, e))?, &query, 20),
        Api::Read {
            id,
            expected_source_sha256,
        } => {
            let records = estate.world().map_err(|e| (503, e))?.records();
            let r = records.get(&id).ok_or((404, "document not found".into()))?;
            if expected_source_sha256.is_some_and(|h| h != r.source_sha256) {
                return Err((
                    409,
                    "This source changed after the search. Search again to open the current text."
                        .into(),
                ));
            }
            Ok(
                json!({"record":r,"journal_head_sha256":estate.world().map_err(|e|(503,e))?.journal_head,
                "world_sha256":estate.world().map_err(|e|(503,e))?.digest().map_err(|e|(503,e))?}),
            )
        }
        Api::Put {
            id, title, text, ..
        } => estate.apply(Event::Put {
            id,
            title,
            text,
            at: None,
        }),
        Api::Checkpoint { .. } => estate.apply(Event::Expand {}),
        Api::Rollback { ring, .. } => estate.apply(Event::Collapse { stable_ring: ring }),
        Api::Backup {} => estate.bundle(),
        Api::Stop {} => {
            state.stop.store(true, Ordering::Release);
            Ok(json!({"status":"stopped"}))
        }
    };
    result.map_err(|e| (400, e))
}

fn handle(mut stream: TcpStream, state: Arc<State>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = match parse(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            json_response(&mut stream, 400, json!({"error":e}));
            return;
        }
    };
    let origin = format!("http://{}", state.host);
    let h = &request.headers;
    if h.get("host") != Some(&state.host)
        || h.get("origin").is_some_and(|s| s != &origin)
        || h.get("sec-fetch-site")
            .is_some_and(|s| !matches!(s.as_str(), "none" | "same-origin"))
    {
        json_response(&mut stream, 403, json!({"error":"local origin required"}));
        return;
    }
    if request.method == "GET" {
        match request.path.as_str() {
            "/" => response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                PAGE.replace("__SESSION_TOKEN__", &state.token).as_bytes(),
            ),
            "/app.js" => response(
                &mut stream,
                200,
                "text/javascript; charset=utf-8",
                SCRIPT.as_bytes(),
            ),
            "/app.css" => response(
                &mut stream,
                200,
                "text/css; charset=utf-8",
                STYLE.as_bytes(),
            ),
            _ => json_response(&mut stream, 404, json!({"error":"not found"})),
        }
        return;
    }
    if request.path != "/api" {
        json_response(&mut stream, 404, json!({"error":"not found"}));
        return;
    }
    if h.get("origin") != Some(&origin) || h.get("x-workspace-token") != Some(&state.token) {
        json_response(
            &mut stream,
            403,
            json!({"error":"Reload this local workspace to reconnect."}),
        );
        return;
    }
    if h.get("content-type")
        .map(|s| s.split(';').next().unwrap().trim())
        != Some("application/json")
    {
        json_response(&mut stream, 400, json!({"error":"JSON required"}));
        return;
    }
    let request = match serde_json::from_slice::<Api>(&request.body) {
        Ok(r) => r,
        Err(_) => {
            json_response(
                &mut stream,
                400,
                json!({"error":"invalid action or fields"}),
            );
            return;
        }
    };
    match api(&state, request) {
        Ok(value) => json_response(&mut stream, 200, value),
        Err((status, error)) => json_response(&mut stream, status, json!({"error":error})),
    }
}

pub fn serve(path: &Path, port: u16) -> Result<Value> {
    let estate = Estate::open(path)?;
    let listener =
        TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let host = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let token = random.iter().map(|b| format!("{b:02x}")).collect();
    let state = Arc::new(State {
        estate: Mutex::new(estate),
        token,
        host: host.clone(),
        stop: AtomicBool::new(false),
    });
    println!(
        "{}",
        json!({"status":"listening","url":format!("http://{host}"),"version":env!("CARGO_PKG_VERSION")})
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut clients: Vec<thread::JoinHandle<()>> = Vec::new();
    while !state.stop.load(Ordering::Acquire) {
        let mut i = 0;
        while i < clients.len() {
            if clients[i].is_finished() {
                let _ = clients.swap_remove(i).join();
            } else {
                i += 1;
            }
        }
        // Keep excess connections in the OS listener backlog until a worker is
        // free. Dropping accepted sockets here breaks ordinary multi-tab loads.
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
                let state = Arc::clone(&state);
                clients.push(thread::spawn(move || handle(stream, state)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    for client in clients {
        let _ = client.join();
    }
    Ok(json!({"status":"stopped"}))
}

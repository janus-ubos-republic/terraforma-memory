//! Model Context Protocol adapter (stdio, JSON-RPC 2.0, one message per line).
//! It gives AI coding assistants a local, versioned, tamper-evident memory.
//! The estate is opened per tool call, so several assistants (for example
//! Claude Code and Codex) can share one memory: the lock is held only while a
//! single call runs, and a busy estate is retried briefly instead of failing.
use crate::{init, now_utc, search, Estate, Event, Result, MAX_RECORDS, MAX_TEXT_BYTES};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const SERVER_NAME: &str = "terraforma-memory";
// Newest first; the client's requested version is echoed when we support it.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const LOCK_WAIT: Duration = Duration::from_secs(5);
const MAX_LINE_BYTES: usize = 1024 * 1024;
const INSTRUCTIONS: &str = "Local project memory. Call `recall` with specific words \
before non-trivial work to find earlier decisions, conventions and known pitfalls. \
Call `remember` when a decision is made, a convention is agreed, a mistake is \
understood or a hard-won fact is found; reuse the same title to update a memory. \
Everything stays on this machine in a tamper-evident journal.";

/// Create the estate on first use so a fresh install works without a separate
/// init step. Missing parent directories are created private to the user, so
/// the documented `~/.terraforma/memory/<project>` works on a clean machine.
pub fn ensure_estate(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(slug)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "memory".into());
    init(path, name)
}

fn open(path: &Path) -> Result<Estate> {
    let started = Instant::now();
    loop {
        match Estate::open(path) {
            Err(e) if e.starts_with("estate busy") && started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(25))
            }
            other => return other,
        }
    }
}

/// Lowercase ASCII identifier: letters and digits kept, everything else a dash.
fn slug(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 72 {
            break;
        }
    }
    out.trim_end_matches('-').to_string()
}
fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut out: String = line.trim().chars().take(max).collect();
    if line.trim().chars().count() > max {
        out.push('…');
    }
    out
}

pub fn tools() -> Value {
    json!([
      {"name":"remember",
       "description":"Save or update a durable memory for this project: a decision and its reason, a convention, a mistake to avoid, a user preference, an open task, or a fact that took effort to discover. Saving again with the same title (or id) updates that memory; every earlier version stays in the tamper-evident journal. Keep one idea per memory, written so a future session understands it without this conversation. Text up to 16 KB.",
       "inputSchema":{"type":"object","additionalProperties":false,"required":["title","text"],
         "properties":{
           "title":{"type":"string","description":"Short, specific title, e.g. 'Decision: use SQLite for the job queue'. Reusing a title updates that memory."},
           "text":{"type":"string","description":"The memory itself, including why it matters."},
           "id":{"type":"string","description":"Optional explicit id (letters, digits, - _ .). Normally derived from the title."}}}},
      {"name":"recall",
       "description":"Search saved memories by exact words (case-insensitive, not semantic). Use specific terms: names, file paths, error messages, identifiers. Check before non-trivial work for earlier decisions and known pitfalls. Full matches rank first.",
       "inputSchema":{"type":"object","additionalProperties":false,"required":["query"],
         "properties":{
           "query":{"type":"string","description":"1 to 32 words."},
           "limit":{"type":"integer","minimum":1,"maximum":20,"description":"Maximum results (default 5)."}}}},
      {"name":"read_memory",
       "description":"Read the full text of one memory by id. Ids come from `recall` or `recent`.",
       "inputSchema":{"type":"object","additionalProperties":false,"required":["id"],
         "properties":{"id":{"type":"string"}}}},
      {"name":"recent",
       "description":"List the most recently saved or updated memories, newest first. Useful at the start of a session to see what was worked on.",
       "inputSchema":{"type":"object","additionalProperties":false,
         "properties":{"limit":{"type":"integer","minimum":1,"maximum":50,"description":"Maximum results (default 10)."}}}},
      {"name":"memory_status",
       "description":"Show how many memories exist, the limits, and the integrity digest of the journal.",
       "inputSchema":{"type":"object","additionalProperties":false,"properties":{}}}
    ])
}

fn args_of<'a>(args: &'a Map<String, Value>, allowed: &[&str]) -> Result<&'a Map<String, Value>> {
    if let Some(k) = args.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unknown argument: {k}"));
    }
    Ok(args)
}
fn text_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` must be a string"))
}
fn limit_arg(args: &Map<String, Value>, default: usize, max: usize) -> Result<usize> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|n| n as usize)
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| format!("`limit` must be an integer from 1 to {max}")),
    }
}

/// Run one tool. The returned text is what the assistant reads.
pub fn call(path: &Path, name: &str, args: &Map<String, Value>) -> Result<String> {
    match name {
        "remember" => {
            let args = args_of(args, &["title", "text", "id"])?;
            let title = text_arg(args, "title")?.trim().to_string();
            let text = text_arg(args, "text")?.to_string();
            if text.len() > MAX_TEXT_BYTES {
                return Err(format!(
                    "text is {} bytes; the limit is {MAX_TEXT_BYTES}. Split it into several memories.",
                    text.len()
                ));
            }
            let mut estate = open(path)?;
            let id = match args.get("id") {
                Some(v) => v.as_str().ok_or("`id` must be a string")?.to_string(),
                None => {
                    let base = slug(&title);
                    let base = if base.is_empty() {
                        "memory".into()
                    } else {
                        base
                    };
                    // A different title that folds to the same slug gets its own id
                    // instead of silently overwriting the other memory.
                    match estate.world()?.get(&base) {
                        Some(r) if r.title != title => {
                            format!("{base}-{}", &crate::sha256(title.as_bytes())[..8])
                        }
                        _ => base,
                    }
                }
            };
            let at = now_utc();
            let receipt = estate.apply(Event::Put {
                id: id.clone(),
                title: title.clone(),
                text,
                at: Some(at.clone()),
            })?;
            if receipt["status"] == "unchanged" {
                return Ok(format!(
                    "Unchanged: memory `{id}` already has exactly this title and text."
                ));
            }
            let world = estate.world()?;
            let writes = world.meta().get(&id).map_or(1, |m| m.writes);
            Ok(format!(
                "Saved memory `{id}` — \"{title}\" (version {writes}, {at}).\n{} of {MAX_RECORDS} memories used. Journal head {}.",
                world.record_count(),
                short(receipt["journal_head_sha256"].as_str().unwrap_or(""))
            ))
        }
        "recall" => {
            let args = args_of(args, &["query", "limit"])?;
            let query = text_arg(args, "query")?;
            let limit = limit_arg(args, 5, 20)?;
            let estate = open(path)?;
            let found = search::query(estate.world()?, query, limit)?;
            let result = &found["result"];
            let hits = result["hits"].as_array().cloned().unwrap_or_default();
            if hits.is_empty() {
                return Ok(format!(
                    "No memories contain any of these words ({} memories searched). Try other specific words, or `recent`.",
                    result["record_count"]
                ));
            }
            let mut out = format!(
                "{} of {} matching memories for \"{}\":\n",
                hits.len(),
                result["unique_match_count"],
                result["query"].as_str().unwrap_or(query)
            );
            let world = estate.world()?;
            for (i, h) in hits.iter().enumerate() {
                let id = h["id"].as_str().unwrap_or("");
                let when = world
                    .meta()
                    .get(id)
                    .and_then(|m| m.last_at.clone())
                    .unwrap_or_else(|| "undated".into());
                let coverage = if h["status"] == "full_match" {
                    "all words".to_string()
                } else {
                    let missing: Vec<&str> = h["missing_terms"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    format!("missing: {}", missing.join(", "))
                };
                out.push_str(&format!(
                    "\n{}. `{id}` — {} ({when}; {coverage})\n   line {}: {}\n",
                    i + 1,
                    h["title"].as_str().unwrap_or(""),
                    h["line"],
                    h["excerpt"].as_str().unwrap_or("")
                ));
            }
            if result["truncated"] == true {
                out.push_str("\nMore matches exist; raise `limit` or use more specific words.\n");
            }
            out.push_str("\nUse `read_memory` with an id for the full text.");
            Ok(out)
        }
        "read_memory" => {
            let args = args_of(args, &["id"])?;
            let id = text_arg(args, "id")?;
            let estate = open(path)?;
            let world = estate.world()?;
            let record = world.get(id).ok_or_else(|| {
                format!("No memory with id `{id}`. Use `recall` or `recent` to find ids.")
            })?;
            let meta = world.meta().get(id);
            Ok(format!(
                "# {}\nid: `{id}` · updated {} · version {} · sha256 {}\n\n{}",
                record.title,
                meta.and_then(|m| m.last_at.clone())
                    .unwrap_or_else(|| "undated".into()),
                meta.map_or(1, |m| m.writes),
                short(&record.source_sha256),
                record.text
            ))
        }
        "recent" => {
            let args = args_of(args, &["limit"])?;
            let limit = limit_arg(args, 10, 50)?;
            let estate = open(path)?;
            Ok(recent_text(estate.world()?, limit))
        }
        "memory_status" => {
            args_of(args, &[])?;
            let estate = open(path)?;
            let world = estate.world()?;
            Ok(format!(
                "Memory `{}`: {} of {MAX_RECORDS} memories, {} journal events.\nWorld sha256: {}\nJournal head: {}\nStored locally at {}; nothing is sent over the network.",
                world.estate_id,
                world.record_count(),
                world.sequence,
                world.digest()?,
                world.journal_head,
                path.display()
            ))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn recent_text(world: &crate::World, limit: usize) -> String {
    let mut items: Vec<_> = world.meta().iter().collect();
    items.sort_by(|a, b| b.1.last_sequence.cmp(&a.1.last_sequence));
    if items.is_empty() {
        return "No memories saved yet. Use `remember` to save the first one.".into();
    }
    let mut out = format!(
        "{} most recent of {} memories (newest first):\n",
        items.len().min(limit),
        items.len()
    );
    for (id, meta) in items.into_iter().take(limit) {
        if let Some(r) = world.get(id) {
            out.push_str(&format!(
                "- `{id}` — {} ({}, v{})\n  {}\n",
                r.title,
                meta.last_at.as_deref().unwrap_or("undated"),
                meta.writes,
                first_line(&r.text, 140)
            ));
        }
    }
    out
}

/// Markdown for an assistant's session-start hook. A missing estate prints
/// nothing so a hook never blocks a session.
pub fn briefing(path: &Path, limit: usize) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let estate = open(path)?;
    let world = estate.world()?;
    Ok(format!(
        "## Project memory\n\n{}\nTools: `recall` (search before non-trivial work), `read_memory`, `remember` (save decisions, conventions, pitfalls).\n",
        recent_text(world, limit)
    ))
}

fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

/// Handle one JSON-RPC message. Notifications return `None`.
pub fn handle(path: &Path, line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error(&Value::Null, -32700, "parse error")),
    };
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = id else {
        return None; // notifications, including notifications/initialized
    };
    let params = message.get("params").cloned().unwrap_or(json!({}));
    Some(match method {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or("");
            let version = PROTOCOLS
                .iter()
                .find(|v| **v == requested)
                .unwrap_or(&PROTOCOLS[0]);
            response(
                &id,
                json!({"protocolVersion":version,
                "capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":SERVER_NAME,"version":env!("CARGO_PKG_VERSION")},
                "instructions":INSTRUCTIONS}),
            )
        }
        "ping" => response(&id, json!({})),
        "tools/list" => response(&id, json!({"tools":tools()})),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let empty = Map::new();
            let args = match params.get("arguments") {
                None | Some(Value::Null) => &empty,
                Some(Value::Object(m)) => m,
                Some(_) => return Some(error(&id, -32602, "arguments must be an object")),
            };
            // Tool failures are results the assistant can read and correct.
            let (text, is_error) = match call(path, name, args) {
                Ok(text) => (text, false),
                Err(e) => (e, true),
            };
            response(
                &id,
                json!({"content":[{"type":"text","text":text}],"isError":is_error}),
            )
        }
        _ => error(&id, -32601, "method not found"),
    })
}

/// Serve MCP over stdin/stdout until stdin closes.
pub fn serve(path: &Path) -> Result<()> {
    ensure_estate(path)?;
    // Refuse to start on a damaged estate rather than failing every call later.
    open(path)?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = (&mut input)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut buf)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        let reply = if buf.len() > MAX_LINE_BYTES && !buf.ends_with(b"\n") {
            // Discard the rest of an oversized line before answering.
            let mut rest = Vec::new();
            input
                .read_until(b'\n', &mut rest)
                .map_err(|e| e.to_string())?;
            Some(error(&Value::Null, -32600, "message too large"))
        } else {
            match std::str::from_utf8(&buf) {
                Ok(line) if line.trim().is_empty() => None,
                Ok(line) => handle(path, line.trim()),
                Err(_) => Some(error(&Value::Null, -32700, "message is not UTF-8")),
            }
        };
        if let Some(reply) = reply {
            let mut out = stdout.lock();
            serde_json::to_writer(&mut out, &reply).map_err(|e| e.to_string())?;
            out.write_all(b"\n")
                .and_then(|_| out.flush())
                .map_err(|e| e.to_string())?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tf-mcp-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn tool(path: &Path, name: &str, args: Value) -> (String, bool) {
        let line = json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":name,"arguments":args}})
        .to_string();
        let reply = handle(path, &line).unwrap();
        (
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
            reply["result"]["isError"].as_bool().unwrap(),
        )
    }

    #[test]
    fn protocol_handshake_and_notifications() {
        let root = temp("handshake");
        ensure_estate(&root).unwrap();
        let init = handle(
            &root,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        let unknown = handle(
            &root,
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        )
        .unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOLS[0]);
        assert!(handle(
            &root,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
        let list = handle(&root, r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);
        let missing = handle(&root, r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#).unwrap();
        assert_eq!(missing["error"]["code"], -32601);
        assert_eq!(handle(&root, "{").unwrap()["error"]["code"], -32700);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_use_creates_missing_parent_directories() {
        let root = temp("fresh-home");
        let estate = root.join(".terraforma").join("memory").join("my-project");
        ensure_estate(&estate).unwrap();
        assert!(estate.join("journal.jsonl").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join(".terraforma"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let (_, err) = tool(&estate, "remember", json!({"title":"t","text":"first use"}));
        assert!(!err);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remember_recall_update_and_recent() {
        let root = temp("memory");
        ensure_estate(&root).unwrap();
        let (saved, err) = tool(
            &root,
            "remember",
            json!({"title":"Decision: SQLite job queue",
            "text":"We use SQLite for the job queue because Redis is unavailable on client machines."}),
        );
        assert!(!err, "{saved}");
        assert!(saved.contains("`decision-sqlite-job-queue`") && saved.contains("version 1"));
        let (again, _) = tool(
            &root,
            "remember",
            json!({"title":"Decision: SQLite job queue",
            "text":"We use SQLite for the job queue because Redis is unavailable on client machines."}),
        );
        assert!(again.starts_with("Unchanged"));
        let (updated, _) = tool(
            &root,
            "remember",
            json!({"title":"Decision: SQLite job queue",
            "text":"SQLite job queue, WAL mode on. Redis is unavailable on client machines."}),
        );
        assert!(updated.contains("version 2"), "{updated}");
        // A title folding to the same slug must not overwrite the first memory.
        let (other, _) = tool(
            &root,
            "remember",
            json!({"title":"Decision — SQLite job queue!",
            "text":"A different memory."}),
        );
        assert!(other.contains("`decision-sqlite-job-queue-"), "{other}");
        let (found, _) = tool(&root, "recall", json!({"query":"redis queue"}));
        assert!(
            found.contains("`decision-sqlite-job-queue`") && found.contains("all words"),
            "{found}"
        );
        let (none, err) = tool(&root, "recall", json!({"query":"kubernetes"}));
        assert!(!err && none.starts_with("No memories"));
        let (full, _) = tool(
            &root,
            "read_memory",
            json!({"id":"decision-sqlite-job-queue"}),
        );
        assert!(full.contains("WAL mode on") && full.contains("version 2"));
        let (recent, _) = tool(&root, "recent", json!({"limit":1}));
        assert!(recent.contains("Decision — SQLite job queue!"), "{recent}");
        let brief = briefing(&root, 5).unwrap();
        assert!(brief.starts_with("## Project memory") && brief.contains("2 most recent of 2"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tool_errors_are_readable_results() {
        let root = temp("errors");
        ensure_estate(&root).unwrap();
        let (msg, err) = tool(&root, "remember", json!({"title":"x"}));
        assert!(err && msg.contains("`text`"));
        let (msg, err) = tool(&root, "remember", json!({"title":"x","text":"y","extra":1}));
        assert!(err && msg.contains("unknown argument"));
        let (msg, err) = tool(&root, "read_memory", json!({"id":"missing"}));
        assert!(err && msg.contains("No memory"));
        let (msg, err) = tool(&root, "recall", json!({"query":"a","limit":99}));
        assert!(err && msg.contains("limit"));
        let big = "x".repeat(MAX_TEXT_BYTES + 1);
        let (msg, err) = tool(&root, "remember", json!({"title":"big","text":big}));
        assert!(err && msg.contains("Split"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn busy_estate_waits_for_the_other_writer() {
        let root = temp("shared");
        ensure_estate(&root).unwrap();
        let holder = Estate::open(&root).unwrap();
        let path = root.clone();
        let writer = std::thread::spawn(move || {
            tool(
                &path,
                "remember",
                json!({"title":"shared","text":"written after the lock"}),
            )
        });
        std::thread::sleep(Duration::from_millis(200));
        drop(holder);
        let (saved, err) = writer.join().unwrap();
        assert!(!err, "{saved}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timestamps_do_not_change_old_journals() {
        assert_eq!(crate::format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(crate::format_utc(1_758_794_400), "2025-09-25T10:00:00Z");
        assert_eq!(crate::format_utc(951_782_400), "2000-02-29T00:00:00Z");
        let old = r#"{"type":"Put","id":"a","title":"t","text":"x"}"#;
        let event: Event = serde_json::from_str(old).unwrap();
        assert_eq!(serde_json::to_string(&event).unwrap(), old);
    }
}

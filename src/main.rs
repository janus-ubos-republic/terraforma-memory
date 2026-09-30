use std::path::Path;
use terraforma_local_core::{read_bounded, Estate, Event, Result, MAX_TEXT_BYTES};

fn run() -> Result<serde_json::Value> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && args[0] == "version" {
        return Ok(serde_json::json!({"version":env!("CARGO_PKG_VERSION"),
            "source_lineage":serde_json::from_str::<serde_json::Value>(include_str!("../lineage.json")).unwrap()}));
    }
    if args.len() < 2 {
        return Err("usage: terraforma-local-core <init|status|put|get|query|serve|mcp|briefing|expand|collapse|export|restore|backup|restore-backup|verify-desk-replay> <estate> [arguments]; version".into());
    }
    let path = Path::new(&args[1]);
    match (args[0].as_str(), args.len()) {
        ("init", 3) => {
            terraforma_local_core::init(path, args[2].clone())?;
            Estate::open(path)?.world()?.status()
        }
        ("verify-desk-replay", 2) => terraforma_local_core::replay::verify(path),
        ("restore", 3) => {
            let dest = Path::new(&args[2]);
            terraforma_local_core::restore(path, dest)?;
            Estate::open(dest)?.world()?.status()
        }
        ("status", 2) => Estate::open(path)?.world()?.status(),
        ("serve", 2) => terraforma_local_core::server::serve(path, 0),
        ("serve", 3) => {
            terraforma_local_core::server::serve(path, args[2].parse().map_err(|_| "invalid port")?)
        }
        ("query", 3) => {
            terraforma_local_core::search::query(Estate::open(path)?.world()?, &args[2], 20)
        }
        ("backup", 3) => Estate::open(path)?.backup(Path::new(&args[2])),
        ("restore-backup", 3) => {
            let bytes = read_bounded(path, terraforma_local_core::MAX_BUNDLE_BYTES)?;
            let dest = Path::new(&args[2]);
            terraforma_local_core::restore_bundle(&bytes, dest)?;
            Estate::open(dest)?.world()?.status()
        }
        ("expand", 2) => Estate::open(path)?.apply(Event::Expand {}),
        ("collapse", 3) => Estate::open(path)?.apply(Event::Collapse {
            stable_ring: args[2]
                .parse()
                .map_err(|_| "ring must be an unsigned integer")?,
        }),
        ("put", 5) => {
            let text = String::from_utf8(read_bounded(Path::new(&args[4]), MAX_TEXT_BYTES as u64)?)
                .map_err(|_| "source must be UTF-8")?;
            Estate::open(path)?.apply(Event::Put {
                id: args[2].clone(),
                title: args[3].clone(),
                text,
                at: None,
            })
        }
        ("get", 3) => {
            let estate = Estate::open(path)?;
            let records = estate.world()?.records();
            Ok(
                serde_json::json!({"schema":"ubos.terraforma-local-query.v1",
                "world_sha256":estate.world()?.digest()?,"id":args[2],
                "status":if records.contains_key(&args[2]) {"found"} else {"not_found"},"record":records.get(&args[2])}),
            )
        }
        ("export", 3) => Estate::open(path)?.export(Path::new(&args[2])),
        _ => Err("unknown command or wrong argument count".into()),
    }
}
/// Commands whose stdout is not a single JSON document: `mcp` speaks JSON-RPC
/// on stdout and `briefing` prints Markdown for a session-start hook.
fn run_stream(args: &[String]) -> Option<Result<()>> {
    match (args.first().map(String::as_str), args.len()) {
        (Some("mcp"), 2) => Some(terraforma_local_core::mcp::serve(Path::new(&args[1]))),
        (Some("briefing"), 2 | 3) => Some((|| {
            let limit = match args.get(2) {
                Some(n) => n
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=50).contains(n))
                    .ok_or("briefing limit must be 1..50")?,
                None => 10,
            };
            print!(
                "{}",
                terraforma_local_core::mcp::briefing(Path::new(&args[1]), limit)?
            );
            Ok(())
        })()),
        _ => None,
    }
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(outcome) = run_stream(&args) {
        if let Err(error) = outcome {
            eprintln!("{}", serde_json::json!({"status":"error","error":error}));
            std::process::exit(1)
        }
        return;
    }
    match run() {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{}", serde_json::json!({"status":"error","error":error}));
            std::process::exit(1)
        }
    }
}

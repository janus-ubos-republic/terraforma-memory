//! Verification for sealed, display-only M4 Desk replay packages.
use crate::{read_bounded, sha256, Result, MAX_BUNDLE_BYTES};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const NAMES: [&str; 5] = [
    "desk-product-export.json",
    "proposal.json",
    "manifest.json",
    "presentation.json",
    "workspace-backup.json",
];
const STOPS: [&str; 4] = [
    "no_m4_owner_access",
    "no_m4_state_write",
    "no_local_core_admission",
    "no_source_authority_change",
];
const MAX_METADATA_BYTES: u64 = 128 * 1024;

fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn object<'a>(value: &'a Value, field: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{field} object required"))
}
fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} string required"))
}
fn regular(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| format!("package file missing: {}", path.display()))?;
    require(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "regular package file required",
    )
}
fn stops(value: &Value) -> Result<()> {
    let actual = value
        .get("required_stops")
        .and_then(Value::as_array)
        .ok_or("required stops required")?;
    require(
        actual.len() == STOPS.len()
            && actual.iter().map(Value::as_str).collect::<Option<Vec<_>>>() == Some(STOPS.to_vec()),
        "replay package stops differ",
    )
}

pub fn verify(root: &Path) -> Result<Value> {
    let metadata =
        fs::symlink_metadata(root).map_err(|_| "replay package directory missing".to_string())?;
    require(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "regular replay package directory required",
    )?;
    let names = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            entry
                .map(|e| e.file_name().to_string_lossy().to_string())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let expected = NAMES
        .iter()
        .chain(["package.json"].iter())
        .map(|name| name.to_string())
        .collect::<BTreeSet<_>>();
    require(names == expected, "replay package file set differs")?;
    let package_path = root.join("package.json");
    regular(&package_path)?;
    let package_bytes = read_bounded(&package_path, MAX_METADATA_BYTES)?;
    let package: Value = serde_json::from_slice(&package_bytes).map_err(|e| e.to_string())?;
    require(
        string(&package, "schema")? == "ubos.m4-desk-product-replay-package.v1",
        "unknown replay package",
    )?;
    require(
        string(&package, "state")? == "display_only_replay_package",
        "non-display replay package refused",
    )?;
    stops(&package)?;
    let files = object(&package, "files")?;
    require(
        files.len() == NAMES.len() && NAMES.iter().all(|name| files.contains_key(*name)),
        "exact replay file bindings required",
    )?;
    for name in NAMES {
        let path = root.join(name);
        regular(&path)?;
        let limit = if name == "workspace-backup.json" {
            MAX_BUNDLE_BYTES
        } else {
            MAX_METADATA_BYTES
        };
        let bytes = read_bounded(&path, limit)?;
        let binding = files
            .get(name)
            .and_then(Value::as_object)
            .ok_or("file binding required")?;
        require(
            binding.get("sha256").and_then(Value::as_str) == Some(sha256(&bytes).as_str()),
            "replay file hash differs",
        )?;
        require(
            binding.get("bytes").and_then(Value::as_u64) == Some(bytes.len() as u64),
            "replay file byte size differs",
        )?;
    }
    let presentation_bytes = read_bounded(&root.join("presentation.json"), MAX_METADATA_BYTES)?;
    let presentation: Value =
        serde_json::from_slice(&presentation_bytes).map_err(|e| e.to_string())?;
    require(
        string(&presentation, "schema")? == "ubos.m4-local-core-product-presentation.v1",
        "unknown replay presentation",
    )?;
    require(
        string(&presentation, "state")? == "display_only",
        "non-display presentation refused",
    )?;
    stops(&presentation)?;
    for field in [
        "presentation_id",
        "proposal_sha256",
        "manifest_sha256",
        "source_binding",
        "status_record",
    ] {
        require(
            package.get(field) == presentation.get(field),
            "package presentation binding differs",
        )?;
    }
    let desk = read_bounded(&root.join("desk-product-export.json"), MAX_METADATA_BYTES)?;
    require(
        package
            .get("source_binding")
            .and_then(|value| value.get("sha256"))
            .and_then(Value::as_str)
            == Some(sha256(&desk).as_str()),
        "Desk export binding differs",
    )?;
    let backup = read_bounded(&root.join("workspace-backup.json"), MAX_BUNDLE_BYTES)?;
    let world = crate::inspect_backup(&backup)?;
    let records = world.records();
    let status = object(&package, "status_record")?;
    let boundary = object(&package, "boundary_record")?;
    let status_id = status
        .get("id")
        .and_then(Value::as_str)
        .ok_or("status record id required")?;
    let boundary_id = boundary
        .get("id")
        .and_then(Value::as_str)
        .ok_or("boundary record id required")?;
    require(
        records
            .get(status_id)
            .map(|record| record.source_sha256.as_str())
            == status.get("source_sha256").and_then(Value::as_str),
        "backup status record differs",
    )?;
    require(
        records.contains_key(boundary_id),
        "backup boundary record missing",
    )?;
    Ok(
        serde_json::json!({"schema":"ubos.terraforma-local-desk-replay-verification.v1","status":"verified_display_only_replay_package","package_sha256":sha256(&package_bytes),"presentation_id":package["presentation_id"],"source_export_sha256":sha256(&desk),"status_record":package["status_record"],"boundary_record":package["boundary_record"],"records":records.len(),"world_sha256":world.digest()?,"required_stops":STOPS,"boundary":"Verification reads one sealed display-only package only. It does not open an M4 owner, write M4 state, admit a source or local record, change authority, or perform an external action."}),
    )
}

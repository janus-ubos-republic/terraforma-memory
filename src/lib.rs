//! A bounded journal adapter around exact inherited Terraforma modules.
//! This v1 journal deliberately does not accept historical daemon WALs.
pub type Coordinate = u64;
#[path = "inherited/cell.rs"]
pub mod cell;
#[path = "inherited/governor.rs"]
pub mod governor;
pub mod mcp;
#[path = "inherited/monolith.rs"]
pub mod monolith;
pub mod replay;
pub mod search;
pub mod server;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_BUNDLE_BYTES: u64 = MAX_LOG_BYTES * 2 + 4096;
pub const MAX_EVENTS: usize = 50_000;
pub const MAX_RECORDS: usize = 5_000;
pub const MAX_RINGS: u32 = 64;
const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
/// RFC 3339 UTC second-precision time without a date dependency.
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}
pub fn format_utc(secs: i64) -> String {
    // Howard Hinnant's days-to-civil conversion.
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}
fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| e.to_string())
}
fn id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Event {
    Genesis {
        estate_id: String,
    },
    Expand {},
    Put {
        id: String,
        title: String,
        text: String,
        // Optional UTC write time. Absent in v0.2 journals, so their bytes and
        // hash chain replay unchanged; it never enters the world digest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    Collapse {
        stable_ring: u32,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryBody {
    schema: String,
    sequence: usize,
    previous_sha256: String,
    event: Event,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    body: EntryBody,
    sha256: String,
}
#[derive(Clone, Serialize)]
pub struct Record {
    pub id: String,
    pub title: String,
    pub text: String,
    pub source_sha256: String,
    pub ring: u32,
    pub cell: cell::DataCell,
}
/// Derived per-record write history. Rebuilt by replay; never part of the digest.
#[derive(Clone, Debug, Serialize)]
pub struct Meta {
    pub last_sequence: usize,
    pub last_at: Option<String>,
    pub writes: usize,
}
pub struct World {
    pub estate_id: String,
    pub monolith: monolith::Monolith,
    versions: BTreeMap<u32, BTreeMap<String, Record>>,
    meta: BTreeMap<String, Meta>,
    pub sequence: usize,
    pub journal_head: String,
}
impl World {
    fn empty() -> Self {
        Self {
            estate_id: String::new(),
            monolith: monolith::Monolith::genesis(),
            versions: BTreeMap::from([(0, BTreeMap::new())]),
            meta: BTreeMap::new(),
            sequence: 0,
            journal_head: ZERO_HASH.to_string(),
        }
    }
    pub fn records(&self) -> BTreeMap<String, Record> {
        let mut out = BTreeMap::new();
        for records in self.versions.values() {
            out.extend(records.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        out
    }
    /// The visible version of one record: the highest ring holding it wins,
    /// matching `records()` without cloning the whole world.
    pub fn get(&self, id: &str) -> Option<&Record> {
        self.versions.values().rev().find_map(|v| v.get(id))
    }
    pub fn meta(&self) -> &BTreeMap<String, Meta> {
        &self.meta
    }
    pub fn record_count(&self) -> usize {
        self.meta.len()
    }
    pub fn digest(&self) -> Result<String> {
        // BTreeMaps, fixed field order and UTF-8 JSON are the v1 serialization.
        // The legacy 64-bit root is diagnostic only; retained text is bound here.
        Ok(sha256(&json_bytes(&serde_json::json!({
            "schema": "ubos.terraforma-local-world.v1", "estate_id": self.estate_id,
            "current_ring": self.monolith.current_ring,
            "coordinates": self.monolith.expansion_ledger, "versions": self.versions
        }))?))
    }
    pub fn status(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "schema": "ubos.terraforma-local-status.v1", "estate_id": self.estate_id,
            "ring": self.monolith.current_ring, "ring_count": self.versions.len(),
            "record_count": self.record_count(), "world_sha256": self.digest()?,
            "journal_head_sha256": self.journal_head, "events": self.sequence,
            "legacy_coordinate_root_u64": self.monolith.merkle_root(),
            "qualification": "bounded local core; no federation or legacy WAL migration"
        }))
    }
    /// Validate an event against the current world without mutating it. Every
    /// refusal happens here, so `commit` after a successful check cannot fail.
    fn check(&self, event: &Event) -> Result<Option<cell::DataCell>> {
        if self.sequence >= MAX_EVENTS {
            return Err("event limit reached".into());
        }
        match event {
            Event::Genesis { estate_id } => {
                if self.sequence != 0 || !id_valid(estate_id) {
                    return Err("invalid genesis".into());
                }
                Ok(None)
            }
            _ if self.sequence == 0 => Err("first event must be genesis".into()),
            Event::Expand {} => {
                if self.monolith.current_ring >= MAX_RINGS {
                    return Err("ring limit reached".into());
                }
                Ok(None)
            }
            Event::Collapse { stable_ring } => {
                if *stable_ring >= self.monolith.current_ring {
                    return Err("collapse requires an earlier existing ring".into());
                }
                Ok(None)
            }
            Event::Put {
                id,
                title,
                text,
                at,
            } => {
                if !id_valid(id)
                    || title.trim().is_empty()
                    || title.len() > 256
                    || text.is_empty()
                    || text.len() > MAX_TEXT_BYTES
                    || at.as_ref().is_some_and(|t| t.is_empty() || t.len() > 40)
                {
                    return Err(
                        "record requires a safe id, 1..256-byte title and 1..16384-byte UTF-8 text"
                            .into(),
                    );
                }
                if !self.meta.contains_key(id) && self.meta.len() >= MAX_RECORDS {
                    return Err("record limit reached".into());
                }
                let id_hash = Sha256::digest(id.as_bytes());
                let coordinate = u64::from_be_bytes(id_hash[..8].try_into().unwrap());
                if coordinate == 0
                    || self
                        .versions
                        .values()
                        .flat_map(|v| v.values())
                        .any(|r| r.cell.coordinate == coordinate && r.id != *id)
                {
                    return Err("coordinate collision".into());
                }
                let mut cell = cell::DataCell::new(coordinate, cell::CellKind::Document);
                cell.teeth[0].copy_from_slice(&Sha256::digest(text.as_bytes()));
                cell.payload_length = text.len() as u32;
                // The inherited Governor checks a nonempty tooth. Explicit limits above
                // are the adapter's policy; its historical throttle is not a clock.
                governor::Governor::new(1)
                    .inspect_and_admit(&cell)
                    .map_err(|e| format!("admission: {e:?}"))?;
                Ok(Some(cell))
            }
        }
    }
    fn apply(&mut self, event: &Event) -> Result<()> {
        let cell = self.check(event)?;
        match event {
            Event::Genesis { estate_id } => self.estate_id = estate_id.clone(),
            Event::Expand {} => {
                self.versions
                    .insert(self.monolith.expand(), BTreeMap::new());
            }
            Event::Collapse { stable_ring } => {
                self.monolith.collapse_to_ring(*stable_ring);
                self.versions.retain(|ring, _| ring <= stable_ring);
                let versions = &self.versions;
                self.meta
                    .retain(|id, _| versions.values().any(|v| v.contains_key(id)));
            }
            Event::Put {
                id,
                title,
                text,
                at,
            } => {
                let cell = cell.ok_or("missing admitted cell")?;
                let coordinate = cell.coordinate;
                let ring = self.monolith.current_ring;
                let records = self.versions.get_mut(&ring).ok_or("missing ring")?;
                if !records.contains_key(id) {
                    self.monolith.add_coordinate(coordinate);
                }
                records.insert(
                    id.clone(),
                    Record {
                        id: id.clone(),
                        title: title.clone(),
                        text: text.clone(),
                        source_sha256: sha256(text.as_bytes()),
                        ring,
                        cell,
                    },
                );
                let meta = self.meta.entry(id.clone()).or_insert(Meta {
                    last_sequence: 0,
                    last_at: None,
                    writes: 0,
                });
                meta.last_sequence = self.sequence + 1;
                meta.last_at = at.clone();
                meta.writes += 1;
            }
        }
        Ok(())
    }
}

pub fn replay(bytes: &[u8]) -> Result<World> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_LOG_BYTES || !bytes.ends_with(b"\n") {
        return Err("journal is empty, too large, or has an incomplete final entry".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut world = World::empty();
    for (i, line) in text.strip_suffix('\n').unwrap().split('\n').enumerate() {
        let entry: Entry =
            serde_json::from_str(line).map_err(|e| format!("journal line {}: {e}", i + 1))?;
        if entry.body.schema != "ubos.terraforma-local-event.v1"
            || entry.body.sequence != world.sequence + 1
            || entry.body.previous_sha256 != world.journal_head
            || sha256(&json_bytes(&entry.body)?) != entry.sha256
        {
            return Err(format!(
                "journal line {}: schema, sequence or hash-chain mismatch",
                i + 1
            ));
        }
        world
            .apply(&entry.body.event)
            .map_err(|e| format!("journal line {}: {e}", i + 1))?;
        world.sequence += 1;
        world.journal_head = entry.sha256;
    }
    Ok(world)
}
fn encode(world: &World, event: Event) -> Result<(Vec<u8>, String)> {
    let body = EntryBody {
        schema: "ubos.terraforma-local-event.v1".into(),
        sequence: world.sequence + 1,
        previous_sha256: world.journal_head.clone(),
        event,
    };
    let hash = sha256(&json_bytes(&body)?);
    let mut bytes = json_bytes(&Entry {
        body,
        sha256: hash.clone(),
    })?;
    bytes.push(b'\n');
    Ok((bytes, hash))
}

pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("expected a regular file, not a link".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("input byte limit exceeded".into());
    }
    Ok(bytes)
}
fn create_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path).map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())
}
fn create_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|e| e.to_string())
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn new_estate(path: &Path, bytes: &[u8]) -> Result<()> {
    replay(bytes)?;
    create_dir(path)?; // Refuse any existing destination, even an empty one.
    create_file(&path.join(".lock"), b"")?;
    create_file(&path.join("journal.jsonl"), bytes)?;
    sync_dir(path)?;
    sync_dir(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
}
pub fn init(path: &Path, estate_id: String) -> Result<()> {
    let mut world = World::empty();
    let event = Event::Genesis { estate_id };
    world.apply(&event)?;
    let (bytes, _) = encode(&world, event)?;
    new_estate(path, &bytes)
}

pub struct Estate {
    // A stable lock file is held for the complete read/append/export operation.
    _lock: File,
    journal: File,
    world: World,
    bytes: Vec<u8>,
    poisoned: bool,
}
impl Estate {
    pub fn open(path: &Path) -> Result<Self> {
        if !fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_dir()
        {
            return Err("estate must be a real directory".into());
        }
        for name in [".lock", "journal.jsonl"] {
            if !fs::symlink_metadata(path.join(name))
                .map_err(|e| e.to_string())?
                .file_type()
                .is_file()
            {
                return Err("estate files must be regular files".into());
            }
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join(".lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("estate busy or lock unavailable: {e}"))?;
        let bytes = read_bounded(&path.join("journal.jsonl"), MAX_LOG_BYTES)?;
        let world = replay(&bytes)?;
        let journal = OpenOptions::new()
            .append(true)
            .open(path.join("journal.jsonl"))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _lock: lock,
            journal,
            world,
            bytes,
            poisoned: false,
        })
    }
    pub fn world(&self) -> Result<&World> {
        if self.poisoned {
            return Err("an append failed; reopen the estate before continuing".into());
        }
        Ok(&self.world)
    }
    pub fn apply(&mut self, event: Event) -> Result<serde_json::Value> {
        self.world()?;
        if let Event::Put {
            id, title, text, ..
        } = &event
        {
            if self
                .world
                .get(id)
                .is_some_and(|r| r.title == *title && r.text == *text)
            {
                return Ok(
                    serde_json::json!({"status":"unchanged","world_sha256":self.world.digest()?}),
                );
            }
        }
        let before = self.world.digest()?;
        let (encoded, hash) = encode(&self.world, event.clone())?;
        if self.bytes.len() + encoded.len() > MAX_LOG_BYTES as usize {
            return Err("journal byte limit reached".into());
        }
        // Every refusal is decided before the authoritative file is touched; the
        // same world then commits the already-admitted event in memory.
        self.world.check(&event)?;
        self.poisoned = true;
        self.journal
            .write_all(&encoded)
            .and_then(|_| self.journal.sync_all())
            .map_err(|e| format!("append failed; reopen before continuing: {e}"))?;
        self.world.apply(&event)?;
        self.world.sequence += 1;
        self.world.journal_head = hash.clone();
        self.bytes.extend(encoded);
        self.poisoned = false;
        Ok(
            serde_json::json!({"schema":"ubos.terraforma-local-action-receipt.v1","event":event,
            "before_world_sha256":before,"after_world_sha256":self.world.digest()?,
            "journal_head_sha256":hash,"events":self.world.sequence,"status":"committed"}),
        )
    }
    pub fn export(&self, dest: &Path) -> Result<serde_json::Value> {
        self.world()?;
        if dest.exists() {
            return Err("export destination already exists".into());
        }
        let manifest = serde_json::json!({"schema":"ubos.terraforma-local-export.v1",
            "journal_sha256":sha256(&self.bytes),"world_sha256":self.world.digest()?,
            "estate_id":self.world.estate_id,"includes":"retained UTF-8 documents and complete v1 journal"});
        create_dir(dest)?;
        create_file(&dest.join("journal.jsonl"), &self.bytes)?;
        create_file(&dest.join("manifest.json"), &json_bytes(&manifest)?)?;
        sync_dir(dest)?;
        sync_dir(
            dest.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        Ok(manifest)
    }
    /// A portable browser-downloadable envelope of the same journal and manifest.
    pub fn bundle(&self) -> Result<serde_json::Value> {
        self.world()?;
        Ok(serde_json::json!({
            "schema":"ubos.terraforma-local-backup.v1",
            "manifest":{
                "schema":"ubos.terraforma-local-export.v1",
                "journal_sha256":sha256(&self.bytes),"world_sha256":self.world.digest()?,
                "estate_id":self.world.estate_id,
                "includes":"retained UTF-8 documents and complete v1 journal"
            },
            "journal":std::str::from_utf8(&self.bytes).map_err(|e|e.to_string())?
        }))
    }
    pub fn backup(&self, dest: &Path) -> Result<serde_json::Value> {
        let bundle = self.bundle()?;
        create_file(dest, &json_bytes(&bundle)?)?;
        sync_dir(
            dest.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        Ok(bundle["manifest"].clone())
    }
}

pub fn inspect_backup(bytes: &[u8]) -> Result<World> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Manifest {
        schema: String,
        journal_sha256: String,
        world_sha256: String,
        estate_id: String,
        includes: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Bundle {
        schema: String,
        manifest: Manifest,
        journal: String,
    }
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err("backup byte limit exceeded".into());
    }
    let bundle: Bundle = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let world = replay(bundle.journal.as_bytes())?;
    let m = bundle.manifest;
    if bundle.schema != "ubos.terraforma-local-backup.v1"
        || m.schema != "ubos.terraforma-local-export.v1"
        || m.journal_sha256 != sha256(bundle.journal.as_bytes())
        || m.world_sha256 != world.digest()?
        || m.estate_id != world.estate_id
        || m.includes != "retained UTF-8 documents and complete v1 journal"
    {
        return Err("backup does not match retained evidence".into());
    }
    Ok(world)
}
pub fn restore_bundle(bytes: &[u8], dest: &Path) -> Result<()> {
    let world = inspect_backup(bytes)?;
    let bundle: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let journal = bundle
        .get("journal")
        .and_then(serde_json::Value::as_str)
        .ok_or("backup journal required")?;
    if world.estate_id != bundle["manifest"]["estate_id"] {
        return Err("backup estate differs".into());
    }
    new_estate(dest, journal.as_bytes())
}
pub fn restore(source: &Path, dest: &Path) -> Result<()> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_bounded(&source.join("manifest.json"), 4096)?)
            .map_err(|e| e.to_string())?;
    let bytes = read_bounded(&source.join("journal.jsonl"), MAX_LOG_BYTES)?;
    let world = replay(&bytes)?;
    if manifest["schema"] != "ubos.terraforma-local-export.v1"
        || manifest["journal_sha256"] != sha256(&bytes)
        || manifest["world_sha256"] != world.digest()?
        || manifest["estate_id"] != world.estate_id
    {
        return Err("export manifest does not match retained evidence".into());
    }
    new_estate(dest, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expand_rejects_unknown_fields_without_changing_wire_format() {
        assert_eq!(
            serde_json::to_string(&Event::Expand {}).unwrap(),
            r#"{"type":"Expand"}"#
        );
        assert!(serde_json::from_str::<Event>(r#"{"type":"Expand","unexpected":true}"#).is_err());
    }
    #[test]
    fn record_capacity_is_enforced_without_mutation() {
        let mut world = World::empty();
        world
            .apply(&Event::Genesis {
                estate_id: "capacity".into(),
            })
            .unwrap();
        world.sequence = 1;
        let put = |i: usize| Event::Put {
            id: format!("record-{i}"),
            title: "Bounded record".into(),
            text: format!("text {i}"),
            at: None,
        };
        for i in 0..MAX_RECORDS {
            world.apply(&put(i)).unwrap();
            world.sequence += 1;
        }
        let before = world.digest().unwrap();
        assert_eq!(
            world.apply(&put(MAX_RECORDS)).unwrap_err(),
            "record limit reached"
        );
        assert_eq!(world.digest().unwrap(), before);
        // Updating an existing record at capacity is still allowed.
        world.apply(&put(0)).unwrap();
        assert_eq!(world.record_count(), MAX_RECORDS);
    }
    #[test]
    fn failed_write_requires_reopen_before_any_further_operation() {
        let name = format!(
            "terraforma-write-fault-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(name);
        init(&root, "fault-test".into()).unwrap();
        let mut estate = Estate::open(&root).unwrap();
        let before = estate.world().unwrap().digest().unwrap();
        // Inject a definite write failure without changing the authoritative bytes.
        estate.journal = File::open(root.join("journal.jsonl")).unwrap();
        assert!(estate.apply(Event::Expand {}).is_err());
        assert!(estate.apply(Event::Expand {}).is_err());
        assert!(estate.world().is_err());
        assert!(estate.export(&root.join("export")).is_err());
        drop(estate);
        assert_eq!(
            Estate::open(&root)
                .unwrap()
                .world()
                .unwrap()
                .digest()
                .unwrap(),
            before
        );
        fs::remove_dir_all(root).unwrap();
    }
}

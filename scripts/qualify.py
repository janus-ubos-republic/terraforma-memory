#!/usr/bin/env python3
"""Qualify the copied native binary with synthetic, process-separated estates."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from datetime import datetime, timezone


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-parent", type=Path, required=True)
    args = parser.parse_args()
    crate = Path(__file__).resolve().parents[1]
    run = Path(tempfile.mkdtemp(prefix="terraforma-core-", dir=args.output_parent))
    (run / "home").mkdir(mode=0o700)
    binary = run / "terraforma-local-core"
    shutil.copyfile(args.binary, binary)
    binary.chmod(0o700)
    environment = {"HOME": str(run / "home"), "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}
    checks = []

    def check(name, condition):
        assert condition, name
        checks.append(name)

    def call(*parts, ok=True):
        p = subprocess.run([str(binary), *map(str, parts)], cwd=run,
                           env=environment, capture_output=True, text=True, timeout=10)
        if ok:
            assert p.returncode == 0, p.stderr
            return json.loads(p.stdout)
        assert p.returncode != 0, p.stdout
        assert not p.stdout, "failed command emitted success output"
        return json.loads(p.stderr)

    lineage = json.loads((crate / "lineage.json").read_text())
    check("three_inherited_modules_match_committed_source",
          len(lineage["inherited_files"]) == 3 and all(
              digest(crate / entry["local_path"]) == entry["sha256"]
              for entry in lineage["inherited_files"]))
    check("copied_binary_identical", digest(binary) == digest(args.binary))
    check("binary_embeds_selected_lineage", call("version")["source_lineage"] == lineage)
    estate = run / "estate"
    empty = call("init", estate, "synthetic-project")
    check("fresh_private_estate", empty["ring"] == 0 and empty["record_count"] == 0
          and estate.stat().st_mode & 0o777 == 0o700
          and (estate / "journal.jsonl").stat().st_mode & 0o777 == 0o600)
    doc = run / "decision.md"
    doc.write_text("Project Atlas: retention period is 30 days.\n")
    receipt = call("put", estate, "decision-001", "Retention decision", doc)
    base = call("status", estate)
    result = call("get", estate, "decision-001")
    check("document_retained_and_source_bound", result["record"]["text"] == doc.read_text()
          and result["record"]["source_sha256"] == digest(doc)
          and result["world_sha256"] == base["world_sha256"])
    check("action_receipt_binds_before_and_after", receipt["before_world_sha256"] == empty["world_sha256"]
          and receipt["after_world_sha256"] == base["world_sha256"])
    check("absent_record_is_explicit", call("get", estate, "absent")["status"] == "not_found")
    before_noop = digest(estate / "journal.jsonl")
    check("identical_admission_is_noop", call("put", estate, "decision-001", "Retention decision", doc)["status"] == "unchanged"
          and digest(estate / "journal.jsonl") == before_noop)
    doc.write_text("Project Atlas: retention period is 60 days.\n")
    check("external_edits_do_not_silently_rewrite_retained_evidence",
          "30 days" in call("get", estate, "decision-001")["record"]["text"])
    call("expand", estate)
    call("put", estate, "decision-001", "Retention decision", doc)
    call("put", estate, "ring-one-note", "New work", doc)
    expanded = call("status", estate)
    check("new_ring_revision_survives_process_restart", expanded["ring"] == 1
          and expanded["record_count"] == 2 and "60 days" in call("get", estate, "decision-001")["record"]["text"])
    archive = run / "export"
    exported = call("export", estate, archive)
    restored = run / "restored"
    restored_status = call("restore", archive, restored)
    check("export_restore_same_world_and_complete_journal",
          restored_status == expanded and exported["world_sha256"] == expanded["world_sha256"]
          and digest(restored / "journal.jsonl") == digest(estate / "journal.jsonl"))
    check("restored_document_bytes_match", call("get", restored, "decision-001") == call("get", estate, "decision-001"))
    call("collapse", estate, 0)
    collapsed = call("status", estate)
    check("collapse_restores_earlier_world_digest", collapsed["world_sha256"] == base["world_sha256"]
          and collapsed["events"] > base["events"])
    check("collapse_restores_content_and_removes_outer_records",
          "30 days" in call("get", estate, "decision-001")["record"]["text"]
          and call("get", estate, "ring-one-note")["status"] == "not_found")

    def rejected_without_write(name, *command):
        before = digest(estate / "journal.jsonl")
        call(*command, ok=False)
        check(name, before == digest(estate / "journal.jsonl"))

    rejected_without_write("invalid_collapse_preserves_journal", "collapse", estate, 30)
    rejected_without_write("unsafe_id_preserves_journal", "put", estate, "../wrong", "Title", doc)
    bad = run / "invalid.txt"
    bad.write_bytes(b"\xff\xfe")
    rejected_without_write("invalid_utf8_preserves_journal", "put", estate, "bad", "Title", bad)
    bad.write_bytes(b"x" * (16 * 1024 + 1))
    rejected_without_write("oversized_source_preserves_journal", "put", estate, "large", "Title", bad)
    link = run / "linked.md"
    link.symlink_to(doc)
    rejected_without_write("source_symlink_rejected", "put", estate, "linked", "Title", link)
    with (estate / ".lock").open("r+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        error = call("expand", estate, ok=False)
        check("second_process_cannot_write_while_locked", "estate busy" in error["error"])
    check("lock_released_after_holder_exits", call("status", estate) == collapsed)
    for operation in [("init", estate, "replacement"), ("restore", archive, estate), ("export", estate, archive)]:
        call(*operation, ok=False)
    check("existing_destinations_preserved", call("status", estate) == collapsed
          and digest(archive / "journal.jsonl") == exported["journal_sha256"])

    good = (restored / "journal.jsonl").read_bytes()
    lines = good.splitlines(keepends=True)
    mutations = {
        "modified_content_rejected": good.replace(b"60 days", b"61 days", 1),
        "torn_final_entry_rejected": good[:-1],
        "malformed_middle_entry_rejected": lines[0] + b"not-json\n" + b"".join(lines[1:]),
        "duplicate_event_rejected": good + lines[-1],
        "reordered_events_rejected": lines[0] + lines[2] + lines[1] + b"".join(lines[3:]),
        "empty_journal_rejected": b"",
        "legacy_wal_not_silently_imported": b'{"MemorySeeded":{"coordinate":1,"label":"old"}}\n',
    }
    altered = json.loads(lines[-1])
    altered["unexpected"] = True
    mutations["unknown_fields_rejected"] = b"".join(lines[:-1]) + json.dumps(altered).encode() + b"\n"
    for name, data in mutations.items():
        damaged = run / name
        shutil.copytree(restored, damaged)
        (damaged / "journal.jsonl").write_bytes(data)
        before = digest(damaged / "journal.jsonl")
        call("status", damaged, ok=False)
        call("expand", damaged, ok=False)
        check(name, digest(damaged / "journal.jsonl") == before)
    damaged_export = run / "damaged-export"
    shutil.copytree(archive, damaged_export)
    (damaged_export / "journal.jsonl").write_bytes(b"".join(lines[:-1]))
    dest = run / "must-not-be-created"
    call("restore", damaged_export, dest, ok=False)
    check("whole_event_truncation_detected_against_export_manifest", not dest.exists())
    missing = run / "missing-journal"
    missing.mkdir()
    (missing / ".lock").touch()
    call("status", missing, ok=False)
    check("missing_journal_not_replaced_with_empty_world", not (missing / "journal.jsonl").exists())

    ring_limit = run / "ring-limit"
    call("init", ring_limit, "ring-limit")
    for _ in range(64):
        call("expand", ring_limit)
    before = digest(ring_limit / "journal.jsonl")
    call("expand", ring_limit, ok=False)
    check("ring_capacity_is_enforced_without_mutation", call("status", ring_limit)["ring"] == 64
          and digest(ring_limit / "journal.jsonl") == before)
    # The 5,000-record capacity is exercised in-process by the Rust unit test
    # record_capacity_is_enforced_without_mutation; spawning 5,000 CLI writes here
    # would only measure process start-up.
    # A valid hash chain cannot authorize an invalid state transition.
    forged = run / "invalid-transition"
    shutil.copytree(restored, forged)
    last = json.loads(lines[-1])
    body = {"schema": "ubos.terraforma-local-event.v1", "sequence": last["body"]["sequence"] + 1,
            "previous_sha256": last["sha256"], "event": {"type": "Collapse", "stable_ring": 60}}
    entry = {"body": body, "sha256": hashlib.sha256(json.dumps(body, separators=(",", ":")).encode()).hexdigest()}
    (forged / "journal.jsonl").write_bytes(good + json.dumps(entry).encode() + b"\n")
    error = call("status", forged, ok=False)
    check("valid_hash_does_not_bypass_transition_rules", "collapse requires" in error["error"])

    sources = [crate / "Cargo.toml", crate / "Cargo.lock", crate / "lineage.json", *sorted((crate / "src").rglob("*.rs")), *sorted((crate / "web").glob("*")), Path(__file__).resolve()]
    evidence = {
        "schema": "ubos.terraforma-local-core-qualification.v1",
        "observed_at": datetime.now(timezone.utc).isoformat(), "status": "PASS",
        "scope": "synthetic copied-binary M4 core rehearsal; not full installer, legacy migration, clean OS or customer proof",
        "platform": os.uname().sysname + " " + os.uname().machine,
        "run_directory": str(run), "binary": {"path": str(binary), "sha256": digest(binary), "bytes": binary.stat().st_size},
        "source_bindings": [{"path": str(p.relative_to(crate)), "sha256": digest(p)} for p in sources],
        "checks": checks, "check_count": len(checks), "example_base_world_sha256": base["world_sha256"],
        "example_expanded_world_sha256": expanded["world_sha256"], "restored_status": restored_status,
        "model_calls": 0, "network_calls": 0,
        "remaining": ["Browser and Kinematic retrieval integration", "Install/update/uninstall lifecycle", "Linux and native Windows qualification", "Historical WAL migration and full UR3 lineage", "Crash/power-loss fault injection", "Signed distribution and customer proof"],
    }
    receipt = run / "qualification.json"
    receipt.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({"status": "PASS", "checks": len(checks), "receipt": str(receipt),
                      "receipt_sha256": digest(receipt), "binary_bytes": binary.stat().st_size}, indent=2))


if __name__ == "__main__":
    main()

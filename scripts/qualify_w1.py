#!/usr/bin/env python3
"""W1 acceptance: two assistants share one memory through the Gate, one is revoked,
the other keeps working, and the estate restores independently on a clean path.
Synthetic data only.

Local:   qualify_w1.py <binary> [receipt.json]
Remote:  qualify_w1.py <local-binary> [receipt.json] --ssh HOST --remote-bin PATH
         The estate, the Gate and its files live on HOST; the client reaches the Gate
         through an SSH tunnel; the backup made on HOST is restored here, so the
         restore crosses a machine (and an operating system, if they differ)."""
import argparse, json, pathlib, shlex, shutil, subprocess, sys, tempfile, time
import urllib.error, urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("binary")
ap.add_argument("receipt", nargs="?")
ap.add_argument("--ssh")
ap.add_argument("--remote-bin")
ap.add_argument("--port", type=int, default=38765)
opt = ap.parse_args()
REMOTE = bool(opt.ssh)
if REMOTE and not opt.remote_bin:
    sys.exit("--ssh needs --remote-bin")
results = []


def check(name, ok, detail=""):
    results.append({"check": name, "pass": bool(ok), "detail": str(detail)[:300]})
    print(("PASS " if ok else "FAIL ") + name + (f"  [{str(detail)[:200]}]" if detail and not ok else ""))


def sh(cmd):  # run a shell command on the machine that holds the estate
    if REMOTE:
        return subprocess.run(["ssh", "-o", "BatchMode=yes", opt.ssh, cmd], capture_output=True, text=True)
    return subprocess.run(cmd, shell=True, capture_output=True, text=True)


def cli(*args, expect_ok=True):
    """Run the binary where the estate lives."""
    b = opt.remote_bin if REMOTE else opt.binary
    p = sh(" ".join(shlex.quote(x) for x in (b, *args)))
    if expect_ok and p.returncode != 0:
        raise SystemExit(f"command failed: {args}\n{p.stderr}")
    return p


def local(*args, expect_ok=True):
    p = subprocess.run([opt.binary, *args], capture_output=True, text=True)
    if expect_ok and p.returncode != 0:
        raise SystemExit(f"command failed: {args}\n{p.stderr}")
    return p


def post(url, token, payload, origin=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    if origin:
        headers["Origin"] = origin
    req = urllib.request.Request(url, json.dumps(payload).encode(), headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=8) as r:
            body = r.read()
            return r.status, json.loads(body) if body else None
    except urllib.error.HTTPError as e:
        body = e.read()
        return e.code, json.loads(body) if body else None


def call(url, token, name, args, rid=1):
    return post(url, token, {"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                             "params": {"name": name, "arguments": args}})


def text_of(resp):
    return resp[1]["result"]["content"][0]["text"]


# Where the estate lives
if REMOTE:
    W = sh("mktemp -d /tmp/w1-XXXXXX").stdout.strip()
    assert W.startswith("/tmp/w1-"), "could not make a remote work directory"
else:
    W = tempfile.mkdtemp(prefix="w1-")
estate, gate_dir, bundle = f"{W}/estate", f"{W}/gate", f"{W}/backup.json"
lwork = pathlib.Path(tempfile.mkdtemp(prefix="w1-local-"))
server = None
try:
    cli("init", estate, "w1-synthetic")
    tokens = {}
    for name, scope in (("assistant-a", "write"), ("assistant-b", "write"), ("auditor-c", "read")):
        tokens[name] = json.loads(cli("principal-add", gate_dir, name, scope).stdout)["token"]
    if REMOTE:
        server = subprocess.Popen(
            ["ssh", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes",
             "-L", f"{opt.port}:127.0.0.1:{opt.port}", opt.ssh,
             f"exec {shlex.quote(opt.remote_bin)} gate-serve {shlex.quote(estate)} {shlex.quote(gate_dir)} {opt.port}"],
            stdout=subprocess.PIPE, text=True)
    else:
        server = subprocess.Popen([opt.binary, "gate-serve", estate, gate_dir, "0"], stdout=subprocess.PIPE, text=True)
    url = json.loads(server.stdout.readline())["url"]
    A, B, C = (tokens[n] for n in ("assistant-a", "assistant-b", "auditor-c"))
    if REMOTE:
        remote_host = sh("hostname").stdout.strip()
        local_host = subprocess.run(["hostname"], capture_output=True, text=True).stdout.strip()
        check("the Gate runs on a different machine, reached through the tunnel",
              bool(remote_host) and remote_host != local_host,
              f"{remote_host} ({sh('uname -sm').stdout.strip()}) vs {local_host}")

    # 1. A saves a decision; B retrieves exactly the same text.
    decision = ("Synthetic decision W1: the invoices service retries three times with "
                "exponential back-off; chosen on 2026-11-25 because the vendor API drops bursts.")
    st, r = post(url, A, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                          "params": {"protocolVersion": "2025-06-18"}})
    check("A initializes through the Gate", st == 200 and r["result"]["serverInfo"]["name"] == "terraforma-memory")
    st, r = call(url, A, "remember", {"title": "Invoice retry policy", "text": decision})
    check("A saves a decision", st == 200 and not r["result"]["isError"], r)
    st, r = call(url, B, "recall", {"query": "invoices retries back-off"})
    found = st == 200 and "Invoice retry policy" in text_of((st, r))
    check("B finds it by words", found)
    mid = None
    if found:
        mid = next((w.strip("`") for w in text_of((st, r)).split() if w.startswith("`") and w.endswith("`")), None)
    st, r = call(url, B, "read_memory", {"id": mid} if mid else {"id": "invoice-retry-policy"})
    check("B reads the exact text A saved", st == 200 and decision in text_of((st, r)), r)

    # 2. Refusals: no token, wrong token, browser origin, read-only write.
    st, _ = post(url, None, {"jsonrpc": "2.0", "id": 2, "method": "ping"})
    check("no token is refused (401)", st == 401)
    st, _ = post(url, "tfg_" + "0" * 64, {"jsonrpc": "2.0", "id": 2, "method": "ping"})
    check("wrong token is refused (401)", st == 401)
    st, _ = post(url, B, {"jsonrpc": "2.0", "id": 2, "method": "ping"}, origin="http://evil.example")
    check("a browser Origin is refused (403)", st == 403)
    st, r = call(url, C, "remember", {"title": "Sneaky write", "text": "should not land"})
    check("read-only principal cannot write", st == 200 and r["result"]["isError"] and "read-only" in text_of((st, r)))
    st, r = call(url, C, "recall", {"query": "invoices"})
    check("read-only principal can read", st == 200 and "Invoice retry policy" in text_of((st, r)))
    check("the refused write left no memory", "Sneaky write" not in cli("query", estate, "sneaky").stdout)

    # 3. Revoke A. A is refused on the very next call; B is unaffected.
    cli("principal-revoke", gate_dir, "assistant-a")
    st, _ = call(url, A, "recall", {"query": "invoices"})
    check("revoked A is refused on the next call", st == 401)
    st, r = call(url, B, "recall", {"query": "invoices"})
    check("B keeps working after A is revoked", st == 200 and "Invoice retry policy" in text_of((st, r)))
    st, r = call(url, B, "remember", {"title": "Written after revoke", "text": "B can still write."})
    check("B can still write after A is revoked", st == 200 and not r["result"]["isError"])

    # 4. Independent restore: the bundle is made where the estate lives and restored HERE.
    cli("backup", estate, bundle)
    lbundle = lwork / "backup.json"
    if REMOTE:
        data = subprocess.run(["ssh", "-o", "BatchMode=yes", opt.ssh, f"cat {shlex.quote(bundle)}"], capture_output=True).stdout
        lbundle.write_bytes(data)
    else:
        shutil.copy(bundle, lbundle)
    clean = lwork / "restored"
    local("restore-backup", str(lbundle), str(clean))
    live = json.loads(cli("status", estate).stdout)
    back = json.loads(local("status", str(clean)).stdout)
    check("restored estate has the same digest and count",
          live["world_sha256"] == back["world_sha256"] and live["record_count"] == back["record_count"] >= 2,
          f"{live['world_sha256'][:12]} {live['record_count']} vs {back['world_sha256'][:12]} {back['record_count']}")
    check("restored estate answers the same question",
          "Invoice retry policy" in local("query", str(clean), "invoices retries").stdout)
    tampered = bytearray(lbundle.read_bytes())
    tampered[len(tampered) // 2] ^= 1
    (lwork / "tampered.json").write_bytes(bytes(tampered))
    p = local("restore-backup", str(lwork / "tampered.json"), str(lwork / "bad"), expect_ok=False)
    check("a tampered backup is refused", p.returncode != 0 and not (lwork / "bad").exists())

    # 5. Audit trail: who did what, never the content.
    audit = sh(f"cat {shlex.quote(gate_dir)}/audit.jsonl").stdout
    lines = [json.loads(x) for x in audit.splitlines()]
    who = {x["principal"] for x in lines}
    check("audit names every principal that acted", {"assistant-a", "assistant-b", "auditor-c"} <= who)
    check("audit records the revoke and the refusals",
          any(x["action"] == "principal.revoke" for x in lines)
          and any(x["outcome"] == "unauthorized" for x in lines)
          and any(x["outcome"] == "denied_read_only" for x in lines))
    check("audit never contains memory text", "exponential" not in audit and "back-off" not in audit)
    stat_cmd = f"stat -c %a {shlex.quote(gate_dir)}/principals.json" if REMOTE else f"stat -f %Lp {shlex.quote(gate_dir)}/principals.json"
    if REMOTE:
        mode = sh(stat_cmd).stdout.strip()
    else:
        mode = sh(stat_cmd).stdout.strip()
    check("principal file is private (0600)", mode == "600", mode)
    stored = sh(f"cat {shlex.quote(gate_dir)}/principals.json").stdout
    check("tokens are stored only as hashes", all(t not in stored for t in tokens.values()))
finally:
    if server:
        server.terminate()
        try:
            server.wait(timeout=5)
        except Exception:
            server.kill()
    if REMOTE:
        sh(f"pkill -f 'gate-serve {W}' ; rm -rf {shlex.quote(W)}")
    else:
        shutil.rmtree(W, ignore_errors=True)
    shutil.rmtree(lwork, ignore_errors=True)

passed = all(r["pass"] for r in results)
receipt = {"schema": "terraforma.w1.receipt.v1", "mode": "remote-ssh-tunnel" if REMOTE else "local",
           "binary": opt.binary, "remote_binary": opt.remote_bin, "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "passed": passed, "checks": results}
print(json.dumps({"W1": "PASS" if passed else "FAIL", "mode": receipt["mode"], "checks": len(results),
                  "failed": [r["check"] for r in results if not r["pass"]]}))
if opt.receipt:
    pathlib.Path(opt.receipt).write_text(json.dumps(receipt, indent=1))
sys.exit(0 if passed else 1)

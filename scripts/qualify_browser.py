#!/usr/bin/env python3
"""Process and HTTP qualification against a copied binary in a synthetic estate."""
import argparse
from datetime import datetime, timezone
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import select
import shutil
import socket
import subprocess
import tempfile
import time


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output-parent', type=Path, required=True)
    args = parser.parse_args()
    crate = Path(__file__).resolve().parents[1]
    run = Path(tempfile.mkdtemp(prefix='terraforma-browser-', dir=args.output_parent))
    (run / 'home').mkdir(mode=0o700)
    binary = run / 'terraforma-local-core'
    shutil.copyfile(args.binary, binary)
    binary.chmod(0o700)
    env = {'HOME': str(run / 'home'), 'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'}
    checks, processes = [], []

    def check(name, condition):
        assert condition, name
        checks.append(name)

    def cli(*parts, ok=True):
        result = subprocess.run([str(binary), *map(str, parts)], cwd=run, env=env,
                                capture_output=True, text=True, timeout=10)
        assert (result.returncode == 0) == ok, result.stderr
        return json.loads(result.stdout if ok else result.stderr)

    def start(estate):
        log = (run / f'server-{len(processes)}.stderr').open('w')
        process = subprocess.Popen([str(binary), 'serve', str(estate), '0'], cwd=run,
                                   env=env, stdout=subprocess.PIPE, stderr=log, text=True)
        processes.append(process)
        assert select.select([process.stdout], [], [], 10)[0], 'server startup timeout'
        line = process.stdout.readline()
        ready = json.loads(line)
        host = ready['url'].removeprefix('http://')
        port = int(host.split(':')[-1])
        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
        conn.request('GET', '/')
        response = conn.getresponse()
        page = response.read().decode()
        headers = dict(response.getheaders())
        assert response.status == 200
        token = re.search(r'name="workspace-token" content="([a-f0-9]{64})"', page).group(1)
        conn.close()
        return {'process': process, 'host': host, 'port': port, 'token': token, 'page': page, 'headers': headers}

    def request(server, action=None, fields=None, status=200, headers=None, method='POST', path='/api', raw_body=None):
        body = raw_body if raw_body is not None else json.dumps({'action': action, **(fields or {})}).encode()
        hs = {'Content-Type': 'application/json', 'Origin': 'http://' + server['host'],
              'X-Workspace-Token': server['token'], **(headers or {})}
        hs = {k: v for k, v in hs.items() if v is not None}
        conn = http.client.HTTPConnection('127.0.0.1', server['port'], timeout=6)
        conn.request(method, path, body=body, headers=hs)
        response = conn.getresponse()
        content = response.read()
        assert response.status == status, (status, response.status, content[:500])
        conn.close()
        return json.loads(content)

    def raw(server, data):
        with socket.create_connection(('127.0.0.1', server['port']), timeout=5) as s:
            s.sendall(data)
            s.shutdown(socket.SHUT_WR)
            return s.recv(4096)

    def head(server):
        return request(server, 'status')['status']['journal_head_sha256']

    def put(server, id_, title, text, expected=None, status=200):
        return request(server, 'put', {'id': id_, 'title': title, 'text': text,
                                     'expected_head': expected or head(server)}, status=status)

    try:
        estate = run / 'estate'
        cli('init', estate, 'browser-rehearsal')
        server = start(estate)
        check('copied_binary_runs_without_repo_or_founder_home', sha(binary) == sha(args.binary))
        check('loopback_only_readiness_without_session_token', server['host'].startswith('127.0.0.1:') and server['token'] not in server['host'])
        check('browser_assets_embedded_and_cache_disabled', 'Try a sample' in server['page'] and server['headers']['Cache-Control'] == 'no-store')
        check('browser_security_headers', "frame-ancestors 'none'" in server['headers']['Content-Security-Policy'] and server['headers']['Cross-Origin-Resource-Policy'] == 'same-origin')
        for name, headers in [
            ('missing_token', {'X-Workspace-Token': None}),
            ('wrong_token', {'X-Workspace-Token': 'wrong'}),
            ('missing_origin', {'Origin': None}),
            ('foreign_origin', {'Origin': 'https://example.invalid'}),
            ('foreign_host', {'Host': 'example.invalid'}),
            ('cross_site', {'Sec-Fetch-Site': 'cross-site'}),
            ('same_site_other_origin', {'Sec-Fetch-Site': 'same-site'}),
        ]:
            request(server, 'status', headers=headers, status=403)
            check(name + '_refused', True)
        request(server, 'status', headers={'Content-Type':'text/plain'}, status=400)
        request(server, 'unknown_action', status=400)
        request(server, 'status', {'unexpected': True}, status=400)
        check('invalid_actions_types_and_fields_refused', True)
        for path in ['/journal.jsonl', '/../journal.jsonl', '/%2e%2e/journal.jsonl']:
            request(server, method='GET', path=path, raw_body=b'', status=404)
        check('no_filesystem_routes', True)
        for suffix in [b'Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}',
                       b'Content-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n{}',
                       b'Content-Length: 99999999\r\n\r\n',
                       b'X-Large: ' + b'x' * 9000 + b'\r\n\r\n',
                       b'Content-Length: 2\r\n\r\n{}EXTRA']:
            result = raw(server, b'POST /api HTTP/1.1\r\nHost: ' + server['host'].encode() + b'\r\n' + suffix)
            assert b' 400 ' in result[:40], result[:80]
        check('duplicate_chunked_oversized_and_extra_framing_refused', True)

        # Occupy all workers with incomplete requests. A normal page request
        # must wait for their deadlines rather than lose a required asset.
        held = []
        try:
            for _ in range(4):
                s = socket.create_connection(('127.0.0.1',server['port']),timeout=5)
                s.sendall(b'GET / HTTP/1.1\r\nHost: ' + server['host'].encode() + b'\r\n')
                held.append(s)
            time.sleep(0.1)
            conn = http.client.HTTPConnection('127.0.0.1',server['port'],timeout=6)
            conn.request('GET','/')
            response = conn.getresponse()
            page = response.read()
            check('worker_pressure_queues_valid_request_until_deadline',response.status==200 and b'Try a sample' in page)
            conn.close()
        finally:
            for s in held:
                s.close()

        text = 'Project 7 — Maple studio\nReview date: 18 September.\nBudget 2400 EUR.\nMálaga office.\n'
        put(server, 'maple', 'Maple project 7', text)
        original_head = head(server)
        put(server, 'maple-copy', 'Archived Maple', text)
        put(server, 'other', 'Project 142', 'Project 142 has a separate budget of 800 EUR.')
        original = request(server, 'read', {'id':'maple'})
        check('saved_source_bytes_and_hash', original['record']['text'] == text and original['record']['source_sha256'] == hashlib.sha256(text.encode()).hexdigest())
        same = put(server, 'maple', 'Maple project 7', text)
        check('identical_save_is_noop', same['status'] == 'unchanged')
        q = request(server, 'query', {'query':'project 7'})
        check('number_identifier_matches_whole_token', q['result']['hits'][0]['id'] == 'maple' and q['result']['hits'][0]['status'] == 'full_match')
        check('duplicate_archive_text_grouped', q['result']['hits'][0]['source_ids_with_identical_text'] == ['maple', 'maple-copy'])
        numeric = request(server, 'query', {'query':'7'})
        check('single_digit_is_searchable_without_substring_matches', len(numeric['result']['hits']) == 1 and numeric['result']['hits'][0]['id'] == 'maple')
        check('unicode_case_matching', request(server,'query',{'query':'MÁLAGA'})['result']['status'] == 'full_match')
        partial = request(server, 'query', {'query':'project 7 unicorn'})
        check('partial_match_explicit', partial['result']['status'] == 'partial_match' and 'unicorn' in partial['result']['hits'][0]['missing_terms'])
        check('absent_answer_explicit', request(server, 'query', {'query':'quantum zebra'})['result']['status'] == 'no_match')
        for query in ['', '!', 'x' * 513, ' '.join(str(i) for i in range(33))]:
            request(server, 'query', {'query':query}, status=400)
        check('bounded_queries', True)
        q2 = request(server, 'query', {'query':'project 7'})
        expected_digest = hashlib.sha256(json.dumps(q['result'], sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode()).hexdigest()
        check('query_result_and_digest_reproducible', q['result'] == q2['result'] and q['result_sha256'] == q2['result_sha256'] == expected_digest)
        before = sha(estate/'journal.jsonl')
        put(server, 'maple', 'Stale overwrite', 'must not commit', expected=original_head, status=409)
        put(server, '../unsafe', 'Unsafe', 'text', status=400)
        put(server, 'oversize', 'Too much', 'x' * 16385, status=400)
        check('stale_invalid_and_oversized_writes_preserve_journal', sha(estate/'journal.jsonl') == before)
        cli('expand', estate, ok=False)
        check('server_holds_single_writer_lock', sha(estate/'journal.jsonl') == before)
        request(server, 'checkpoint', {'expected_head': head(server)})
        base_world = request(server, 'status')['status']['world_sha256']
        changed_text = text.replace('2400', '3200')
        put(server, 'maple', 'Maple project 7', changed_text)
        request(server, 'read', {'id':'maple','expected_source_sha256':original['record']['source_sha256']}, status=409)
        check('old_search_cannot_open_changed_source_silently', True)
        changed = request(server, 'query', {'query':'3200'})
        check('saved_edit_immediately_searchable', changed['result']['status'] == 'full_match')
        backup = request(server, 'backup')
        backup_path = run/'downloaded-backup.json'
        backup_path.write_text(json.dumps(backup))
        request(server, 'stop')
        server['process'].wait(timeout=6)
        check('clean_stop_preserves_saved_state', server['process'].returncode == 0)
        check('cli_and_browser_share_identical_query_contract', cli('query', estate, '3200')['result_sha256'] == changed['result_sha256'])
        resumed = start(estate)
        check('restart_rotates_session_and_replays_same_result', resumed['token'] != server['token'] and request(resumed, 'query', {'query':'3200'})['result_sha256'] == changed['result_sha256'])
        request(resumed, 'status', headers={'X-Workspace-Token':server['token']}, status=403)
        check('old_session_refused_after_restart', True)
        request(resumed, 'rollback', {'ring':0,'expected_head':head(resumed)})
        check('checkpoint_restores_earlier_source', request(resumed,'query',{'query':'3200'})['result']['status'] == 'no_match' and request(resumed,'read',{'id':'maple'})['record']['text'] == text)
        request(resumed,'stop'); resumed['process'].wait(timeout=6)
        restored = run/'restored'
        cli('restore-backup',backup_path,restored)
        recovered = start(restored)
        check('downloaded_backup_restores_identical_browser_result', request(recovered,'query',{'query':'3200'})['result_sha256'] == changed['result_sha256'])
        check('backup_retains_full_history', (restored/'journal.jsonl').read_text() == backup['journal'])
        request(recovered,'stop'); recovered['process'].wait(timeout=6)
        cli('restore-backup',backup_path,restored,ok=False)
        check('backup_restore_refuses_existing_destination', (restored/'journal.jsonl').read_text() == backup['journal'])
        altered = json.loads(backup_path.read_text()); altered['journal'] = altered['journal'].replace('3200','3201',1)
        (run/'tampered.json').write_text(json.dumps(altered))
        cli('restore-backup',run/'tampered.json',run/'must-not-exist',ok=False)
        check('tampered_backup_refused_before_creation', not (run/'must-not-exist').exists())
        altered = json.loads(backup_path.read_text()); altered['unexpected']=True
        (run/'unknown.json').write_text(json.dumps(altered))
        cli('restore-backup',run/'unknown.json',run/'must-not-exist',ok=False)
        check('backup_unknown_fields_refused', not (run/'must-not-exist').exists())
        cli('backup',restored,run/'cli-backup.json')
        check('cli_backup_same_envelope_as_browser', json.loads((run/'cli-backup.json').read_text()) == backup)
        sources = [crate/'Cargo.toml',crate/'Cargo.lock',crate/'lineage.json',
                   *sorted((crate/'src').rglob('*.rs')),*sorted((crate/'web').glob('*')),Path(__file__).resolve()]
        receipt = {'schema':'ubos.terraforma-browser-qualification.v1','status':'PASS_LOCAL_M4_REHEARSAL',
                   'observed_at':datetime.now(timezone.utc).isoformat(),'run_directory':str(run),
                   'binary':{'path':str(binary),'sha256':sha(binary),'bytes':binary.stat().st_size},
                   'source_bindings':[{'path':str(p.relative_to(crate)),'sha256':sha(p)} for p in sources],
                   'checks':checks,'check_count':len(checks),'query_result_sha256':changed['result_sha256'],
                   'backup_sha256':sha(backup_path),'restored_journal_sha256':sha(restored/'journal.jsonl'),
                   'boundary':'Synthetic local process/HTTP qualification; visual browser journey is separate; no clean-OS, installer, Linux, Windows, real customer, multiuser or public deployment claim.'}
        path=run/'qualification.json';path.write_text(json.dumps(receipt,indent=2)+'\n')
        print(json.dumps({'status':receipt['status'],'checks':len(checks),'receipt':str(path),'sha256':sha(path)},indent=2))
    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired: process.kill();process.wait(timeout=5)


if __name__ == '__main__':
    main()

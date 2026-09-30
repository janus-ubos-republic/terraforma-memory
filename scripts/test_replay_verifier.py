import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/debug/terraforma-local-core'
STOPS = ['no_m4_owner_access', 'no_m4_state_write', 'no_local_core_admission', 'no_source_authority_change']

def sha(value): return hashlib.sha256(Path(value).read_bytes()).hexdigest()
def call(*args, check=True):
    result = subprocess.run([str(BINARY), *map(str, args)], text=True, capture_output=True, timeout=15)
    if check and result.returncode: raise AssertionError(result.stderr)
    return result

class ReplayVerifierTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name);self.estate=self.root/'estate';self.package=self.root/'package';self.package.mkdir()
        call('init',self.estate,'replay-fixture')
        status=self.root/'status.json';status.write_text('{"state":"fresh","stops":"display only"}\n')
        boundary=self.root/'boundary.json';boundary.write_text(json.dumps({'required_stops':STOPS},sort_keys=True))
        call('put',self.estate,'m4-status-product','M4 product status',status);call('put',self.estate,'m4-product-boundary','M4 product boundary',boundary);call('backup',self.estate,self.package/'workspace-backup.json')
        (self.package/'desk-product-export.json').write_text('{"schema":"ubos.m4-desk-product-export.v1","status":"read_only_export"}');(self.package/'proposal.json').write_text('{"sealed":true}');(self.package/'manifest.json').write_text('{"sealed":true}')
        presentation={'schema':'ubos.m4-local-core-product-presentation.v1','state':'display_only','presentation_id':'fixture-presentation','source_binding':{'kind':'m4-desk-product-export','sha256':sha(self.package/'desk-product-export.json'),'bytes':(self.package/'desk-product-export.json').stat().st_size},'proposal_sha256':sha(self.package/'proposal.json'),'manifest_sha256':sha(self.package/'manifest.json'),'status_record':{'id':'m4-status-product','title':'M4 product status','source_sha256':sha(status)},'required_stops':STOPS}
        (self.package/'presentation.json').write_text(json.dumps(presentation,sort_keys=True))
        names=('desk-product-export.json','proposal.json','manifest.json','presentation.json','workspace-backup.json');files={name:{'sha256':sha(self.package/name),'bytes':(self.package/name).stat().st_size}for name in names}
        package={'schema':'ubos.m4-desk-product-replay-package.v1','state':'display_only_replay_package','source_binding':presentation['source_binding'],'proposal_sha256':presentation['proposal_sha256'],'manifest_sha256':presentation['manifest_sha256'],'presentation_id':presentation['presentation_id'],'status_record':presentation['status_record'],'boundary_record':{'id':'m4-product-boundary','title':'M4 product boundary'},'required_stops':STOPS,'files':files}
        (self.package/'package.json').write_text(json.dumps(package,sort_keys=True))
    def tearDown(self): self.temp.cleanup()
    def test_verifies_hashes_backup_and_stops(self):
        value=json.loads(call('verify-desk-replay',self.package).stdout);self.assertEqual(value['status'],'verified_display_only_replay_package');self.assertEqual(value['records'],2);self.assertEqual(value['required_stops'],STOPS)
    def test_refuses_tampering_before_any_mutation(self):
        (self.package/'presentation.json').write_text('{"tampered":true}');refused=call('verify-desk-replay',self.package,check=False);self.assertNotEqual(refused.returncode,0);self.assertIn('replay file hash differs',refused.stderr)
if __name__=='__main__': unittest.main()

import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import package_macos
import package_linux


class PackageTests(unittest.TestCase):
    def make_package(self, root: Path) -> tuple[Path, Path]:
        binary = root / "candidate"
        binary.write_text("#!/bin/sh\nprintf '%s\\n' '{\"status\":\"stub\"}'\n", encoding="utf-8")
        binary.chmod(0o700)
        output = root / "deliveries"
        output.mkdir()
        result = package_macos.build(binary, output, "0.2.0")
        return Path(result["archive"]), output

    def extract(self, archive: Path, root: Path) -> Path:
        with tarfile.open(archive) as tar:
            tar.extractall(root, filter="data")
        return next(root.iterdir())

    def test_clean_install_runs_binary_and_preserves_estate_on_uninstall(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); archive, _ = self.make_package(root)
            source = self.extract(archive, root / "extract") if (root / "extract").exists() else None
            if source is None:
                (root / "extract").mkdir(); source = self.extract(archive, root / "extract")
            prefix = root / "apps" / "terraforma-local-core"
            installed = subprocess.run(["sh", source / "install.sh", prefix], text=True, capture_output=True)
            self.assertEqual(installed.returncode, 0, installed.stderr)
            self.assertEqual(json.loads(subprocess.check_output([prefix / "bin" / "terraforma-local-core"], text=True))["status"], "stub")
            estate = root / "explicit-estate"; estate.mkdir(); (estate / "journal.jsonl").write_text("kept")
            removed = subprocess.run(["sh", prefix / "share" / "uninstall.sh", prefix], text=True, capture_output=True)
            self.assertEqual(removed.returncode, 0, removed.stderr)
            self.assertFalse(prefix.exists()); self.assertEqual((estate / "journal.jsonl").read_text(), "kept")

    def test_corrupted_source_refuses_install_without_destination(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); archive, _ = self.make_package(root); (root / "extract").mkdir(); source = self.extract(archive, root / "extract")
            (source / "bin" / "terraforma-local-core").write_text("tampered")
            prefix = root / "apps" / "terraforma-local-core"
            result = subprocess.run(["sh", source / "install.sh", prefix], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0); self.assertFalse(prefix.exists())

    def test_uninstall_refuses_unexpected_file(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); archive, _ = self.make_package(root); (root / "extract").mkdir(); source = self.extract(archive, root / "extract")
            prefix = root / "apps" / "terraforma-local-core"
            self.assertEqual(subprocess.run(["sh", source / "install.sh", prefix]).returncode, 0)
            (prefix / "unexpected.txt").write_text("keep")
            result = subprocess.run(["sh", prefix / "share" / "uninstall.sh", prefix], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0); self.assertTrue((prefix / "unexpected.txt").exists())

    def test_prefix_must_be_dedicated_package_directory(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); archive, _ = self.make_package(root); (root / "extract").mkdir(); source = self.extract(archive, root / "extract")
            result = subprocess.run(["sh", source / "install.sh", root / "apps" / "wrong"], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)

    def test_same_binary_produces_same_archive_digest(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); binary = root / "candidate"; binary.write_text("candidate"); binary.chmod(0o700)
            first = root / "first"; second = root / "second"; first.mkdir(); second.mkdir()
            a = package_macos.build(binary, first, "0.2.0")["archive_sha256"]
            b = package_macos.build(binary, second, "0.2.0")["archive_sha256"]
            self.assertEqual(a, b)

    def test_target_is_bound_into_archive_and_manifest(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); binary = root / "candidate"; binary.write_text("candidate"); binary.chmod(0o700)
            output = root / "delivery"; output.mkdir()
            result = package_macos.build(binary, output, "0.2.0", target="linux-x86-64")
            self.assertIn("linux-x86-64", result["archive"])
            with tarfile.open(result["archive"]) as tar:
                manifest = json.load(tar.extractfile("terraforma-local-core-0.2.0-linux-x86-64/manifest.json"))
            self.assertEqual(manifest["target"], "linux-x86-64")

    def test_wrong_platform_is_refused_before_archive_creation(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); binary = root / "not-linux"; binary.write_text("not an ELF")
            output = root / "delivery"; output.mkdir()
            result = subprocess.run([sys.executable, Path(package_linux.__file__), "--binary", binary, "--output-parent", output], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0); self.assertEqual(list(output.iterdir()), [])

    def test_verified_update_and_rollback_swap_only_package_prefix(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            old_binary = root / "old"; old_binary.write_text("#!/bin/sh\nprintf '%s\\n' '{\"status\":\"old\"}'\n"); old_binary.chmod(0o700)
            new_binary = root / "new"; new_binary.write_text("#!/bin/sh\nprintf '%s\\n' '{\"status\":\"new\"}'\n"); new_binary.chmod(0o700)
            deliveries = root / "deliveries"; deliveries.mkdir()
            old = Path(package_macos.build(old_binary, deliveries, "0.2.0")["archive"])
            new = Path(package_macos.build(new_binary, deliveries, "0.2.1")["archive"])
            (root / "old-extract").mkdir(); old_source = self.extract(old, root / "old-extract")
            (root / "new-extract").mkdir(); new_source = self.extract(new, root / "new-extract")
            prefix = root / "apps" / "terraforma-local-core"
            backup = root / "apps" / "terraforma-local-core-backup-0.2.0"
            self.assertEqual(subprocess.run(["sh", old_source / "install.sh", prefix]).returncode, 0)
            estate = root / "explicit-estate"; estate.mkdir(); (estate / "journal.jsonl").write_text("kept")
            updated = subprocess.run(["sh", new_source / "update.sh", prefix, backup], text=True, capture_output=True)
            self.assertEqual(updated.returncode, 0, updated.stderr)
            self.assertEqual(json.loads(subprocess.check_output([prefix / "bin" / "terraforma-local-core"], text=True))["status"], "new")
            self.assertEqual(json.loads(subprocess.check_output([backup / "bin" / "terraforma-local-core"], text=True))["status"], "old")
            rolled = subprocess.run(["sh", prefix / "share" / "rollback.sh", prefix, backup], text=True, capture_output=True)
            self.assertEqual(rolled.returncode, 0, rolled.stderr)
            self.assertEqual(json.loads(subprocess.check_output([prefix / "bin" / "terraforma-local-core"], text=True))["status"], "old")
            self.assertEqual(json.loads(subprocess.check_output([backup / "bin" / "terraforma-local-core"], text=True))["status"], "new")
            self.assertEqual((estate / "journal.jsonl").read_text(), "kept")

    def test_tampered_installed_prefix_refuses_update_without_move(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw); archive, _ = self.make_package(root)
            (root / "extract").mkdir(); source = self.extract(archive, root / "extract")
            prefix = root / "apps" / "terraforma-local-core"; backup = root / "apps" / "terraforma-local-core-backup-0.2.0"
            self.assertEqual(subprocess.run(["sh", source / "install.sh", prefix]).returncode, 0)
            (prefix / "bin" / "terraforma-local-core").write_text("tampered")
            result = subprocess.run(["sh", source / "update.sh", prefix, backup], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(prefix.exists()); self.assertFalse(backup.exists())

    def test_current_updater_migrates_legacy_three_payload_prefix(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            old_binary = root / "old"; old_binary.write_text("#!/bin/sh\nprintf '%s\\n' '{\"status\":\"old\"}'\n"); old_binary.chmod(0o700)
            new_binary = root / "new"; new_binary.write_text("#!/bin/sh\nprintf '%s\\n' '{\"status\":\"new\"}'\n"); new_binary.chmod(0o700)
            deliveries = root / "deliveries"; deliveries.mkdir()
            old = Path(package_macos.build(old_binary, deliveries, "0.2.0")["archive"])
            new = Path(package_macos.build(new_binary, deliveries, "0.2.1")["archive"])
            (root / "old-extract").mkdir(); old_source = self.extract(old, root / "old-extract")
            (root / "new-extract").mkdir(); new_source = self.extract(new, root / "new-extract")
            prefix = root / "apps" / "terraforma-local-core"; prefix.mkdir(parents=True)
            (prefix / "bin").mkdir(); (prefix / "share").mkdir()
            (prefix / "bin" / "terraforma-local-core").write_bytes((old_source / "bin" / "terraforma-local-core").read_bytes())
            (prefix / "bin" / "terraforma-local-core").chmod(0o700)
            for name in ("manifest.json", "uninstall.sh"):
                (prefix / "share" / name).write_bytes((old_source / name).read_bytes())
            (prefix / "share" / "uninstall.sh").chmod(0o700)
            legacy_sums = "\n".join((old_source / "SHA256SUMS").read_text().splitlines()[:3]) + "\n"
            (prefix / "share" / "SHA256SUMS").write_text(legacy_sums)
            backup = root / "apps" / "terraforma-local-core-backup-0.2.0"
            updated = subprocess.run(["sh", new_source / "update.sh", prefix, backup], text=True, capture_output=True)
            self.assertEqual(updated.returncode, 0, updated.stderr)
            self.assertEqual(json.loads(subprocess.check_output([prefix / "bin" / "terraforma-local-core"], text=True))["status"], "new")
            self.assertEqual(json.loads(subprocess.check_output([backup / "bin" / "terraforma-local-core"], text=True))["status"], "old")
            rolled = subprocess.run(["sh", prefix / "share" / "rollback.sh", prefix, backup], text=True, capture_output=True)
            self.assertEqual(rolled.returncode, 0, rolled.stderr)
            self.assertEqual(json.loads(subprocess.check_output([prefix / "bin" / "terraforma-local-core"], text=True))["status"], "old")


class DefaultPrefixTests(unittest.TestCase):
    make_package = PackageTests.make_package
    extract = PackageTests.extract

    def test_install_without_prefix_uses_home_and_prints_connect_commands(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw).resolve(); home = root / "home"; home.mkdir()
            archive, _ = self.make_package(root)
            (root / "extract").mkdir(); source = self.extract(archive, root / "extract")
            env = {"HOME": str(home), "PATH": "/usr/bin:/bin"}
            installed = subprocess.run(["sh", source / "install.sh"], text=True, capture_output=True, env=env)
            self.assertEqual(installed.returncode, 0, installed.stderr)
            prefix = home / ".terraforma" / "terraforma-local-core"
            binary = prefix / "bin" / "terraforma-local-core"
            self.assertTrue(binary.is_file())
            self.assertIn(f'claude mcp add --scope local terraforma-memory -- "{binary}" mcp', installed.stdout)
            self.assertIn(f'command = "{binary}"', installed.stdout)
            self.assertIn(str(home / ".terraforma" / "memory" / "my-project"), installed.stdout)
            again = subprocess.run(["sh", source / "install.sh"], text=True, capture_output=True, env=env)
            self.assertNotEqual(again.returncode, 0)
            removed = subprocess.run(["sh", prefix / "share" / "uninstall.sh", prefix], text=True, capture_output=True)
            self.assertEqual(removed.returncode, 0, removed.stderr); self.assertFalse(prefix.exists())

    def test_version_defaults_to_crate_version(self):
        self.assertEqual(package_macos.cargo_version(), "0.3.0")


if __name__ == "__main__":
    unittest.main()

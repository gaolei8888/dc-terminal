import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).parent

def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / (name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result

class BackupTests(unittest.TestCase):
    def test_legacy_allowlist_does_not_capture_other_volumes(self):
        backup = module('export')
        self.assertEqual(backup.LEGACY, {'dcw_dct-state', 'dcw_claude-state', 'dcw_work'})
        self.assertFalse(backup.managed_volume('dcw-student-' + 'a' * 32 + '-work', {}))
        self.assertTrue(backup.managed_volume('dcw-student-' + 'a' * 32 + '-work', {'dcw.classroom':'1'}))
        self.assertFalse(backup.managed_volume('database-data', {'dcw.classroom':'1'}))

    def test_receiver_rejects_path_traversal(self):
        receive = module('receive')
        for name in ['../secret', 'backup.tar', 'dct-backup-evil.cms', '/tmp/owned']:
            self.assertFalse(receive.valid_name(name))

    def test_inventory_excludes_sockets_and_detects_changes(self):
        import socket
        backup = module('export')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'file').write_text('first')
            sock = socket.socket(socket.AF_UNIX)
            sock.bind(str(root / 'daemon.sock'))
            try:
                before = backup.inventory([root])
                self.assertNotIn(str(root / 'daemon.sock'), before)
                (root / 'file').write_text('changed')
                self.assertNotEqual(before, backup.inventory([root]))
            finally:
                sock.close()

    def test_receiver_digest_failure_and_retention_are_bounded(self):
        import io, hashlib
        receive = module('receive')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            body = b'encrypted fixture'
            digest = hashlib.sha256(body).hexdigest()
            first = 'dct-backup-20260911T010000Z-' + 'a' * 16 + '.cms'
            second = 'dct-backup-20260911T020000Z-' + 'b' * 16 + '.cms'
            (root / 'unrelated.cms').write_text('keep')
            with self.assertRaises(ValueError):
                receive.receive(root, first, '0' * 64, io.BytesIO(body))
            self.assertFalse((root / first).exists())
            self.assertFalse(list(root.glob('.dct-incoming-*')))
            receive.receive(root, first, digest, io.BytesIO(body), keep=1)
            self.assertEqual((root / first).stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                receive.receive(root, first, digest, io.BytesIO(body))
            receive.receive(root, second, digest, io.BytesIO(body), keep=1)
            self.assertFalse((root / first).exists())
            self.assertTrue((root / second).exists())
            self.assertTrue((root / 'unrelated.cms').exists())

    def test_tar_failure_never_publishes_ciphertext(self):
        from unittest.mock import patch
        import subprocess
        backup = module('export')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'archive.cms'
            with patch.object(backup, 'run', side_effect=subprocess.CalledProcessError(1, ['tar'])):
                with self.assertRaises(subprocess.CalledProcessError):
                    backup.archive([root], output, {}, root / 'cert')
            self.assertFalse(output.exists())

    def test_changed_files_never_publish_ciphertext(self):
        from unittest.mock import patch
        backup = module('export')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'archive.cms'
            with patch.object(backup, 'inventory', side_effect=[{'/file': (1,)}, {'/file': (2,)}]), patch.object(backup, 'run') as run:
                with self.assertRaises(RuntimeError):
                    backup.archive([root], output, {}, root / 'cert')
                self.assertEqual(run.call_count, 1)
            self.assertFalse(output.exists())

    def test_authenticated_encryption_roundtrip_and_tamper_rejection(self):
        import subprocess
        restore = module('restore')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def command(args):
                subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            command(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-subj',
                     '/CN=Synthetic Test/', '-days', '1', '-keyout', str(root / 'key'), '-out', str(root / 'cert')])
            (root / 'plain').write_bytes(b'synthetic student work; no credentials')
            command(['openssl', 'cms', '-encrypt', '-binary', '-aes-256-gcm', '-outform', 'DER',
                     '-in', str(root / 'plain'), '-out', str(root / 'encrypted'), str(root / 'cert')])
            restore.decrypt(root / 'encrypted', root / 'cert', root / 'key', root / 'result')
            self.assertEqual((root / 'result').read_bytes(), (root / 'plain').read_bytes())
            content = bytearray((root / 'encrypted').read_bytes())
            content[-1] ^= 1
            (root / 'tampered').write_bytes(content)
            with self.assertRaises(subprocess.CalledProcessError):
                restore.decrypt(root / 'tampered', root / 'cert', root / 'key', root / 'rejected')
            self.assertFalse((root / 'rejected').exists())

    def test_changed_volume_topology_never_publishes_ciphertext(self):
        from unittest.mock import patch
        backup = module('export')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'archive.cms'
            with patch.object(backup, 'inventory', return_value={'/file': (1,)}), patch.object(backup, 'run'):
                with self.assertRaises(RuntimeError):
                    backup.archive([root], output, {}, root / 'cert', topology_unchanged=lambda: False)
            self.assertFalse(output.exists())

if __name__ == '__main__':
    unittest.main()

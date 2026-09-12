import importlib.machinery
import importlib.util
from pathlib import Path
import tempfile
import unittest
import sys
sys.dont_write_bytecode = True

CLI = Path(__file__).resolve().parents[3] / 'bin' / 'dct-publish'
loader = importlib.machinery.SourceFileLoader('publisher', str(CLI))
spec = importlib.util.spec_from_loader(loader.name, loader)
publisher = importlib.util.module_from_spec(spec)
loader.exec_module(publisher)

class PublishTests(unittest.TestCase):
    def test_collect_static_and_skip_source_config(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'index.html').write_text('<html>作品</html>')
            (root / 'package.json').write_text('{}')
            (root / 'app.js.map').write_text('{}')
            files, size = publisher.collect(root)
            self.assertEqual([f['path'] for f in files], ['index.html'])
            self.assertGreater(size, 0)

    def test_reject_credentials_even_hidden(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'index.html').write_text('okay')
            (root / '.env').write_text('SECRET=private')
            with self.assertRaises(publisher.PublishError):
                publisher.collect(root)

    def test_reject_symlink_hardlink_and_socket(self):
        import os, socket
        for kind in ['symlink', 'hardlink', 'socket']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'index.html').write_text('okay')
                sock = None
                if kind == 'symlink': (root / 'other.html').symlink_to(root / 'index.html')
                elif kind == 'hardlink': os.link(root / 'index.html', root / 'other.html')
                else:
                    sock = socket.socket(socket.AF_UNIX)
                    sock.bind(str(root / 'daemon.sock'))
                try:
                    with self.assertRaises(publisher.PublishError): publisher.collect(root)
                finally:
                    if sock: sock.close()

    def test_config_origin_is_fixed_and_token_not_echoed(self):
        with tempfile.TemporaryDirectory() as directory:
            import json
            path = Path(directory) / 'publish.json'
            path.write_text(json.dumps({'url': 'https://evil.example', 'token': 'a' * 64}))
            path.chmod(0o600)
            with self.assertRaises(publisher.PublishError) as failure: publisher.load_config(path)
            self.assertNotIn('a' * 64, str(failure.exception))

    def test_byte_and_file_limits(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'index.html').write_text('12345')
            with patch.object(publisher, 'MAX_BYTES', 4):
                with self.assertRaises(publisher.PublishError): publisher.collect(root)
            with patch.object(publisher, 'MAX_FILES', 0):
                with self.assertRaises(publisher.PublishError): publisher.collect(root)

    def test_directory_boundary_and_stable_namespaced_key(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / 'dist').mkdir()
            with patch.object(publisher, 'ROOTS', [(root, 'legacy')]):
                path, key = publisher.safe_directory(str(root / 'dist'))
                self.assertEqual(key, publisher.safe_directory(str(root / 'dist' / '.'))[1])
                with self.assertRaises(publisher.PublishError): publisher.safe_directory(str(root.parent))
            with patch.object(publisher, 'ROOTS', [(root, 'library')]):
                self.assertNotEqual(key, publisher.safe_directory(str(root / 'dist'))[1])

    def test_prepared_response_requires_real_safe_links(self):
        valid = {'version': 'v1', 'status': 'prepared', 'previewUrl': 'https://works.dataclue.cn/preview/' + ('b' * 64) + '/index.html',
                 'confirmUrl': 'https://dataclue.cn/w/shared/?publish=' + ('a' * 32)}
        self.assertEqual(publisher.validate_result(valid, True), valid)
        for url in ['http://localhost:5173', 'https://evil.example/a', 'https://user@dataclue.cn/a', 'https://dataclue.cn/preview/' + ('b' * 64) + '/index.html']:
            with self.assertRaises(publisher.PublishError): publisher.validate_result(dict(valid, previewUrl=url), True)
        with self.assertRaises(publisher.PublishError): publisher.validate_result({'status': 'prepared'}, True)

    def test_network_failure_does_not_retry_or_expose_token(self):
        from unittest.mock import patch, Mock
        connection = Mock()
        connection.request.side_effect = TimeoutError('secret diagnostic')
        with patch.object(publisher.http.client, 'HTTPSConnection', return_value=connection):
            with self.assertRaises(publisher.PublishError) as failure:
                publisher.request({'token': 'a' * 64}, 'POST', '/_agent/publish/prepare', {})
        self.assertEqual(connection.request.call_count, 1)
        self.assertNotIn('secret diagnostic', str(failure.exception))
        self.assertNotIn('a' * 64, str(failure.exception))

    def test_http_success_uses_exact_contract_and_rejects_redirect(self):
        from unittest.mock import patch, Mock
        import json
        response = Mock(status=200)
        response.read.return_value = json.dumps({'version': 'v1', 'status': 'prepared',
             'previewUrl': 'https://works.dataclue.cn/preview/' + ('b' * 64) + '/index.html', 'confirmUrl': 'https://dataclue.cn/w/shared/?publish=' + ('a' * 32)}).encode()
        connection = Mock()
        connection.getresponse.return_value = response
        with patch.object(publisher.http.client, 'HTTPSConnection', return_value=connection):
            result = publisher.request({'token': 'a' * 64}, 'POST', '/_agent/publish/prepare', {'projectKey':'key'})
            self.assertEqual(result['status'], 'prepared')
            self.assertEqual(connection.request.call_args.args[:2], ('POST', '/_agent/publish/prepare'))
            self.assertEqual(connection.request.call_args.kwargs['headers']['Authorization'], 'Bearer ' + 'a' * 64)
            response.status = 302
            with self.assertRaises(publisher.PublishError): publisher.request({'token':'a' * 64}, 'POST', '/_agent/publish/prepare', {})

    def test_collect_uses_verified_fd_even_if_path_changes(self):
        import os
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'index.html').write_text('verified directory')
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
            try:
                files, _ = publisher.collect(None, root_fd=fd)
                self.assertEqual(files[0]['path'], 'index.html')
                os.fstat(fd)  # collector must not close its caller's descriptor
            finally: os.close(fd)

    def test_credentials_in_json_content_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'index.html').write_text('okay')
            (root / 'settings.json').write_text('{"apiKey":"not-a-real-test-secret"}')
            with self.assertRaises(publisher.PublishError): publisher.collect(root)

    def test_server_path_contract_and_config_exclusion(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'index.html').write_text('okay')
            (root / 'config.json').write_text('{}')
            files, _ = publisher.collect(root)
            self.assertEqual([f['path'] for f in files], ['index.html'])
            (root / 'bad%name.js').write_text('okay')
            with self.assertRaises(publisher.PublishError): publisher.collect(root)

if __name__ == '__main__': unittest.main()

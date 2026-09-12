#!/usr/bin/env python3
"""Operator-side relay: existing SSH trust to each host; no plaintext downloads."""
import hashlib
import os
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

SOURCE = 'root@dataclue.cn'
RECEIVER = 'dev@ai.tzspace.cn'
SSH = ['ssh', '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes', '-o', 'ConnectTimeout=15']

def remote(host, args, **kwargs):
    return subprocess.run(SSH + [host, shlex.join(args)], check=True, stderr=subprocess.PIPE, **kwargs)

def main():
    os.umask(0o077)
    name = 'dct-backup-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + secrets.token_hex(8) + '.cms'
    source_path = '/var/tmp/' + name
    # Source export and upload are deliberately separate. Source private temp
    # output is retained on transfer failure for explicit operator recovery.
    remote(SOURCE, ['python3', '/opt/dct-backup/export.py', '--certificate',
                    '/etc/dct-backup/recipient.crt', '--output', source_path], stdout=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory(prefix='dct-backup-relay-') as directory:
        archive = Path(directory) / name
        with archive.open('xb') as output:
            remote(SOURCE, ['cat', '--', source_path], stdout=output)
        digest = hashlib.sha256()
        with archive.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        with archive.open('rb') as stream:
            remote(RECEIVER, ['python3', '/home/dev/dct-backup-tools/receive.py', name,
                              digest.hexdigest()], stdin=stream, stdout=subprocess.DEVNULL)
    # Only this exact newly generated ciphertext is removed after receiver ACK.
    remote(SOURCE, ['rm', '--', source_path], stdout=subprocess.DEVNULL)
    print('Offsite encrypted backup completed: ' + name)

if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('Offsite backup FAILED. Source ciphertext may remain in /var/tmp; inspect privately and retry.', file=sys.stderr)
        sys.exit(1)

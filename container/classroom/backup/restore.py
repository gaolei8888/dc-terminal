#!/usr/bin/env python3
"""Authenticate/decrypt only. Never extracts or changes a production volume."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

def decrypt(archive, certificate, key, output):
    with tempfile.TemporaryDirectory(prefix='dct-restore-') as temporary:
        plain = Path(temporary) / 'archive.tar'
        # CMS GCM checks the authentication tag. Plaintext is never published
        # when openssl returns nonzero, even if it wrote unauthenticated bytes.
        subprocess.run(['openssl', 'cms', '-decrypt', '-binary', '-inform', 'DER',
                        '-in', str(archive), '-recip', str(certificate), '-inkey', str(key),
                        '-out', str(plain)], check=True, stdout=subprocess.DEVNULL)
        with Path(output).open('xb') as destination, plain.open('rb') as source:
            os.chmod(output, 0o600)
            shutil.copyfileobj(source, destination)
            destination.flush()
            os.fsync(destination.fileno())

def main():
    parser = argparse.ArgumentParser()
    for option in ['archive', 'certificate', 'key', 'output']:
        parser.add_argument('--' + option, required=True, type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    decrypt(args.archive, args.certificate, args.key, args.output)
    print('Archive authenticated and decrypted. Restore into a separate empty staging directory.')

if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('Restore authentication/decryption failed; no restore performed.', file=sys.stderr)
        sys.exit(1)

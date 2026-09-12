#!/usr/bin/env python3
"""Receive ciphertext on stdin, verify digest, atomically publish, retain owned files."""
import argparse
from datetime import datetime
import hashlib
import os
from pathlib import Path
import re
import stat
import sys
import tempfile

PATTERN = re.compile(r'dct-backup-[0-9]{8}T[0-9]{6}Z-[a-f0-9]{16}\.cms\Z')

def valid_name(name):
    if not PATTERN.fullmatch(name):
        return False
    try:
        datetime.strptime(name[11:27], '%Y%m%dT%H%M%SZ')
        return True
    except ValueError:
        return False

def receive(directory, name, digest, stream, keep=14, limit=100 * 1024**3):
    if not valid_name(name) or not re.fullmatch('[a-f0-9]{64}', digest) or not 1 <= keep <= 90:
        raise ValueError('Invalid request')
    directory = Path(directory)
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError('Backup directory must be private and owned by current user')
    fd, temporary = tempfile.mkstemp(prefix='.dct-incoming-', dir=directory)
    try:
        checksum, size = hashlib.sha256(), 0
        with os.fdopen(fd, 'wb') as target:
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                size += len(chunk)
                if size > limit:
                    raise ValueError('Archive too large')
                checksum.update(chunk)
                target.write(chunk)
            target.flush()
            os.fsync(target.fileno())
        if not size or checksum.hexdigest() != digest:
            raise ValueError('Archive digest mismatch')
        os.link(temporary, directory / name)  # atomic and refuses overwrite
    finally:
        os.unlink(temporary)
    descriptor = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    owned = []
    for entry in directory.iterdir():
        info = entry.lstat()
        if valid_name(entry.name) and stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.getuid():
            owned.append(entry)
    # Bounded count; never touch non-owned names, symlinks, directories, or staging.
    for entry in sorted(owned, reverse=True)[keep:]:
        entry.unlink()

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('name')
    parser.add_argument('sha256')
    parser.add_argument('--directory', default=str(Path.home() / 'dct-backups'))
    parser.add_argument('--keep', type=int, default=14)
    args = parser.parse_args()
    os.umask(0o077)
    receive(args.directory, args.name, args.sha256, sys.stdin.buffer, args.keep)
    print('Encrypted backup received and verified.')

if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('Backup receive failed; check private receiver storage.', file=sys.stderr)
        sys.exit(1)

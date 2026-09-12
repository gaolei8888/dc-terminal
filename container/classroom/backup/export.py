#!/usr/bin/env python3
"""Root-only live file archive. Never claims application-level consistency."""
import argparse
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile

LEGACY = {'dcw_dct-state', 'dcw_claude-state', 'dcw_work'}
MANAGED = re.compile(r'dcw-student-[a-f0-9]{32}-(state|claude|codex|work)\Z')
DESTINATIONS = {'state': '/home/dc/.dct', 'claude': '/home/dc/.claude',
                'codex': '/home/dc/.codex', 'work': '/home/dc/work'}

def run(args):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout

def managed_volume(name, labels):
    return bool(MANAGED.fullmatch(name) and labels.get('dcw.classroom') == '1')

def inventory(roots):
    result = {}
    def visit(path):
        info = path.lstat()
        if stat.S_ISSOCK(info.st_mode):
            return
        if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode)):
            raise RuntimeError('Unsupported special file')
        result[str(path)] = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid,
                             info.st_size, info.st_mtime_ns, info.st_ctime_ns)
        if stat.S_ISDIR(info.st_mode):
            for child in sorted(path.iterdir()):
                visit(child)
    for root in roots:
        if root.is_symlink() or not root.is_dir():
            raise RuntimeError('Invalid archive root')
        visit(root)
    return result

def discover():
    names = run(['docker', 'ps', '-aq', '--filter', 'label=dcw.classroom=1']).decode().split()
    wanted = set(LEGACY)
    for name in names:
        info = json.loads(run(['docker', 'inspect', name]))[0]
        labels = info['Config'].get('Labels') or {}
        student = labels.get('dcw.student', '')
        if labels.get('dcw.classroom') != '1' or not re.fullmatch('[a-f0-9]{32}', student):
            raise RuntimeError('Invalid managed container labels')
        expected = 'dcw-student-' + student
        if info['Name'] != '/' + expected:
            raise RuntimeError('Invalid managed container name')
        mounts = {item['Destination']: item for item in info['Mounts']}
        for suffix, destination in DESTINATIONS.items():
            mount = mounts.get(destination, {})
            volume = expected + '-' + suffix
            if mount.get('Type') != 'volume' or mount.get('Name') != volume:
                raise RuntimeError('Invalid managed volume mapping')
            wanted.add(volume)
    # Include labeled managed volumes whose containers have been removed, but
    # only the exact classroom-owned naming scheme. Never archive arbitrary labels.
    for name in run(['docker', 'volume', 'ls', '-q', '--filter', 'label=dcw.classroom=1']).decode().split():
        if MANAGED.fullmatch(name):
            wanted.add(name)
    volumes = {}
    for name in sorted(wanted):
        info = json.loads(run(['docker', 'volume', 'inspect', name]))[0]
        if name not in LEGACY and not managed_volume(name, info.get('Labels') or {}):
            raise RuntimeError('Invalid managed volume label')
        if info.get('Driver') != 'local' or info.get('Options'):
            raise RuntimeError('Only ordinary local Docker volumes supported')
        root = Path(info['Mountpoint'])
        if root != Path('/var/lib/docker/volumes') / name / '_data':
            raise RuntimeError('Unexpected volume mountpoint')
        volumes[name] = str(root)
    return volumes

def archive(roots, output, manifest, certificate, topology_unchanged=None):
    before = inventory(roots)
    with tempfile.TemporaryDirectory(prefix='dct-backup-') as temporary:
        directory = Path(temporary)
        os.chmod(directory, 0o700)
        listing = directory / 'files'
        listing.write_bytes(b''.join(os.fsencode(name.lstrip('/')) + b'\0' for name in before))
        metadata = directory / 'manifest.json'
        metadata.write_text(json.dumps(manifest, indent=2))
        plain = directory / 'data.tar'
        # GNU tar nonzero (including file-changed exit 1) is fatal. Do not use
        # --ignore-failed-read. Explicit no-recursion list skips Unix sockets.
        run(['tar', '--create', '--file', str(plain), '--numeric-owner', '--acls', '--xattrs',
             '--directory', '/', '--no-recursion', '--null', '--verbatim-files-from',
             '--files-from', str(listing), '--directory', str(directory), 'manifest.json'])
        if inventory(roots) != before:
            raise RuntimeError('Files changed during archive; retry during quiet period')
        if topology_unchanged is not None and not topology_unchanged():
            raise RuntimeError('Workspace volume topology changed during archive')
        encrypted = directory / 'data.cms'
        run(['openssl', 'cms', '-encrypt', '-binary', '-aes-256-gcm', '-outform', 'DER',
             '-in', str(plain), '-out', str(encrypted), str(certificate)])
        # Exclusive creation prevents accidentally overwriting an earlier backup.
        with output.open('xb') as target, encrypted.open('rb') as source:
            os.chmod(output, 0o600)
            import shutil
            shutil.copyfileobj(source, target)
            target.flush()
            os.fsync(target.fileno())

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--certificate', required=True, type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    if os.geteuid() != 0:
        raise RuntimeError('Source export requires root')
    volumes = discover()
    state = Path('/var/lib/dcw-classroom')
    if not (state / 'classroom.json').is_file():
        raise RuntimeError('Manager state unavailable')
    manifest = {'format': 1, 'consistency': 'live-files-checked-not-application-snapshot',
                'manager': str(state), 'volumes': volumes}
    archive([state] + [Path(value) for value in volumes.values()], args.output, manifest,
            args.certificate, topology_unchanged=lambda: discover() == volumes)
    print('Encrypted archive ready; live file copy, not an application snapshot.')

if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('Backup export failed; no successful snapshot reported.', file=sys.stderr)
        sys.exit(1)

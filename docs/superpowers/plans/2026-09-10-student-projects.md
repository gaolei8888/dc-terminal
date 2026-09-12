# Student projects implementation

Approved scope: real project library, file listing/upload/download, ZIP export,
server-confirmed archive status, continue project, save and end practice.
Named versions, ZIP import and deletion/recycle bin are deferred.

## Storage and lifecycle

One gate credential identifies one workspace; shared links share projects.
Register existing cwd as `current` without moving files. New project roots live
under the gate socket's state directory `student-projects/work/<id>`; metadata
and latest ZIP archives are siblings, outside the legacy work directory.
Both existing Docker volumes must survive container replacement.
Ending practice saves first, stops only matching dct sessions, then saves the
settled files again. No project directory or dependency is deleted. Failure
preserves files and reports the failed step. Detached programs are outside the
dct session lifecycle and are not claimed to be stopped.

Archives exclude credentials, environment files, symlinks, dependency/cache and
Git internals. Bound file counts and bytes; detect concurrent file changes;
publish only complete verified archives atomically. Keep previous archive on
failure. No archive extraction endpoint in this phase.

## HTTP contract (gate authentication applies to all routes)

Base `/_dct/projects`. Errors: JSON `{error: string}`, non-2xx.
- GET base: `{projects:[{id,name,dir,saved_at: unix seconds|null}], max_archive_bytes: number}`.
- POST base, JSON `{name}`: project object. Max 80 chars; random ID.
- GET `/<id>/files`: `{files:[{path,size}], excluded: string[], truncated: false}`; relative file paths, recursively listed exportable files.
- GET `/<id>/file?path=<encoded relative path>`: attachment bytes.
- POST `/<id>/save`: `{saved_at, bytes, files}`; empty JSON body allowed.
- GET `/<id>/download`: prepares fresh verified ZIP and serves attachment.
- POST `/<id>/upload?name=<encoded filename>`: raw body, returns `{name}`; uploads into project's uploads/ without overwriting existing names.
- POST `/<id>/continue`, JSON `{profile:"claude"|"codex"|"shell"}`: reuses running matching session or creates one, returns `{id,dir}`. Browser shows “已打开，请在终端看板选择该会话”; no terminal output injection.
- POST `/<id>/end`: `{saved_at,bytes,files,stopped}`; archive then stop sessions then final archive. Failure never removes project files.

Browser remembers selected project per tab, never displays invented student name
or current save status from a timer. Archive timestamp means only that archive.
Live file writes remain in persistent project directory. Desktop collapsible
panel, mobile drawer, keyboard focus restored when closed, existing xterm theme
sync retained. Never show controls that only simulate unsupported operations.

## Verification

1. Backend tests: survival across reopen, create validation, traversal/symlink
   exclusion, ZIP contents, quotas, atomic failure preserving previous snapshot.
2. Gate tests: unauthenticated access denied, JSON/body limits, correct downloads.
3. Frontend browser integration: real API create/upload/list/download/end/resume,
   error states, dark/light themes, 390px mobile and terminal resize/focus.
4. Build container and stage on isolated volumes, validate before deployment.
5. Keep live state and credentials intact; record migration/rollback and report
   exact tested/deployed scope.

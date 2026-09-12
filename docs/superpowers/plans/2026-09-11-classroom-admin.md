# Classroom administration implementation plan

**Goal:** Independently owned student workspaces, a simple administrator list with
read-only observation and explicit assistance, and a minimal student UI.
**Architecture:** A host-side Node service provisions labeled Docker containers,
stores its own access credentials outside student volumes, and proxies authenticated
student/admin routes to existing gates. Existing shared workspace stays available.
**Stack:** Node built-ins (HTTP, net, crypto, child_process, atomic JSON), existing
Rust daemon/gate and browser panel, Docker/Caddy. No Docker socket in student containers.

## Scope and invariants

- Admin login at `/admin/`; students use `/w/<id>/#t=<secret>`.
- Student tokens and cookies authorize exactly one workspace. Never deliver backend
  gate credentials to browsers. HttpOnly, Secure, SameSite cookies; write requests
  checked for same origin. Reset/disable revokes live connections too.
- Admin observes through a read-only screen API by default. Assistance explicitly
  enables the real student UI in an admin iframe and shows a student-side banner.
- One student's files, agent state and 3 GiB runtime allocation are separate.
  Reserve host memory and cap running workspaces; stopped workspaces retain volumes.
- Stop/end archives projects before stopping runtime; failure preserves runtime.
  No student deletion or volume deletion UI. Legacy shared workspace is labeled shared.
- Student UI: current project name + upload/download, optional project list, one end
  button. Persisted files need no manual save action. Explain errors in plain Chinese.
- Publishing to ai.tzspace.cn remains a future integration through a unified service;
  this task neither publishes artifacts nor creates per-agent deployment credentials.

## Files and independent work

1. `container/classroom/server.mjs`, `docker.mjs`, `store.mjs`: authenticated host
   management/proxy service, atomic persistent state, constrained Docker lifecycle.
2. `container/classroom/admin.html`: simple administrator login/list/observation/
   assistance page against the contract below.
3. `container/upload-panel.html`: simplify student UX, configurable route prefix,
   periodic presence banner; preserve file APIs, themes, resize and input focus.
4. `container/classroom/*.test.mjs`, browser/API integration tests and deployment docs.

## Frontend HTTP contract

Admin JSON calls use `/admin/api`. All errors `{error:string}` with non-2xx status.
- POST `/login` `{password}`, POST `/logout` `{}`.
- GET `/state`: `{capacity:{running,max,totalMemory},students:[{id,name,status,
  shared,memoryBytes,diskBytes,connections,assisting}],audit:[{time,action,name}]}`.
  status is `running`, `stopped`, `new`, or `disabled`; counters may be null if unknown.
- POST `/students` `{names:[string]}`: creates stopped records, returns `{students}`.
- POST `/students/<id>/start|stop|rotate|disable|enable|link` `{}`.
  link and rotate return `{url}`. start/stop return `{ok:true}`.
- GET `/students/<id>/observe?session=<numeric id>` returns `{sessions:[{id,profile,
  state}],screen:{lines:[[{text:string,...}]],state}|null}`. No write input accepted.
- POST `/students/<id>/assist` `{}`: lease starts/renews for 45 seconds, returns
  `{url:'/admin/workspaces/<id>/view/'}`; renewal every 15 seconds while assisting.
- POST `/students/<id>/end-assist` `{}`: closes admin input connections; files untouched.
- GET `/students/<id>/projects`: proxy library listing; GET
  `/students/<id>/projects/<project>/download`: admin archive download.

Student/assist iframe HTML receives `window.DCW_BASE_PATH` equal to route prefix
without trailing slash, plus optional `window.DCW_STUDENT_NAME`.
Panel uses `${DCW_BASE_PATH}/_dct/projects` (empty prefix retains legacy behavior).
GET `${DCW_BASE_PATH}/_dct/presence`: `{assisting:boolean}`. Legacy prefix may return
404 and should silently skip the presence notice. ttyd's own WS/token paths already
derive from browser pathname, so backend proxy strips the workspace prefix.

## Verification

- Auth tests: admin/student separation, cross-student reads/writes, unsafe origins,
  link revocation, disabled users, assistance lease and WS admission, no token leaks.
- Driver tests: scoped labels/names, 3 GiB limits, persistent separate volumes,
  capacity race serialization, save failure prevents stop, no global Docker actions.
- Browser: student simplicity + projects/download/upload retained; admin create,
  copy link, observe, assist/end, reset, stop; mobile layout and keyboard focus.
- Real isolated containers: two identities cannot access each other's project or
  administrator API; observe cannot send input; assistance presence works.
- Deploy manager and updated UI without recreating the active shared workspace;
  route the entire origin through the manager, which serves trusted host-owned HTML
  and treats student backends only as data/terminal streams. Adopt the original root
  access link and preserve existing volumes; rotations revoke legacy access too.
- Managed students finish through authenticated `/w/<id>/finish`: archive projects,
  stop the container, release capacity, and show a simple return-to-learning page.

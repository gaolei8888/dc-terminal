// 云端创作服务本机端到端：教室服务（真 DockerDriver）→ 容器里的门 → 创作服务。
// 用法：node container/tests/authoring-e2e.mjs
//
// 只在本机跑，不碰生产。容器用测试专用镜像 dc-workspace:authoring-e2e：
// FROM dc-workspace:authoring-dev，再加一个假 claude 和它的包装脚本，并把
// DCW_CLAUDE 指过去。假 claude 绝不进正式 Dockerfile。
//
// 可调环境变量：
//   E2E_BASE_IMAGE   基础镜像（默认 dc-workspace:authoring-dev）
//   FAKE_CLAUDE      fake-claude.mjs 的路径（默认 dc_desktop 的 cloud-authoring 工作树）
import assert from 'node:assert/strict';
import {execFileSync, spawnSync} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {Store} from '../classroom/store.mjs';
import {Classroom} from '../classroom/server.mjs';
import {DockerDriver} from '../classroom/docker.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.E2E_BASE_IMAGE || 'dc-workspace:authoring-dev';
const IMAGE = 'dc-workspace:authoring-e2e';
const FAKE = process.env.FAKE_CLAUDE || '/Users/lei/work/dc/dc_desktop-cloud-authoring/services/smart-app-runtime/src/agent-cli/testing/fake-claude.mjs';
const sh = (args, opts = {}) => execFileSync('docker', args, {encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'], ...opts});
const log = (...a) => console.log('[e2e]', ...a);

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-authoring-e2e-'));
let app = null, student = null;

async function cleanup() {
  try { await app?.close(); } catch {}
  if (student) {
    const name = `dcw-student-${student.id}`;
    spawnSync('docker', ['rm', '-f', name]);
    for (const s of ['state', 'claude', 'codex', 'work']) spawnSync('docker', ['volume', 'rm', '-f', `${name}-${s}`]);
  }
  fs.rmSync(tmp, {recursive: true, force: true});
}

function buildImage() {
  if (!fs.existsSync(FAKE)) throw new Error('找不到 fake-claude.mjs：' + FAKE);
  const ctx = path.join(tmp, 'image');
  fs.mkdirSync(ctx);
  fs.copyFileSync(FAKE, path.join(ctx, 'fake-claude.mjs'));
  // 假 claude 不会回答 `auth status`，包装脚本替它答。
  fs.writeFileSync(path.join(ctx, 'claude-wrap.sh'), '#!/bin/sh\nif [ "$1" = auth ]; then echo \'{"loggedIn":true}\'; exit 0; fi\nexec node /opt/e2e/fake-claude.mjs "$@"\n', {mode: 0o755});
  fs.writeFileSync(path.join(ctx, 'Dockerfile'), `FROM ${BASE}\nUSER root\nCOPY fake-claude.mjs claude-wrap.sh /opt/e2e/\nRUN chmod 755 /opt/e2e/claude-wrap.sh && chmod 644 /opt/e2e/fake-claude.mjs\nENV DCW_CLAUDE=/opt/e2e/claude-wrap.sh FAKE_SCRIPT=hello\nUSER dc\n`);
  sh(['build', '-q', '-t', IMAGE, ctx]);
}

// 读 SSE 直到 stop(events) 为真；返回已收到的事件（含 id）。
async function readEvents(url, headers, stop, ms = 30000) {
  const ctl = new AbortController(), timer = setTimeout(() => ctl.abort(), ms);
  const res = await fetch(url, {headers: {...headers, accept: 'text/event-stream'}, signal: ctl.signal});
  assert.equal(res.status, 200, 'events status');
  assert.match(res.headers.get('content-type'), /^text\/event-stream/);
  const events = [], reader = res.body.getReader(), dec = new TextDecoder();
  let buf = '';
  try {
    for (;;) {
      const {value, done} = await reader.read();
      if (done) break;
      buf += dec.decode(value, {stream: true});
      let i;
      while ((i = buf.indexOf('\n\n')) >= 0) {
        const block = buf.slice(0, i); buf = buf.slice(i + 2);
        const data = block.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).trimStart()).join('\n');
        const id = block.split('\n').find(l => l.startsWith('id:'))?.slice(3).trim();
        if (!data) continue;
        let ev; try { ev = JSON.parse(data); } catch { continue; }
        events.push({id, ...ev});
        if (stop(events)) return events;
      }
    }
  } finally { clearTimeout(timer); ctl.abort(); }
  throw new Error('事件流结束但没等到目标事件：' + JSON.stringify(events));
}

async function main() {
  log('build test image', IMAGE, 'FROM', BASE);
  buildImage();

  const store = new Store(path.join(tmp, 'classroom'), {password: 'e2e-admin'});
  [student] = store.add(['E2E学生']);
  const driver = new DockerDriver({image: IMAGE, maxRunning: 2, memory: '1g'});
  app = new Classroom({store, driver, origin: 'http://localhost', secure: false});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  const base = `${origin}/w/${student.id}/_dct/authoring/v1`;

  const login = await fetch(`${origin}/w/${student.id}/login`, {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({token: student.token})});
  assert.equal(login.status, 200, 'login');
  const cookie = login.headers.get('set-cookie').split(';')[0];
  const H = {cookie};
  const J = {...H, 'Content-Type': 'application/json'};

  assert.equal((await fetch(base + '/health')).status, 401, '未登录被拒');

  log('start workspace via driver (real docker run)');
  const started = await fetch(`${origin}/w/${student.id}/start`, {method: 'POST', headers: J, body: '{}'});
  assert.equal(started.status, 200, 'start: ' + await started.clone().text());

  // 服务比门晚一点起来
  let health;
  for (let i = 0; i < 40; i++) {
    const r = await fetch(base + '/health', {headers: H});
    if (r.status === 200) { health = await r.json(); break; }
    await new Promise(r => setTimeout(r, 500));
  }
  assert.ok(health, 'health 200');
  assert.equal(health.ai.loggedIn, true);
  assert.equal(typeof health.version, 'string');
  log('health', JSON.stringify(health));

  const created = await fetch(base + '/projects', {method: 'POST', headers: J, body: JSON.stringify({requestId: 'e2e-req-1', name: '喝水记录', goal: '做一个喝水记录'})});
  assert.equal(created.status, 201, 'create: ' + await created.clone().text());
  const {projectId, turnId} = await created.json();
  assert.ok(projectId && turnId);
  log('project', projectId, 'turn', turnId);

  // 同一 requestId 重放不应再开一个项目
  const replay = await fetch(base + '/projects', {method: 'POST', headers: J, body: JSON.stringify({requestId: 'e2e-req-1', name: '喝水记录', goal: '做一个喝水记录'})});
  assert.ok([200, 201].includes(replay.status), 'replay status ' + replay.status);
  assert.equal((await replay.json()).projectId, projectId, 'requestId 幂等');

  const events = await readEvents(`${base}/events?project=${encodeURIComponent(projectId)}&lastEventId=0`, H, evs => evs.some(e => e.type === 'turn_done'));
  const types = events.map(e => e.type);
  log('events', types.join(' > '));
  assert.ok(types.includes('turn_started'), 'has turn_started');
  assert.ok(types.indexOf('turn_started') < types.indexOf('turn_done'), 'turn_started 在 turn_done 之前');
  assert.equal(types.filter(t => t === 'turn_done').length, 1);
  assert.ok(!types.includes('turn_failed'), 'no turn_failed');
  // 事件 id 是 <boot>-<seq>：同一次起服务 boot 相同，seq 递增
  const ids = events.map(e => String(e.id));
  const boots = new Set(ids.map(id => id.split('-')[0]));
  const seqs = ids.map(id => Number(id.split('-')[1]));
  assert.ok(ids.every(id => /^[0-9a-f]+-\d+$/.test(id)) && boots.size === 1, '事件 id 形如 <boot>-<seq>: ' + ids);
  assert.ok(seqs.every((v, i) => i === 0 || v > seqs[i - 1]), '事件 id 递增: ' + ids);

  // 断线续传：从第一条之后继续，不会重复第一条
  const resumed = await readEvents(`${base}/events?project=${encodeURIComponent(projectId)}`, {...H, 'last-event-id': ids[0]}, evs => evs.length >= events.length - 1);
  assert.equal(resumed[0].id, ids[1], 'Last-Event-ID 续传');

  const detail = await fetch(`${base}/projects/${encodeURIComponent(projectId)}`, {headers: H});
  assert.equal(detail.status, 200);
  const dj = await detail.json();
  assert.equal(dj.busy, false);
  assert.ok(Array.isArray(dj.turns) && dj.turns.length >= 1);
  assert.equal(typeof dj.turns[0].checkpointId, 'string', '用户那句带撤销点');

  // 假 claude 不写文件；服务建项目时给了脚手架，这里按「AI 做完了」的样子
  // 经 docker exec 把最小 app.yaml + 入口写进项目目录（覆盖脚手架）。
  const name = `dcw-student-${student.id}`;
  const projDir = `/home/dc/work/apps/${projectId}`;
  sh(['exec', name, 'test', '-d', projDir]);
  const appYaml = ['apiVersion: dcdesktop.io/v1alpha1', 'kind: SmartApp', 'metadata:', '  id: app-water', '  name: 喝水记录', '  version: 0.1.0', 'runtime:', '  entryAgent: main-agent', '  minimumVersion: 0.1.0', 'clients:', '  - kind: web', '    entry: src/index.ts', 'secretRefs: []', ''].join('\n');
  const entry = "document.title = '喝水记录';\ndocument.body.innerHTML = '<h1>喝水记录 e2e-marker</h1>';\n";
  sh(['exec', '-i', name, 'sh', '-c', `cat > ${projDir}/app.yaml`], {input: appYaml});
  sh(['exec', '-i', name, 'sh', '-c', `cat > ${projDir}/src/index.ts`], {input: entry});

  const bundle = await fetch(`${base}/projects/${encodeURIComponent(projectId)}/bundle`, {headers: H});
  assert.equal(bundle.status, 200, 'bundle: ' + await bundle.clone().text());
  const b = await bundle.json();
  assert.equal(b.appId, 'app-water'); assert.equal(b.name, '喝水记录'); assert.equal(b.version, '0.1.0');
  assert.equal(typeof b.html, 'string');
  assert.ok(b.html.includes('喝水记录 e2e-marker') || b.html.includes('\\u5588\\u6c34'), 'html 含标题');
  assert.ok(b.bytes > 0);
  log('bundle ok', b.appId, b.name, b.version, b.bytes + ' bytes');

  // 不存在的项目 → 404；管理员不放行见 server.test.mjs
  assert.equal((await fetch(`${base}/projects/p-nope/bundle`, {headers: H})).status, 404);
  console.log('E2E_PASS');
}

try {
  await main();
} catch (e) {
  console.error('[e2e] FAILED:', e);
  process.exitCode = 1;
} finally {
  await cleanup();
}

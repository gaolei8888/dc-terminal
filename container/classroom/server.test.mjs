import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import {Store} from './store.mjs';
import {Classroom} from './server.mjs';

test('separate roles and students, safe proxy credentials, readonly observation, revoke and persistence', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-classroom-test-'));
  let received;
  const upstream = http.createServer((req, res) => { received = req.headers; res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify({projects: []})); });
  await new Promise(r => upstream.listen(0, '127.0.0.1', r));
  const backendPort = upstream.address().port;
  const store = new Store(dir, {password: 'test-admin'});
  const [alice, bob] = store.add(['Alice', 'Bob']);
  let rpc = [], starts = 0, stops = 0;
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), start: async () => { starts++; }, backend: async () => ({port: backendPort, apiPort: backendPort, token: 'BACKEND_ONLY'}), rpc: async (_, request) => { rpc.push(request); return request === 'List' ? {Sessions: [{id: 1, profile: 'shell', state: 'Idle'}]} : {Screen: {lines: [[{text: 'student screen'}]]}}; }};
  const terminalFile = path.join(dir, 'trusted.html');
  fs.writeFileSync(terminalFile, '<html><head></head><body>TRUSTED TERMINAL</body></html>');
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, terminalFile});
  driver.stop = async () => { stops++; };
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  async function request(url, body, cookie, extra = {}) {
    return fetch(origin + url, {method: body === undefined ? 'GET' : 'POST', headers: {...(body === undefined ? {} : {'Content-Type': 'application/json'}), ...(cookie ? {Cookie: cookie} : {}), ...extra}, body: body === undefined ? undefined : JSON.stringify(body)});
  }
  try {
    assert.equal((await request('/admin/api/state')).status, 401);
    assert.equal((await request('/admin/api/login', {password: 'wrong'})).status, 401);
    const adminLogin = await request('/admin/api/login', {password: 'test-admin'});
    const admin = adminLogin.headers.get('set-cookie').split(';')[0];
    assert.match(adminLogin.headers.get('set-cookie'), /HttpOnly/);
    assert.equal((await request('/admin/api/students', {names: ['Eve']}, admin, {Origin: 'https://evil.example'})).status, 403);
    const aliceLogin = await request(`/w/${alice.id}/login`, {token: alice.token});
    const student = aliceLogin.headers.get('set-cookie').split(';')[0];
    assert.equal((await request('/admin/api/state', undefined, student)).status, 401);
    assert.equal((await request(`/w/${bob.id}/_dct/projects`, undefined, student)).status, 401);
    assert.equal((await request(`/w/${alice.id}/_dct/projects`, undefined, admin)).status, 401);
    assert.equal((await request(`/w/${alice.id}/_dct/projects`, undefined, student)).status, 200);
    assert.equal(received.cookie, 'dct_gate=BACKEND_ONLY');
    assert.ok(!received.cookie.includes(student));
    assert.equal((await request(`/admin/workspaces/${alice.id}/view/`, undefined, admin)).status, 403);
    assert.equal((await request(`/admin/api/students/${alice.id}/observe`, {}, admin)).status, 404);
    const observed = await request(`/admin/api/students/${alice.id}/observe`, undefined, admin);
    assert.equal(observed.status, 200); assert.deepEqual(rpc, ['List', {Screen: {id: 1}}]);
    assert.equal((await request(`/admin/api/students/${alice.id}/assist`, {}, admin)).status, 200);
    const trusted = await request(`/admin/workspaces/${alice.id}/view/`, undefined, admin);
    assert.match(await trusted.text(), /TRUSTED TERMINAL/);
    upstream.removeAllListeners('request');
    upstream.on('request', (_, res) => { res.setHeader('Content-Type', 'text/html'); res.setHeader('Set-Cookie', 'dcw_admin=forged; Path=/admin/'); res.setHeader('Refresh', '0;url=/admin/'); res.end('<script>fetch("/admin/api/students")</script>'); });
    const untrusted = await request(`/admin/workspaces/${alice.id}/view/token`, undefined, admin);
    assert.match(untrusted.headers.get('content-type'), /^application\/json/);
    assert.equal(untrusted.headers.get('x-content-type-options'), 'nosniff');
    assert.equal(untrusted.headers.get('set-cookie'), null);
    assert.equal(untrusted.headers.get('refresh'), null);
    assert.equal((await (await request(`/w/${alice.id}/_dct/presence`, undefined, student)).json()).assisting, true);
    await request(`/admin/api/students/${alice.id}/end-assist`, {}, admin);
    assert.equal((await (await request(`/w/${alice.id}/_dct/presence`, undefined, student)).json()).assisting, false);
    assert.equal((await request(`/w/${alice.id}/finish`, {}, student)).status, 200);
    assert.equal(stops, 1);
    assert.equal((await request(`/w/${alice.id}/_dct/presence`, undefined, student)).status, 200);
    const state = await (await request('/admin/api/state', undefined, admin)).text();
    assert.ok(!state.includes(alice.token)); assert.ok(!state.includes('BACKEND_ONLY'));
    const oldToken = alice.token;
    assert.equal((await request(`/admin/api/students/${alice.id}/rotate`, {}, admin)).status, 200);
    assert.equal((await request(`/w/${alice.id}/_dct/projects`, undefined, student)).status, 401);
    assert.equal((await request(`/w/${alice.id}/login`, {token: oldToken})).status, 401);
    await request(`/admin/api/students/${alice.id}/disable`, {}, admin);
    assert.equal((await request(`/w/${alice.id}/login`, {token: alice.token})).status, 403);
    assert.equal(new Store(dir).data.students.length, 2);
    assert.equal(starts, 0);
  } finally { await app.close(); await new Promise(r => upstream.close(r)); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('Docker driver validates ownership, isolated volumes and 3 GiB budget', async () => {
  const {DockerDriver} = await import('./docker.mjs');
  const commands = [], id = 'a'.repeat(32);
  const w = {id, name: 'Student'};
  const driver = new DockerDriver({command: async (_, args) => { commands.push(args); if (args[0] === 'inspect') return {stdout: JSON.stringify([{Config: {Labels: {'dcw.classroom': 'wrong'}}, State: {Running: true}}])}; return {stdout: ''}; }});
  assert.throws(() => driver.name({id: '--privileged'}));
  await assert.rejects(driver.inspect(w), /标识/);
  assert.equal(commands.length, 1);
  // 共享工作区**不再**被一口回绝：在只有两三个名额的机器上，那条规矩的
  // 实际后果是共享的那个白占一个名额、全班只剩一个能用。现在它跟别的
  // 工作区走同一条路（没在跑就直接返回），代价写在界面的确认框里。
  const friendly = new DockerDriver({command: async (_, args) => args[0] === 'inspect'
    ? {stdout: JSON.stringify([{State: {Running: false}, Config: {Labels: {'dcw.classroom': '1'}}, NetworkSettings: {Ports: {}}, Mounts: []}])}
    : {stdout: ''}});
  await friendly.stop({shared: true, containerName: 'dcw-workspace-1'});

  // 名额和单容器内存都能用环境变量压过去——一台机器该跑几个学生是部署
  // 时的判断，不该是重新发一版才能改的东西。
  assert.equal(new DockerDriver({maxRunning: 5, memory: '1200m'}).maxRunning, 5);
  assert.equal(new DockerDriver({maxRunning: 5, memory: '1200m'}).memory, '1200m');
});

test('强制直播：只上架活着的会话、路名用学生名字、旧版本 dct 给一句人话', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-live-test-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  let live = null, sent = [], speaksLive = true;
  const driver = {
    maxRunning: 2,
    status: async () => ({status: 'running'}),
    rpc: async (_, request) => {
      sent.push(request);
      if (request === 'List') {
        return {Sessions: [
          {id: 1, profile: 'claude', state: 'Idle'},
          {id: 2, profile: 'shell', state: 'Stopped'},
          {id: 3, profile: 'codex', state: 'Working'},
        ]};
      }
      if (request === 'LiveStatus') return {Live: live || {id: '', token: '', url: '', staged: [], viewers: 0, readiness: 'Pending'}};
      if (request === 'LiveStop') { live = null; return {Ok: null}; }
      if (request.LiveStart) {
        // 旧版本的守护进程不认识这条请求，回的就是这句。
        if (!speaksLive) return {Error: {BadRequest: 'unknown variant `LiveStart`'}};
        live = {id: 'abc', token: 't'.repeat(64), url: 'https://live.example/live/abc#t=' + 't'.repeat(64),
                staged: request.LiveStart.ids.map((id, i) => [id, request.LiveStart.names[i]]), viewers: 3, readiness: 'Ready'};
        return {Live: live};
      }
      return {Ok: null};
    },
  };
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  const request = (url, body, cookie) => fetch(origin + url, {
    method: body === undefined ? 'GET' : 'POST',
    headers: {...(body === undefined ? {} : {'Content-Type': 'application/json'}), ...(cookie ? {Cookie: cookie} : {})},
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  try {
    const admin = (await request('/admin/api/login', {password: 'test-admin'})).headers.get('set-cookie').split(';')[0];

    const started = await request(`/admin/api/students/${ming.id}/live-start`, {}, admin);
    assert.equal(started.status, 200);
    const payload = await started.json();
    assert.equal(payload.live.viewers, 3);
    assert.match(payload.live.url, /^https:\/\/live\.example\/live\/abc#t=/);

    // 已停止的那一路不该被播出去：学生看到的会是一屏死画面，分不清是
    // 「老师还没开始」还是「这一路本来就没东西」。
    const start = sent.find(r => r.LiveStart);
    assert.deepEqual(start.LiveStart.ids, [1, 3], '上架的不该包含已停止的会话');
    // 路名用学生的名字——「第 1 路」对看的人毫无信息。
    assert.deepEqual(start.LiveStart.names, ['小明 · 1', '小明 · 2']);

    // 列表上要看得见谁在播、几个人在看。
    const state = await (await request('/admin/api/state', undefined, admin)).json();
    assert.equal(state.students.find(s => s.id === ming.id).live.viewers, 3);

    const stopped = await request(`/admin/api/students/${ming.id}/live-stop`, {}, admin);
    assert.equal(stopped.status, 200);
    assert.equal((await stopped.json()).live, null);
    assert.ok(sent.includes('LiveStop'));

    // 旧镜像的工作区：老师该看到「换镜像」，不是「unknown variant」。
    speaksLive = false;
    const refused = await request(`/admin/api/students/${ming.id}/live-start`, {}, admin);
    assert.equal(refused.status, 409);
    assert.match((await refused.json()).error, /版本还不支持直播/);
  } finally {
    app.server.close();
    fs.rmSync(dir, {recursive: true, force: true});
  }
});

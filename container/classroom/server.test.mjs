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

// 换镜像这件事以前只存在于提示文案里：「请先停止它再启动（会换成新镜像）」。
// 可 `stop()` 只是 `docker stop`，`start()` 看到容器还在就 `docker start`——
// 拉起来的永远是当初那个镜像，老师照着点一百遍也换不上。
// dct 不再带内置中转地址，所以直播中转必须由部署方交给工作区——而交给它的
// 唯一途径是创建容器时的环境变量。没配就不传：传一个空的 `DCT_RELAY=` 进去
// 跟没传是一回事，只会让人以为配过了。
test('Docker driver: 配了 CLASSROOM_LIVE_RELAY 才把 DCT_RELAY 交给新工作区', async () => {
  const {DockerDriver} = await import('./docker.mjs');
  const id = 'c'.repeat(32), w = {id, name: 'Student'};
  const runWith = async relay => {
    const commands = [];
    const driver = new DockerDriver({image: 'img', maxRunning: 9, relay, command: async (_, args) => {
      commands.push(args);
      if (args[0] === 'inspect') { const e = new Error('none'); e.stderr = 'No such object'; throw e; }
      return {stdout: ''};
    }});
    driver.backend = async () => ({port: 1, apiPort: 1, token: 't'});
    await driver.start(w).catch(() => {});
    return commands.find(c => c[0] === 'run');
  };
  assert.ok((await runWith('https://relay.example')).includes('DCT_RELAY=https://relay.example'));
  assert.ok(!(await runWith(undefined)).some(a => String(a).startsWith('DCT_RELAY')), '没配就不许传');
});

test('Docker driver: 停着的容器镜像过期了，启动时删容器、留卷、用新镜像重建', async () => {
  const {DockerDriver} = await import('./docker.mjs');
  const id = 'b'.repeat(32), w = {id, name: 'Student'};
  const drive = ({containerImage, shared = false}) => {
    const commands = [];
    const driver = new DockerDriver({image: 'dc-workspace:new', maxRunning: 9, command: async (_, args) => {
      commands.push(args);
      if (args[0] === 'inspect') {
        // 删掉之后再问就是「没有这个容器」，跟真 docker 一样
        if (commands.some(c => c[0] === 'rm')) { const e = new Error('gone'); e.stderr = 'No such object'; throw e; }
        return {stdout: JSON.stringify([{Image: containerImage, State: {Running: false}, Config: {Labels: {'dcw.classroom': '1', 'dcw.student': id}}, NetworkSettings: {Ports: {}}, Mounts: []}])};
      }
      if (args[0] === 'image') return {stdout: 'sha256:new\n'};
      return {stdout: ''};
    }});
    // 真去等工作区就绪要 10 秒，这里只关心 docker 被怎么调
    driver.backend = async () => ({port: 1, apiPort: 1, token: 't'});
    return {driver, commands};
  };

  {
    const {driver, commands} = drive({containerImage: 'sha256:old'});
    await driver.start(w).catch(() => {});
    const verbs = commands.map(c => c[0]);
    assert.ok(verbs.includes('rm'), `镜像过期必须删掉旧容器：${JSON.stringify(verbs)}`);
    assert.ok(!verbs.includes('start'), '删掉之后不该再 docker start 那个旧的');
    const rm = commands.find(c => c[0] === 'rm');
    assert.ok(!rm.includes('-v') && !rm.includes('--volumes'), '只删容器，四个命名卷（作品、登录态）必须留着');
    const run = commands.find(c => c[0] === 'run');
    assert.ok(run, '删完要用新镜像重建');
    assert.equal(run.at(-1), 'dc-workspace:new');
    assert.ok(run.includes(`type=volume,source=dcw-student-${id}-work,target=/home/dc/work`), '重建必须挂回原来的作品卷');
  }
  {
    // 镜像没变：照旧 docker start，不许白白删一次容器
    const {driver, commands} = drive({containerImage: 'sha256:new'});
    await driver.start(w).catch(() => {});
    const verbs = commands.map(c => c[0]);
    assert.ok(verbs.includes('start') && !verbs.includes('rm') && !verbs.includes('run'), `镜像没变不该重建：${JSON.stringify(verbs)}`);
  }
  {
    // 共享工作区不是这里建的，删了就再也建不回来——镜像再旧也只 start
    const {driver, commands} = drive({containerImage: 'sha256:old'});
    await driver.start({shared: true, containerName: 'dcw-workspace-1', id}).catch(() => {});
    const verbs = commands.map(c => c[0]);
    assert.ok(!verbs.includes('rm') && !verbs.includes('run'), `共享工作区绝不能被删：${JSON.stringify(verbs)}`);
  }
});

test('强制直播：只上架活着的会话、路名用学生名字、旧版本 dct 给一句人话', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-live-test-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  let live = null, sent = [], speaksLive = true, hasRelay = true;
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
        // 没带参数的错误码在线上就是一个裸字符串，不是对象。
        if (!hasRelay) return {Error: 'LiveRelayNotConfigured'};
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

    // 工作区里的 dct 没配中转：老师该看到「管理员去配中转」，不是一个
    // 被 `Object.values` 拆成单个字母的 "L"。
    speaksLive = true; hasRelay = false;
    const noRelay = await request(`/admin/api/students/${ming.id}/live-start`, {}, admin);
    assert.equal(noRelay.status, 409);
    assert.match((await noRelay.json()).error, /没有配置直播中转/);
  } finally {
    app.server.close();
    fs.rmSync(dir, {recursive: true, force: true});
  }
});

test('语音输入：没配就给人话、太大就拒、key 不出现在答复里', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-stt-test-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async () => ({Live: {id: ''}})};

  // 假的 dc_llm 网关：把它收到的 Authorization 记下来，回一段转写。
  let seenAuth = null;
  const gateway = http.createServer(async (req, res) => {
    seenAuth = req.headers.authorization;
    for await (const _ of req) { /* 读完 */ }
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify({text: '把这一行改成 import os'}));
  });
  await new Promise(r => gateway.listen(0, '127.0.0.1', r));
  const gatewayUrl = `http://127.0.0.1:${gateway.address().port}`;

  const unconfigured = new Classroom({store, driver, origin: 'http://localhost', secure: false, stt: {url: '', key: ''}});
  await new Promise(r => unconfigured.server.listen(0, '127.0.0.1', r));
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false,
    stt: {url: gatewayUrl, key: 'SECRET-GATEWAY-KEY', model: 'whisper-1', language: 'zh'}});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));

  const speak = async (server, bytes, cookie) => fetch(`http://127.0.0.1:${server.address().port}/w/${ming.id}/_dct/transcribe`, {
    method: 'POST', headers: {'Content-Type': 'audio/webm', ...(cookie ? {Cookie: cookie} : {})}, body: Buffer.alloc(bytes, 1),
  });
  try {
    const login = await fetch(`http://127.0.0.1:${app.server.address().port}/w/${ming.id}/login`, {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({token: ming.token})});
    const student = login.headers.get('set-cookie').split(';')[0];

    // 没登录的人不能拿它当免费的转写服务用。
    assert.equal((await speak(app.server, 16)).status, 401);

    // 没配 key：一句人话，不是 500。（会话是跟着实例走的，这台要自己登一次。）
    const offLogin = await fetch(`http://127.0.0.1:${unconfigured.server.address().port}/w/${ming.id}/login`, {
      method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({token: ming.token})});
    const off = await speak(unconfigured.server, 16, offLogin.headers.get('set-cookie').split(';')[0]);
    assert.equal(off.status, 503);
    assert.match((await off.json()).error, /还没配/);

    const ok = await speak(app.server, 2048, student);
    assert.equal(ok.status, 200);
    assert.equal((await ok.json()).text, '把这一行改成 import os');
    assert.equal(seenAuth, 'Bearer SECRET-GATEWAY-KEY', 'key 该由服务端带上，不该让浏览器碰');

    // 超过上限的录音在读进内存之前就被挡掉。
    const big = await speak(app.server, 6 * 1024 * 1024, student);
    assert.equal(big.status, 413);
    assert.doesNotMatch(JSON.stringify(await big.json()), /SECRET-GATEWAY-KEY/, 'key 绝不能出现在给浏览器的答复里');
  } finally {
    app.server.close(); unconfigured.server.close(); gateway.close();
    fs.rmSync(dir, {recursive: true, force: true});
  }
});

test('公开直播：凭证 + 管理台密钥去中转公开；错误码说人话；自愈遇到吊销就停', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-public-test-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  // 假中转：记下请求，按吩咐回状态码，并维护一份「公开列表」。room01 全程
  // 当成中转已经认得的房间（守护进程一直在跑），跟「中转还没认得」是另一个
  // 场景，见下面专门的自愈测试。
  const seen = []; let putStatus = 204; const listed = new Set(); const registered = new Set(['room01']);
  const relay = http.createServer((req, res) => {
    let body = ''; req.on('data', c => body += c); req.on('end', () => {
      seen.push({method: req.method, url: req.url, headers: req.headers, body});
      if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end(JSON.stringify([...listed].map(id => ({id, title: 't', lanes: [], viewers: 0})))); }
      const lanes = req.url.match(/^\/live\/([^/]+)\/lanes$/);
      if (lanes && req.method === 'GET') { if (registered.has(lanes[1]) && req.headers['x-live-token'] === 't'.repeat(64)) { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); } res.writeHead(401); return res.end(); }
      const m = req.url.match(/^\/live\/([^/]+)\/public$/);
      if (m && req.method === 'PUT') { if (putStatus === 204) listed.add(m[1]); res.writeHead(putStatus); return res.end(); }
      if (m && req.method === 'DELETE') { listed.delete(m[1]); res.writeHead(204); return res.end(); }
      res.writeHead(404); res.end();
    });
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  let live = {id: 'room01', token: 't'.repeat(64), url: `${relayUrl}/live/room01#t=${'t'.repeat(64)}`, staged: [[1, '小明']], viewers: 2, readiness: 'Ready', public: 'Private'};
  const driver = {
    maxRunning: 2,
    status: async () => ({status: 'running'}),
    rpc: async (_, request) => {
      if (request === 'LiveStatus') return {Live: live};
      if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)};
      if (request === 'List') return {Sessions: [{id: 1, profile: 'claude', state: 'Idle'}]};
      if (request.LiveStart) return {Live: live};
      return {Ok: null};
    },
  };
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  const request = (url, body, cookie) => fetch(origin + url, {method: body === undefined ? 'GET' : 'POST', headers: {...(body === undefined ? {} : {'Content-Type': 'application/json'}), ...(cookie ? {Cookie: cookie} : {})}, body: body === undefined ? undefined : JSON.stringify(body)});
  try {
    const admin = (await request('/admin/api/login', {password: 'test-admin'})).headers.get('set-cookie').split(';')[0];
    assert.equal((await (await request('/admin/api/state', undefined, admin)).json()).features.publicLive, true);

    const ok = await request(`/admin/api/students/${ming.id}/live-public`, {title: '小明 的工作区'}, admin);
    assert.equal(ok.status, 200);
    const put = seen.find(s => s.method === 'PUT');
    assert.equal(put.url, '/live/room01/public');
    assert.equal(put.headers['x-live-grant'], 'g'.repeat(64), '用凭证，不用推帧钥匙');
    assert.deepEqual(JSON.parse(put.body), {title: '小明 的工作区', key: 'K'.repeat(64)});
    assert.deepEqual(new Store(dir).student(ming.id).livePublic, {title: '小明 的工作区', id: 'room01'});
    assert.ok(new Store(dir).data.audit.some(a => /公开直播工作区/.test(a.action || a.text || JSON.stringify(a))));

    // 自愈：中转重启丢了公开状态 → 重新 PUT
    listed.clear(); seen.length = 0;
    await app.healPublic();
    assert.ok(seen.some(s => s.method === 'PUT'), '中转说不公开时要重新公开');

    // 吊销：中转回 401 → 不再重试、清记录、行上带原因
    listed.clear(); putStatus = 401; seen.length = 0;
    await app.healPublic();
    await app.healPublic();
    assert.equal(seen.filter(s => s.method === 'PUT').length, 1, '被吊销之后又去撞了中转');
    const state = await (await request('/admin/api/state', undefined, admin)).json();
    const row = state.students.find(s => s.id === ming.id);
    assert.match(row.livePublicError, /吊销/);

    // T9：直播停掉之后不该还留着旧的公开错误提示
    const stopped = await request(`/admin/api/students/${ming.id}/live-stop`, {}, admin);
    assert.equal(stopped.status, 200);
    assert.equal(new Store(dir).student(ming.id).livePublicError, undefined, '停止直播应该清掉公开错误提示');

    // 错误映射
    for (const [code, text] of [[401, /吊销/], [403, /没有开启公开直播|下线/], [413, /标题太长/]]) {
      putStatus = code;
      const r = await request(`/admin/api/students/${ming.id}/live-public`, {title: 't'}, admin);
      assert.equal(r.status, 409);
      assert.match((await r.json()).error, text);
    }

    // 取消公开
    putStatus = 204;
    await request(`/admin/api/students/${ming.id}/live-public`, {title: 't'}, admin);
    const off = await request(`/admin/api/students/${ming.id}/live-private`, {}, admin);
    assert.equal(off.status, 200);
    assert.ok(seen.some(s => s.method === 'DELETE' && s.headers['x-live-grant'] === 'g'.repeat(64)));
    assert.equal(new Store(dir).student(ming.id).livePublic, undefined);
  } finally { app.server.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('没配发布密钥：管理台不开公开功能', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-public-off-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async () => ({Ok: null})};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false});
  await new Promise(r => app.server.listen(0, '127.0.0.1', r));
  const origin = `http://127.0.0.1:${app.server.address().port}`;
  const request = (url, body, cookie) => fetch(origin + url, {method: body === undefined ? 'GET' : 'POST', headers: {...(body === undefined ? {} : {'Content-Type': 'application/json'}), ...(cookie ? {Cookie: cookie} : {})}, body: body === undefined ? undefined : JSON.stringify(body)});
  try {
    const admin = (await request('/admin/api/login', {password: 'test-admin'})).headers.get('set-cookie').split(';')[0];
    assert.equal((await (await request('/admin/api/state', undefined, admin)).json()).features.publicLive, false);
    assert.equal((await request(`/admin/api/students/${ming.id}/live-public`, {title: 't'}, admin)).status, 404);
  } finally { app.server.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：daemon RPC 失败时保留公开记录，不清空、不写审计', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-rpcfail-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播'}; store.audit('公开直播工作区', ming); });
  const auditBefore = store.data.audit.length;
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') throw new Error('daemon socket timeout'); return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await app.healPublic();
    assert.deepEqual(store.data.students.find(s => s.id === ming.id).livePublic, {title: '小明的直播'}, 'RPC 打不通不该被当成「没在播」');
    assert.equal(store.data.audit.length, auditBefore, '不该因为一次 RPC 失败就写审计');
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：daemon 确认没在播时清空公开记录', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-notlive-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播'}; store.audit('公开直播工作区', ming); });
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') return {Live: {id: ''}}; return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await app.healPublic();
    assert.equal(store.data.students.find(s => s.id === ming.id).livePublic, undefined);
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：重入守卫挡住并发调用，中转只挨一次 PUT', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-concurrent-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播'}; store.audit('公开直播工作区', ming); });
  let puts = 0;
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    if (req.method === 'PUT' && req.url === '/live/room01/public') { puts++; return setTimeout(() => { res.writeHead(204); res.end(); }, 200); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const live = {id: 'room01', url: `${relayUrl}/live/room01#t=${'t'.repeat(64)}`, viewers: 0, readiness: 'Ready', public: 'Private'};
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') return {Live: live}; if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)}; return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await Promise.all([app.healPublic(), app.healPublic()]);
    assert.equal(puts, 1, '第二次调用该被重入守卫挡住');
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：中转刚重启还没认得房间时，401 不能当成密钥被吊销——留着记录，等房间出现再补公开', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-notregistered-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播', id: 'room01'}; store.audit('公开直播工作区', ming); });
  let registered = false, puts = 0;
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    if (req.method === 'GET' && req.url === '/live/room01/lanes') { if (registered && req.headers['x-live-token'] === 't'.repeat(64)) { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); } res.writeHead(401); return res.end(); }
    if (req.method === 'PUT' && req.url === '/live/room01/public') { puts++; res.writeHead(registered ? 204 : 401); return res.end(); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const live = {id: 'room01', url: `${relayUrl}/live/room01#t=${'t'.repeat(64)}`, viewers: 0, readiness: 'Ready', public: 'Private'};
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') return {Live: live}; if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)}; return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await app.healPublic();
    assert.deepEqual(store.data.students.find(s => s.id === ming.id).livePublic, {title: '小明的直播', id: 'room01'}, '房间还没在中转注册，不该清记录');
    assert.equal(store.data.students.find(s => s.id === ming.id).livePublicError, undefined, '房间还没注册不算被拒绝，不该报错');

    registered = true;
    await app.healPublic();
    assert.deepEqual(store.data.students.find(s => s.id === ming.id).livePublic, {title: '小明的直播', id: 'room01'}, '房间出现之后记录还在');
    assert.equal(store.data.students.find(s => s.id === ming.id).livePublicError, undefined);
    assert.equal(puts, 2, '两轮都该去 PUT，第二次才成功');
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：房间一直在中转注册着，PUT 却始终 401——是真的密钥被吊销，照常清记录报错', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-realreject-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播', id: 'room01'}; store.audit('公开直播工作区', ming); });
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    if (req.method === 'GET' && req.url === '/live/room01/lanes') { if (req.headers['x-live-token'] === 't'.repeat(64)) { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); } res.writeHead(401); return res.end(); }
    if (req.method === 'PUT' && req.url === '/live/room01/public') { res.writeHead(401); return res.end(); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const live = {id: 'room01', url: `${relayUrl}/live/room01#t=${'t'.repeat(64)}`, viewers: 0, readiness: 'Ready', public: 'Private'};
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') return {Live: live}; if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)}; return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await app.healPublic();
    assert.equal(store.data.students.find(s => s.id === ming.id).livePublic, undefined, '房间在，密钥被拒就该清记录');
    assert.match(store.data.students.find(s => s.id === ming.id).livePublicError, /吊销/);
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：房间号变了说明那场直播已经播完，清记录但不当成被拒绝，也不去公开新房间', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-roomchanged-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播', id: 'roomA'}; store.audit('公开直播工作区', ming); });
  const auditBefore = store.data.audit.length;
  let puts = 0;
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    if (req.method === 'PUT') { puts++; res.writeHead(204); return res.end(); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const live = {id: 'roomB', url: `${relayUrl}/live/roomB#t=${'t'.repeat(64)}`, viewers: 0, readiness: 'Ready', public: 'Private'};
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') return {Live: live}; if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)}; return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await app.healPublic();
    assert.equal(store.data.students.find(s => s.id === ming.id).livePublic, undefined, '老房间那场已经结束，该清记录');
    assert.equal(store.data.students.find(s => s.id === ming.id).livePublicError, undefined, '这不是被拒绝，不该报错');
    assert.equal(puts, 0, '不该去公开新房间——那要老师重新点');
    assert.equal(store.data.audit.length, auditBefore, '不该因为房间号变了就写审计');
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

test('自愈：老版本写的记录没有房间号，认领当前房间而不是清掉', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-heal-legacyid-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  await store.mutate(async () => { ming.livePublic = {title: '小明的直播'}; store.audit('公开直播工作区', ming); });
  const relay = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end('[]'); }
    if (req.method === 'PUT' && req.url === '/live/room01/public') { res.writeHead(204); return res.end(); }
    res.writeHead(404); res.end();
  });
  await new Promise(r => relay.listen(0, '127.0.0.1', r));
  const relayUrl = `http://127.0.0.1:${relay.address().port}`;
  const live = {id: 'room01', url: `${relayUrl}/live/room01#t=${'t'.repeat(64)}`, viewers: 0, readiness: 'Ready', public: 'Private'};
  const driver = {maxRunning: 2, status: async () => ({status: 'running'}), rpc: async (_, request) => { if (request === 'LiveStatus') return {Live: live}; if (request === 'LivePublishGrant') return {LiveGrant: 'g'.repeat(64)}; return {Ok: null}; }};
  const app = new Classroom({store, driver, origin: 'http://localhost', secure: false, live: {relay: relayUrl, publishKey: 'K'.repeat(64)}});
  try {
    await app.healPublic();
    assert.deepEqual(store.data.students.find(s => s.id === ming.id).livePublic, {title: '小明的直播', id: 'room01'}, '应该认领当前房间，不是清掉');
  } finally { await app.close(); relay.close(); fs.rmSync(dir, {recursive: true, force: true}); }
});

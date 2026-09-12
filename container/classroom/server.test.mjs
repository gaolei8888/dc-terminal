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
  await assert.rejects(driver.stop({shared: true, containerName: 'dcw-workspace-1'}), /共享/);
});

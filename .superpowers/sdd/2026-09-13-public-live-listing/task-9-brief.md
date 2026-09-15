### Task 9: 管理台后端 —— 公开、取消、自愈、错误映射

**Files:**
- Modify: `container/classroom/server.mjs`
- Modify: `container/classroom/server.test.mjs`

**Interfaces:**
- Consumes: 守护进程 `LivePublishGrant`（Task 7）、中转 `PUT/DELETE /live/{id}/public`、`GET /live/public`（Task 4）。
- Produces:
  - `new Classroom({..., live: {relay: string, publishKey: string}})`（缺省或缺 `publishKey` = 不开公开功能）
  - 管理接口：`POST /admin/api/students/{id}/live-public` `{title}`、`POST /admin/api/students/{id}/live-private`；`live-start` 接受 `{public: true, title}`
  - `GET /admin/api/state` 答复多 `features: {publicLive: boolean}`；每行 `live` 多 `public: {title} | null`、学生行多 `livePublicError: string | null`
  - `Classroom#healPublic()`（后台每 30 秒调一次；测试直接调）

- [ ] **Step 1: 写失败的测试**

在 `server.test.mjs` 追加（沿用「强制直播」那条测试的 `Store` / fake driver / `request` 写法）：

```js
test('公开直播：凭证 + 管理台密钥去中转公开；错误码说人话；自愈遇到吊销就停', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcw-public-test-'));
  const store = new Store(dir, {password: 'test-admin'});
  const [ming] = store.add(['小明']);
  // 假中转：记下请求，按吩咐回状态码，并维护一份「公开列表」
  const seen = []; let putStatus = 204; const listed = new Set();
  const relay = http.createServer((req, res) => {
    let body = ''; req.on('data', c => body += c); req.on('end', () => {
      seen.push({method: req.method, url: req.url, headers: req.headers, body});
      if (req.method === 'GET' && req.url === '/live/public') { res.writeHead(200, {'content-type': 'application/json'}); return res.end(JSON.stringify([...listed].map(id => ({id, title: 't', lanes: [], viewers: 0})))); }
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
    assert.deepEqual(new Store(dir).student(ming.id).livePublic, {title: '小明 的工作区'});
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
```

（`store.student(id)` 若不存在，按 `server.mjs` 里取学生记录的现有方法改；审计记录的字段名按 `Store#audit` 的真实实现改断言。）文件顶部 `import http from 'node:http'` 若没有就加。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd container/classroom && node --test server.test.mjs`
Expected: 两条新测试 FAIL。

- [ ] **Step 3: 实现**

`server.mjs`：
1. 构造函数接 `live` 选项并起自愈定时器：

```js
    // 公开直播：管理台自己的发布密钥 + 中转地址。缺一样就不开这个功能。
    this.publicLive = live && live.relay && live.publishKey ? {relay: live.relay.replace(/\/+$/, ''), key: live.publishKey} : null;
    if (this.publicLive) this.healTimer = setInterval(() => this.healPublic().catch(() => {}), 30000);
```

`close()` 里 `clearInterval(this.healTimer)`。
2. 错误映射与中转调用：

```js
const PUBLIC_ERRORS = {
  401: '发布密钥无效或已吊销',
  403: '服务器没有开启公开直播，或这场直播已被下线',
  413: '标题太长（最多 60 个字）',
  429: '操作太频繁，请一分钟后再试',
};
```

```js
  // 对中转公开 / 取消公开这一场。凭证由学生工作区的守护进程签发，发布密钥只在管理台进程里。
  async relayPublic(w, method, title) {
    const status = await this.liveStatus(w);
    if (!status) throw fail(409, '这个工作区现在没在直播');
    const id = new URL(status.url).pathname.split('/').pop();
    const answer = await this.liveRpc(w, 'LivePublishGrant');
    const grant = answer.LiveGrant;
    if (typeof grant !== 'string') throw fail(409, '这个工作区的 dct 版本还不支持，请先停止它再启动（会换成新镜像）');
    const headers = {'x-live-grant': grant};
    let body;
    if (method === 'PUT') { headers['content-type'] = 'application/json'; body = JSON.stringify({title: [...title].slice(0, 60).join(''), key: this.publicLive.key}); }
    let res;
    try { res = await fetch(`${this.publicLive.relay}/live/${encodeURIComponent(id)}/public`, {method, headers, body}); }
    catch { throw fail(409, '连不上直播中转，请稍后再试'); }
    if (res.status !== 204) throw Object.assign(fail(409, PUBLIC_ERRORS[res.status] || `直播中转拒绝了（状态码 ${res.status}）`), {relayStatus: res.status});
    return id;
  }

  // 后台自愈：记着要公开的工作区，中转说不公开就再公开一次；被吊销/下线就停。
  async healPublic() {
    if (!this.publicLive) return;
    let listed;
    try { const r = await fetch(`${this.publicLive.relay}/live/public`); listed = new Set((await r.json()).map(x => x.id)); }
    catch { return; }
    for (const w of this.store.data.students.filter(s => s.livePublic)) {
      await this.store.mutate(async () => {
        const status = await this.liveStatus(w).catch(() => null);
        if (!status) { delete w.livePublic; return; }
        const id = new URL(status.url).pathname.split('/').pop();
        if (listed.has(id)) return;
        try { await this.relayPublic(w, 'PUT', w.livePublic.title); delete w.livePublicError; }
        catch (e) {
          if (e.relayStatus === 401 || e.relayStatus === 403) {
            w.livePublicError = e.message;
            delete w.livePublic;
            this.store.audit(`公开直播被中转拒绝（${e.message}）`, w);
          }
        }
      });
    }
  }
```

（`fail(...)` 的返回值若不是 `Error` 子类，把 `Object.assign` 换成对应写法；`store.mutate` 的用法照文件里现有调用。）
3. 路由：在 `live-start` / `live-stop` 那一段之前加：

```js
      if (action === 'live-public' || action === 'live-private') {
        if (!this.publicLive) throw fail(404, '接口不存在');
        if (action === 'live-public') {
          const title = String(data.title || '').trim();
          if (!title) throw fail(400, '请填写公开标题');
          await this.relayPublic(w, 'PUT', title);
          w.livePublic = {title}; delete w.livePublicError;
          this.store.audit('公开直播工作区', w);
        } else {
          await this.relayPublic(w, 'DELETE');
          delete w.livePublic;
          this.store.audit('取消公开直播', w);
        }
        return json(res, 200, {live: await this.liveStatus(w)});
      }
```

这一段需要请求体：把 `await body(req);` 改成 `const data = await body(req);`（确认 `body()` 的返回值就是解析后的对象）。`live-start` 成功之后若 `data.public && this.publicLive` 就接着走一次 `live-public` 的逻辑（抽成一个小函数复用，失败时直播照常开、答复里带 `publicError`）。`live-stop` 成功后 `delete w.livePublic`。
4. `liveStatus` 的返回值加 `public: info.public && info.public.Listed ? {title: info.public.Listed.title} : null`；`safeRow` 加 `livePublicError: w.livePublicError || null`；`state()` 答复加 `features: {publicLive: !!this.publicLive}`（按 `state()` 现有返回结构放）。
5. 入口：

```js
  const publishKeyFile = process.env.CLASSROOM_LIVE_PUBLISH_KEY_FILE;
  const live = publishKeyFile ? {relay: process.env.CLASSROOM_LIVE_RELAY, publishKey: fs.readFileSync(publishKeyFile, 'utf8').trim()} : undefined;
```

传给 `new Classroom({..., live})`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd container/classroom && node --test server.test.mjs publishing.test.mjs sso.test.mjs`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`healPublic` 里去掉 401/403 时的 `delete w.livePublic`（「被吊销之后又去撞了中转」红）；`relayPublic` 把 `x-live-grant` 改成空串（凭证断言红）。改回。

- [ ] **Step 6: Commit**

```bash
git add container/classroom/server.mjs container/classroom/server.test.mjs
git commit -m "feat(classroom): publish student lives through a daemon grant, heal after relay restarts"
```

---


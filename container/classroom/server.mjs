import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import {fileURLToPath} from 'node:url';
import {Store, secret, digest, equal} from './store.mjs';
import {Federation} from './sso.mjs';
import {Publishing} from './publishing.mjs';
import {DockerDriver, localRequest} from './docker.mjs';

const here = fileURLToPath(new URL('.', import.meta.url));
const json = (res, status, value) => { const body = JSON.stringify(value); res.writeHead(status, {'Content-Type': 'application/json; charset=utf-8', 'Content-Length': Buffer.byteLength(body), 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff'}); res.end(body); };
const html = (res, body) => { res.writeHead(200, {'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer', 'Content-Security-Policy': "frame-ancestors 'self'"}); res.end(body); };
const fail = (status, message) => Object.assign(new Error(message), {status});
// 一场直播最多几路。**跟 dct 那边的 `dct_link::live::MAX_LANES` 是同一个数**，
// 超了守护进程会拒，而那时候老师已经点下去了——在这儿先截断，他看到的是
// 「播了前四路」而不是一句拒绝。
const LIVE_MAX_LANES = 4;
const js = value => JSON.stringify(value).replaceAll('<', '\\u003c');
async function body(req) {
  if (!String(req.headers['content-type'] || '').startsWith('application/json')) throw fail(400, '请求格式无效');
  let size = 0, chunks = [];
  for await (const chunk of req) { size += chunk.length; if (size > 16384) throw fail(413, '请求过大'); chunks.push(chunk); }
  try { const data = JSON.parse(Buffer.concat(chunks)); if (!data || Array.isArray(data) || typeof data !== 'object') throw Error(); return data; }
  catch { throw fail(400, '请求格式无效'); }
}
function cookie(req, name) { return (req.headers.cookie || '').split(';').map(x => x.trim()).find(x => x.startsWith(name + '='))?.slice(name.length + 1); }
function studentPage(prefix, starting = false) {
  return `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>学习工作区</title><style>body{font:16px/1.7 system-ui;background:#f5f7fa;color:#263248;display:grid;place-content:center;min-height:90vh;padding:24px}main{max-width:430px}button{font:inherit;padding:10px 18px;border:0;border-radius:8px;background:#315bea;color:white}</style><main><h1>学习工作区</h1><p id="message">${starting ? '正在打开你的工作区…' : '请使用老师发给你的完整链接。'}</p><button hidden id="retry">重试</button></main><script>
const prefix=${js(prefix)},starting=${starting};const message=document.querySelector('#message');async function open(){try{const token=new URLSearchParams(location.hash.slice(1)).get('t');if(token){history.replaceState(null,'',location.pathname);const r=await fetch(prefix+'/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token})});if(!r.ok)throw Error((await r.json()).error);location.reload();return}if(starting){const r=await fetch(prefix+'/start',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});if(!r.ok)throw Error((await r.json()).error);location.reload()}}catch(e){message.textContent=e.message;document.querySelector('#retry').hidden=false}}document.querySelector('#retry').onclick=open;open();</script></html>`;
}
function finishedPage(prefix) {
  return `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>学习已结束</title><body style="font:16px/1.7 system-ui;padding:8vh 8vw;background:#f5f7fa"><h1>本次学习已结束</h1><p>你的作品已保留，下次可以继续。</p><button style="font:inherit;padding:10px 20px" id="continue">继续学习</button><p id="error"></p><script>document.querySelector('#continue').onclick=async function(){this.disabled=true;try{const r=await fetch(${js(prefix + '/start')},{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});if(!r.ok)throw Error((await r.json()).error);location.href=${js(prefix + '/')};}catch(e){document.querySelector('#error').textContent=e.message;this.disabled=false;}};</script></body></html>`;
}

export class Classroom {
  constructor({store, driver, origin = 'https://dataclue.cn', secure = true, terminalFile = here + 'terminal.html', issuers = {}, ssoAllowHttp = false, publishing} = {}) {
    this.store = store;
    this.driver = driver;
    this.origin = new URL(origin).origin;
    this.secure = secure;
    this.terminalFile = terminalFile;
    this.sso = new Federation(this, issuers, ssoAllowHttp);
    this.publishing = publishing ? new Publishing(this, publishing) : null;
    this.sessions = new Map((store.data.sessions || []).filter(s => s.expires > Date.now()).map(s => [s.key, s]));
    this.assistance = new Map(); this.connections = new Set(); this.rates = new Map(); this.stats = null;
    this.server = http.createServer((req, res) => this.handle(req, res).catch(e => { if (!res.headersSent) json(res, e.status || 409, {error: e.publicMessage || (e.status ? e.message : '操作未完成，请稍后重试；学生文件已保留')}); else res.destroy(); }));
    this.server.on('upgrade', (req, socket, head) => this.upgrade(req, socket, head).catch(() => socket.destroy()));
    this.timer = setInterval(() => {
      for (const [id, lease] of this.assistance) if (lease.expires < Date.now()) this.endAssistance(id);
      for (const c of this.connections) if (!this.sessions.has(c.session.key) || c.session.expires < Date.now()) c.socket.destroy();
      const active = new Map([...this.connections].map(c => [c.session.key, c.session]));
      for (const session of active.values()) if (session.external) this.sso.ensure(session).catch(() => {});
    }, 5000).unref();
  }
  close() { clearInterval(this.timer); for (const c of this.connections) c.socket.destroy(); this.publishing?.server.close(); return new Promise(r => this.server.close(r)); }
  writeOrigin(req) {
    if (req.headers.origin && req.headers.origin !== this.origin) throw fail(403, '请从当前管理台或工作区操作');
    if (req.headers['sec-fetch-site'] && !['same-origin', 'none'].includes(req.headers['sec-fetch-site'])) throw fail(403, '请从当前工作区操作');
    // Browser mutation APIs always send JSON; form submissions cannot reach them.
  }
  rate(req, suffix) {
    const key = req.socket.remoteAddress + ':' + suffix, now = Date.now();
    let r = this.rates.get(key); if (!r || r.until < now) { r = {n: 0, until: now + 60000}; this.rates.set(key, r); }
    if (++r.n > 20) throw fail(429, '尝试次数较多，请一分钟后重试');
    if (this.rates.size > 10000) for (const [k, v] of this.rates) if (v.until < now) this.rates.delete(k);
  }
  authenticate(req, role, w) {
    const token = cookie(req, role === 'admin' ? 'dcw_admin' : 'dcw_student');
    if (!token || token.length !== 64) return null;
    const session = this.sessions.get(digest(token));
    if (!session || session.role !== role || session.expires < Date.now()) return null;
    if (role === 'student' && (!w || w.disabled || session.student !== w.id || session.generation !== w.generation)) return null;
    return session;
  }
  login(res, role, w, external) {
    const token = secret(), key = digest(token);
    const session = {key, role, student: w?.id, generation: w?.generation, expires: Date.now() + (role === 'admin' || external ? 8 * 3600000 : 7 * 86400000), ...(external ? {external} : {})};
    this.sessions.set(key, session);
    this.persistSessions();
    res.setHeader('Set-Cookie', `${role === 'admin' ? 'dcw_admin' : 'dcw_student'}=${token}; Path=${role === 'admin' ? '/admin/' : `/w/${w.id}/`}; HttpOnly; SameSite=Strict; Max-Age=${role === 'admin' ? 28800 : 604800}${this.secure ? '; Secure' : ''}`);
  }
  persistSessions() {
    const now = Date.now(); for (const [key, s] of this.sessions) if (s.expires < now) this.sessions.delete(key);
    while (this.sessions.size > 10000) this.sessions.delete(this.sessions.keys().next().value);
    this.store.data.sessions = [...this.sessions.values()].slice(-10000); this.store.save();
  }
  revoke(w) {
    delete w.publishToken;
    for (const [key, session] of this.sessions) if (session.student === w.id) this.sessions.delete(key);
    for (const c of this.connections) if (c.student === w.id) c.socket.destroy();
    this.endAssistance(w.id); this.persistSessions();
  }
  endAssistance(id) { this.assistance.delete(id); for (const c of this.connections) if (c.student === id && c.session.role === 'admin') c.socket.destroy(); }
  lease(w, session) { const a = this.assistance.get(w.id); return a && a.expires > Date.now() && (!session || a.owner === session.key); }
  workspace(id) { const w = this.store.student(id); if (!w) throw fail(404, '工作区不存在'); return w; }
  legacyAuth(req, w) {
    const token = cookie(req, 'dct_gate');
    if (w.disabled || !token || !equal(token, w.token)) return null;
    const key = 'legacy:' + digest(token);
    if (!this.sessions.has(key)) this.sessions.set(key, {key, role: 'student', student: w.id, generation: w.generation, expires: Date.now() + 7 * 86400000});
    return this.sessions.get(key);
  }
  safeRow(w, status = {}) { return {id: w.id, name: w.name, shared: !!w.shared, status: w.disabled ? 'disabled' : status.status || 'new', memoryBytes: status.memoryBytes ?? null, diskBytes: status.diskBytes ?? null, connections: [...this.connections].filter(c => c.student === w.id && c.session.role === 'student').length, assisting: !!this.lease(w), live: status.live || null, classId: w.classId || '', className: w.className || ''}; }
  // 往某个工作区的守护进程发一条直播请求。
  //
  // **旧版本的 dct 不认识这几条请求**，它会回一句解析失败——直接把那句话
  // 抛给老师的话，他看到的是「无法解析请求：unknown variant LiveStart」，
  // 而他需要知道的其实是「这个工作区的镜像该换了」。
  async liveRpc(w, request) {
    const answer = await this.driver.rpc(w, request);
    if (answer.Error) {
      const reason = Object.values(answer.Error)[0];
      if (typeof reason === 'string' && /unknown variant|missing field/.test(reason)) {
        throw fail(409, '这个工作区的 dct 版本还不支持直播，请先停止它再启动（会换成新镜像）');
      }
      throw fail(409, typeof reason === 'string' ? reason : '工作区拒绝了这次操作');
    }
    return answer;
  }
  // 这个工作区此刻在不在播。没在播、或者问不出来，一律当没在播——
  // 管理台上「没显示在播」必须意味着「确实没在播」，不能因为一次 RPC 抖动
  // 就把一场真的直播藏起来……所以问不出来时不是静默 null，是让调用方决定。
  async liveStatus(w) {
    const info = (await this.liveRpc(w, 'LiveStatus')).Live;
    if (!info || !info.id) return null;
    return {url: info.url, viewers: info.viewers, staged: info.staged, readiness: info.readiness};
  }
  async state(session) {
    if (session) await this.sso.syncRoster(session);
    const students = []; let running = 0;
    for (const w of this.store.data.students) { const status = await this.driver.status(w); if (status.status === 'running') running++;
      // 在播的话把链接和人数一并带上：老师那一列要一眼看得见「谁在播、
      // 几个人在看」，为此每行再发一次请求不值得。问不出来就当没在播，
      // 一次抖动不该让整张表打不开。
      if (status.status === 'running') { try { status.live = await this.liveStatus(w); } catch { status.live = null; } } if (!session || this.sso.allowed(session, w)) students.push(this.safeRow(w, status)); }
    return {students, capacity: {running, max: this.driver.maxRunning, totalMemory: os.totalmem()}, audit: session?.external ? [] : this.store.data.audit.slice(0, 30), permissions: {manageAccess: !session?.external}};
  }
  async handle(req, res) {
    const u = new URL(req.url, this.origin), route = u.pathname;
    if (route.startsWith('/_agent/publish/')) { if (!this.publishing) throw fail(503, '作品发布尚未配置'); return this.publishing.agent(req, res, u); }
    if (route.startsWith('/sso/') || route.startsWith('/integration/')) return this.sso.handle(req, res, u);
    if (req.method !== 'GET' && req.method !== 'HEAD') this.writeOrigin(req);
    const shared = this.store.student('shared');
    if (shared && (['/', '/login', '/token'].includes(route) || route.startsWith('/_dct/'))) {
      if (shared.disabled) throw fail(403, '此工作区链接已停用，请联系老师');
      if (route === '/login' && req.method === 'POST') {
        this.rate(req, 'legacy'); const data = await body(req);
        if (typeof data.token !== 'string' || data.token.length !== 64 || !equal(data.token, shared.token)) throw fail(401, '链接已失效，请获取新链接');
        res.setHeader('Set-Cookie', `dct_gate=${shared.token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800${this.secure ? '; Secure' : ''}`);
        return json(res, 200, {ok: true});
      }
      const session = this.legacyAuth(req, shared);
      if (!session) { if (route === '/' && req.method === 'GET') return html(res, studentPage('')); throw fail(401, '请使用完整工作区链接'); }
      return this.proxy(req, res, shared, (route === '/_dct/upload' ? '/_dct/projects/current/upload' : route) + u.search, '', session);
    }
    if (route === '/admin') { res.writeHead(302, {Location: '/admin/'}); return res.end(); }
    if (route === '/admin/' && req.method === 'GET') return html(res, fs.readFileSync(here + 'admin.html', 'utf8'));
    if (route === '/admin/api/login' && req.method === 'POST') {
      this.rate(req, 'admin'); const data = await body(req);
      if (!this.store.password(data.password)) throw fail(401, '管理员密码不正确');
      this.login(res, 'admin'); this.store.audit('管理员登录'); return json(res, 200, {ok: true});
    }
    if (route.startsWith('/admin/')) {
      const session = this.authenticate(req, 'admin'); if (!session) throw fail(401, '请先登录管理员账号');
      await this.sso.ensure(session);
      if (route === '/admin/api/logout' && req.method === 'POST') { await body(req); this.sessions.delete(session.key); for (const [id, a] of this.assistance) if (a.owner === session.key) this.endAssistance(id); this.persistSessions(); return json(res, 200, {ok: true}); }
      if (route === '/admin/api/state' && req.method === 'GET') return json(res, 200, await this.state(session));
      if (route === '/admin/api/students' && req.method === 'POST') { if (session.external) throw fail(403, '请在教学平台管理学生账号'); const data = await body(req); return this.store.mutate(async () => { let rows; try { rows = this.store.add(data.names); } catch (e) { throw fail(400, e.message); } return json(res, 201, {students: rows.map(w => this.safeRow(w))}); }); }
      const api = route.match(/^\/admin\/api\/students\/(shared|[a-f0-9]{32})\/([a-z-]+)(.*)$/);
      if (api) return this.adminStudent(req, res, u, this.workspace(api[1]), api[2], api[3], session);
      const view = route.match(/^\/admin\/workspaces\/(shared|[a-f0-9]{32})\/view(\/.*)$/);
      if (view) { const w = this.workspace(view[1]); if (!this.sso.allowed(session, w) || !this.lease(w, session)) throw fail(403, '请先点击“协助学生”'); return this.proxy(req, res, w, view[2] + u.search, route.slice(0, route.length - view[2].length), session); }
      throw fail(404, '页面不存在');
    }
    const student = route.match(/^\/w\/(shared|[a-f0-9]{32})(\/.*)?$/);
    if (!student) throw fail(404, '页面不存在');
    const w = this.workspace(student[1]), prefix = `/w/${w.id}`, sub = student[2] || '';
    if (w.disabled) throw fail(403, '此链接已停用，请联系老师');
    if (sub === '') { res.writeHead(302, {Location: prefix + '/' + u.search}); return res.end(); }
    if (sub === '/login' && req.method === 'POST') { this.rate(req, w.id); const data = await body(req); if (typeof data.token !== 'string' || data.token.length !== 64 || !equal(data.token, w.token)) throw fail(401, '链接已失效，请向老师获取新链接'); this.login(res, 'student', w); return json(res, 200, {ok: true}); }
    const session = this.authenticate(req, 'student', w) || (w.shared ? this.legacyAuth(req, w) : null);
    if (!session) { if (sub === '/' && req.method === 'GET') return html(res, studentPage(prefix)); throw fail(401, '请重新打开老师发给你的完整链接'); }
    await this.sso.ensure(session);
    if (sub === '/finish' && req.method === 'POST' && !w.shared) {
      await body(req);
      return this.store.mutate(async () => {
        if (!this.authenticate(req, 'student', w)) throw fail(403, '链接已停用或重置');
        try { await this.driver.stop(w); } catch (e) { throw fail(409, e.message); }
        this.endAssistance(w.id);
        for (const c of this.connections) if (c.student === w.id) c.socket.destroy();
        this.store.audit('学生保存并结束学习', w);
        return json(res, 200, {ok: true});
      });
    }
    if (sub === '/start' && req.method === 'POST') {
      await body(req); return this.store.mutate(async () => { if (!this.authenticate(req, 'student', w)) throw fail(403, '链接已停用或重置'); try { await this.driver.start(w, this.store.data.students); } catch (e) { throw fail(409, e.message); } this.store.audit('学生打开工作区', w); return json(res, 200, {ok: true}); });
    }
    if (sub === '/' && req.method === 'GET' && (await this.driver.status(w)).status !== 'running') return html(res, u.searchParams.get('finished') === '1' ? finishedPage(prefix) : studentPage(prefix, true));
    return this.proxy(req, res, w, sub + u.search, prefix, session);
  }
  async adminStudent(req, res, u, w, action, suffix, session) {
    if (!this.sso.allowed(session, w)) throw fail(403, '你没有这个学生的管理权限');
    if (session.external && ['link', 'rotate', 'disable', 'enable'].includes(action)) throw fail(403, '请在教学平台管理学生账号');
    if (req.method === 'GET') {
      if (action === 'observe' && !suffix) {
        const list = await this.driver.rpc(w, 'List');
        if (!Array.isArray(list.Sessions)) throw fail(409, '暂时无法读取会话');
        const sessions = list.Sessions.map(s => ({id: s.id, profile: s.profile, state: s.state}));
        const selected = Number(u.searchParams.get('session')) || sessions.find(s => s.state !== 'Stopped')?.id;
        let screen = null;
        if (sessions.some(s => s.id === selected)) screen = (await this.driver.rpc(w, {Screen: {id: selected}})).Screen || null;
        return json(res, 200, {sessions, screen});
      }
      if (action === 'projects') {
        if (suffix && !/^\/(current|[a-f0-9]{32})\/download$/.test(suffix)) throw fail(404, '项目不存在');
        return this.proxy(req, res, w, '/_dct/projects' + suffix, '', session, true);
      }
      // 这个工作区在不在播。答复直接来自它自己的守护进程（LiveStatus），
      // 不在这儿另存一份状态——存了就会跟真相漂，而真相只有那边知道。
      if (action === 'live') return json(res, 200, {live: await this.liveStatus(w)});
      throw fail(404, '接口不存在');
    }
    if (req.method !== 'POST' || suffix) throw fail(404, '接口不存在');
    await body(req);
    return this.store.mutate(async () => {
      if (action === 'link') return json(res, 200, {url: `${this.origin}/w/${w.id}/#t=${w.token}`});
      if (action === 'assist') {
        if (w.disabled || (await this.driver.status(w)).status !== 'running') throw fail(409, '请先启用并启动学生工作区');
        if (!this.lease(w, session)) { this.endAssistance(w.id); this.store.audit('开始协助', w); }
        this.assistance.set(w.id, {owner: session.key, expires: Date.now() + 45000});
        return json(res, 200, {url: `/admin/workspaces/${w.id}/view/`});
      }
      if (action === 'end-assist') { this.endAssistance(w.id); this.store.audit('结束协助', w); return json(res, 200, {ok: true}); }
      if (action === 'rotate') { w.token = secret(); w.generation++; this.revoke(w); this.store.audit('重置学生链接', w); return json(res, 200, {url: `${this.origin}/w/${w.id}/#t=${w.token}`}); }
      if (action === 'disable' || action === 'enable') { w.disabled = action === 'disable'; if (w.disabled) this.revoke(w); this.store.audit(w.disabled ? '停用学生链接' : '启用学生链接', w); return json(res, 200, {ok: true}); }
      if (action === 'live-start' || action === 'live-stop') {
        if ((await this.driver.status(w)).status !== 'running') throw fail(409, '请先启动工作区');
        if (action === 'live-stop') {
          await this.liveRpc(w, 'LiveStop');
          this.store.audit('停止直播工作区', w);
          return json(res, 200, {live: null});
        }
        const list = await this.liveRpc(w, 'List');
        // 只上架还活着的会话。已停止的推过去也是一屏死画面，学生看不出
        // 那是「老师还没开始」还是「这一路本来就没东西」。
        const alive = (list.Sessions || []).filter(x => x.state !== 'Stopped').slice(0, LIVE_MAX_LANES);
        if (!alive.length) throw fail(409, '这个工作区里没有在跑的会话，先让它起一个再直播');
        // 路名会出现在学生那一页上。用学生自己的名字：一屋子人本来就认识
        // 彼此，而「第 1 路」对看的人毫无信息。会话标题和目录仍然不出去。
        const names = alive.map((x, i) => alive.length > 1 ? `${w.name} · ${i + 1}` : w.name);
        await this.liveRpc(w, {LiveStart: {ids: alive.map(x => x.id), names}});
        this.store.audit('开始直播工作区', w);
        return json(res, 200, {live: await this.liveStatus(w)});
      }
      if (action === 'start' || action === 'stop') {
        if (action === 'start' && w.disabled) throw fail(409, '请先启用学生链接');
        try { if (action === 'start') await this.driver.start(w, this.store.data.students); else { await this.driver.stop(w); this.revoke(w); } }
        catch (e) { throw fail(409, e.message); }
        this.store.audit(action === 'start' ? '启动工作区' : '保存并结束工作区', w);
        return json(res, 200, {ok: true});
      }
      throw fail(404, '接口不存在');
    });
  }
  allowed(path) { return path === '/' || path === '/token' || path === '/ws' || /^\/_dct\/projects(?:\/|$)/.test(path); }
  async proxy(req, res, w, target, prefix, session, adminDownload = false) {
    const p = new URL(target, this.origin).pathname;
    if (p.startsWith('/_dct/publish/')) { if (!this.publishing) throw fail(503, '作品发布尚未配置'); return this.publishing.browser(req, res, w, target, prefix, session); }
    if (p === '/_dct/presence' && req.method === 'GET') return json(res, 200, {assisting: !!this.lease(w)});
    if (!this.allowed(p)) throw fail(404, '接口不存在');
    if (req.method !== 'GET') this.writeOrigin(req);
    if (w.disabled && session.role === 'student') throw fail(403, '链接已停用');
    const backend = await this.driver.backend(w);
    // The student owns every process in their container. Its HTTP response is
    // untrusted content, never an administrator-origin document or executable asset.
    if (p === '/') {
      if (req.method !== 'GET') throw fail(405, '不支持此操作');
      if (this.publishing) await this.publishing.provision(w);
      const claim = session.role === 'student' ? `let claiming=false;async function claimLink(){const token=new URLSearchParams(location.hash.slice(1)).get('t');if(!token||claiming)return;claiming=true;try{const response=await fetch(window.DCW_BASE_PATH+'/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token})});if(response.ok){history.replaceState(null,'',location.pathname);location.reload();}else{alert('链接已失效，请联系老师获取新链接');}}finally{claiming=false;}}window.addEventListener('hashchange',claimLink);claimLink();` : '';
      const config = `<script>window.DCW_BASE_PATH=${js(prefix)};window.DCW_STUDENT_NAME=${js(w.name)};window.DCW_MANAGED_STUDENT=${!w.shared && session.role === 'student'};${claim}</script>`;
      return html(res, fs.readFileSync(this.terminalFile, 'utf8').replace('</head>', config + '</head>'));
    }
    const headers = {host: new URL(this.origin).host, cookie: `dct_gate=${backend.token}`, connection: 'close'};
    for (const key of ['content-type', 'content-length', 'origin', 'sec-fetch-site']) if (req.headers[key]) headers[key] = req.headers[key];
    const port = p.startsWith('/_dct/projects') ? backend.apiPort : backend.port;
    return new Promise((resolve, reject) => {
      const upstream = http.request({host: '127.0.0.1', port, path: target, method: req.method, headers}, response => {
        const download = /\/(file|download)$/.test(p) && response.statusCode === 200;
        const responseHeaders = {'cache-control': 'no-store', 'referrer-policy': 'no-referrer', 'x-content-type-options': 'nosniff', 'content-security-policy': "default-src 'none'; sandbox", 'content-type': download ? 'application/octet-stream' : 'application/json; charset=utf-8'};
        if (download) responseHeaders['content-disposition'] = 'attachment; filename="download"';
        const max = download ? 136 * 1024 ** 2 : 8 * 1024 ** 2;
        const length = Number(response.headers['content-length']);
        if (Number.isFinite(length) && length > max) { response.destroy(); return reject(fail(502, '工作区响应过大')); }
        if (Number.isFinite(length) && length >= 0) responseHeaders['content-length'] = length;
        // Do not forward Location, Refresh, Set-Cookie, CSP or other headers from
        // a student-controlled process into the administrator's origin.
        const status = response.statusCode >= 300 && response.statusCode < 400 ? 502 : response.statusCode;
        let bytes = 0;
        response.on('data', chunk => { bytes += chunk.length; if (bytes > max) { response.destroy(); res.destroy(); } });
        res.writeHead(status, responseHeaders); response.pipe(res); response.on('end', resolve);
        response.on('error', reject);
      });
      upstream.setTimeout(120000, () => upstream.destroy(new Error('工作区响应超时')));
      upstream.on('error', reject); req.on('aborted', () => upstream.destroy()); req.pipe(upstream);
    });
  }
  async upgrade(req, socket, head) {
    if (req.headers.origin !== this.origin) throw fail(403, '来源无效');
    const u = new URL(req.url, this.origin);
    const student = u.pathname.match(/^\/w\/(shared|[a-f0-9]{32})\/ws$/);
    const admin = u.pathname.match(/^\/admin\/workspaces\/(shared|[a-f0-9]{32})\/view\/ws$/);
    const legacy = u.pathname === '/ws';
    if (!student && !admin && !legacy) throw fail(404, '接口不存在');
    const w = this.workspace(legacy ? 'shared' : (student || admin)[1]);
    const session = legacy ? this.legacyAuth(req, w) : this.authenticate(req, student ? 'student' : 'admin', w);
    if (!session || (admin && !this.lease(w, session))) throw fail(403, '未授权');
    await this.sso.ensure(session);
    if (!this.sso.allowed(session, w)) throw fail(403, '你没有这个工作区的权限');
    const b = await this.driver.backend(w);
    const headers = {host: new URL(this.origin).host, cookie: `dct_gate=${b.token}`, origin: this.origin, connection: 'Upgrade', upgrade: 'websocket'};
    for (const key of ['sec-websocket-key', 'sec-websocket-version', 'sec-websocket-protocol']) if (req.headers[key]) headers[key] = req.headers[key];
    const upstream = http.request({host: '127.0.0.1', port: b.port, path: '/ws' + u.search, headers});
    upstream.on('error', () => socket.destroy()); upstream.on('response', () => socket.destroy());
    upstream.setTimeout(10000, () => upstream.destroy());
    upstream.on('upgrade', (response, up, upHead) => {
      // Authorization may have been revoked while connecting to the backend.
      if (!this.sessions.has(session.key) || w.disabled || !this.sso.allowed(session, w) || (admin && !this.lease(w, session))) { up.destroy(); return socket.destroy(); }
      up.setTimeout(0); socket.setTimeout(0);
      const lines = Object.entries(response.headers).filter(([k]) => k !== 'set-cookie').map(([k, v]) => `${k}: ${v}`);
      socket.write(`HTTP/1.1 101 Switching Protocols\r\n${lines.join('\r\n')}\r\n\r\n`);
      if (upHead.length) socket.write(upHead); if (head.length) up.write(head);
      const connection = {student: w.id, session, socket}; this.connections.add(connection);
      socket.on('close', () => { this.connections.delete(connection); up.destroy(); });
      socket.on('error', () => up.destroy()); up.on('error', () => socket.destroy()); up.on('close', () => socket.destroy());
      socket.pipe(up); up.pipe(socket);
    });
    upstream.end();
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const store = new Store(process.env.CLASSROOM_STATE || '/var/lib/dcw-classroom', {password: process.env.CLASSROOM_ADMIN_PASSWORD, legacy: process.env.CLASSROOM_LEGACY_CONTAINER});
  const driver = new DockerDriver({image: process.env.CLASSROOM_IMAGE, maxRunning: Number(process.env.CLASSROOM_MAX_RUNNING) || undefined});
  const legacy = store.student('shared');
  if (legacy && !legacy.legacyReady) {
    // Adopt the existing public link once. Later link rotations are managed here;
    // the backend gate credential remains private and does not need restarting.
    legacy.token = (await driver.backend(legacy)).token;
    legacy.legacyReady = true;
    store.save();
  }
  const origin = process.env.CLASSROOM_ORIGIN || 'https://dataclue.cn';
  const issuers = process.env.CLASSROOM_SSO_ISSUERS_FILE ? JSON.parse(fs.readFileSync(process.env.CLASSROOM_SSO_ISSUERS_FILE, 'utf8')) : {};
  const publishing = process.env.CLASSROOM_PUBLISH_CONFIG ? JSON.parse(fs.readFileSync(process.env.CLASSROOM_PUBLISH_CONFIG, 'utf8')) : undefined;
  const app = new Classroom({store, driver, origin, issuers, publishing, secure: origin.startsWith('https:')});
  if (app.publishing) app.publishing.server.listen(Number(process.env.CLASSROOM_WORKS_PORT || 17701), '127.0.0.1');
  app.server.listen(Number(process.env.CLASSROOM_PORT || 17700), '127.0.0.1', () => console.log('Classroom manager listening on loopback'));
  process.on('SIGTERM', () => app.close().then(() => process.exit(0)));
}

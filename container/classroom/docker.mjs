import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';

const exec = promisify(execFile), GiB = 1024 ** 3;
export function localRequest(port, token, route, body) {
  return new Promise((resolve, reject) => {
    const bytes = body === undefined ? null : Buffer.from(JSON.stringify(body));
    const req = http.request({host: '127.0.0.1', port, path: route, method: bytes ? 'POST' : 'GET', headers: {Cookie: `dct_gate=${token}`, Connection: 'close', ...(bytes ? {'Content-Type': 'application/json', 'Content-Length': bytes.length} : {})}}, res => {
      let size = 0, chunks = [];
      res.on('data', chunk => { size += chunk.length; if (size > 8 * 1024 ** 2) res.destroy(new Error('响应过大')); else chunks.push(chunk); });
      res.on('error', reject);
      res.on('end', () => { try { const value = JSON.parse(Buffer.concat(chunks)); if (res.statusCode >= 400) reject(new Error(value.error || '工作区操作失败')); else resolve(value); } catch { reject(new Error('工作区响应无效')); } });
    });
    req.setTimeout(120000, () => req.destroy(new Error('工作区响应超时，文件保留，请重试')));
    req.on('error', reject); req.end(bytes);
  });
}
export class DockerDriver {
  // `memory` 是每个工作区的内存上限，`maxRunning` 是同时能跑几个。
  //
  // 默认值按「一台机器留 1 GB 给自己，其余按每人 3 GB 分」算。3 GB 是很宽
  // 的余量——实测一个 agent 会话吃 150–320 MB，加上学生跑起来的 dev server
  // 也就 400–500 MB。真正卡住的是名额：一台 8 GB 的机器只分得出 2 个，而
  // 共享工作区自己就占一个。所以两个数都能用环境变量压过去，调完重启
  // 服务即可，不用改代码、不用发版。
  constructor({image = 'dc-workspace:0.2.14-classroom', maxRunning, memory, command = exec} = {}) {
    this.image = image;
    this.memory = memory || process.env.CLASSROOM_MEMORY || '3g';
    this.maxRunning = maxRunning || Number(process.env.CLASSROOM_MAX_RUNNING) || Math.max(1, Math.floor((os.totalmem() - GiB) / (3 * GiB)));
    this.command = command;
    this.usage = new Map();
  }
  name(w) {
    if (w.shared && w.containerName === 'dcw-workspace-1') return w.containerName;
    if (!/^[a-f0-9]{32}$/.test(w.id)) throw new Error('工作区编号无效');
    return `dcw-student-${w.id}`;
  }
  async docker(args, timeout = 15000) { return (await this.command('docker', args, {timeout, maxBuffer: 8 * 1024 ** 2})).stdout; }
  async inspect(w) {
    let data;
    try { data = JSON.parse(await this.docker(['inspect', this.name(w)]))[0]; }
    catch (error) { if (/No such (object|container)/i.test(error.stderr || '')) return null; throw new Error('无法读取工作区状态'); }
    if (!w.shared && (data.Config.Labels?.['dcw.classroom'] !== '1' || data.Config.Labels?.['dcw.student'] !== w.id)) throw new Error('工作区标识不匹配，未执行操作');
    return data;
  }
  async status(w) {
    const info = await this.inspect(w);
    let memoryBytes = null, diskBytes = null;
    if (info) {
      if (info.State.Running && process.platform === 'linux') {
        try {
          const cgroup = (await fs.readFile(`/proc/${info.State.Pid}/cgroup`, 'utf8')).split('\n').find(s => s.startsWith('0::'))?.slice(3);
          if (cgroup) memoryBytes = Number(await fs.readFile(path.join('/sys/fs/cgroup', cgroup, 'memory.current'), 'utf8'));
        } catch { /* Explicitly unknown on hosts without readable cgroup v2. */ }
      }
      const cached = this.usage.get(w.id);
      if (cached && cached.until > Date.now()) diskBytes = cached.bytes;
      else {
        try {
          const sources = info.Mounts.filter(m => ['/home/dc/.dct', '/home/dc/work', '/home/dc/.claude', '/home/dc/.codex'].includes(m.Destination)).map(m => m.Source);
          if (sources.length) {
            const result = await this.command('du', ['-sb', '--', ...sources], {timeout: 5000, maxBuffer: 16384});
            diskBytes = result.stdout.trim().split('\n').reduce((sum, line) => sum + Number(line.split(/\s/)[0]), 0);
          }
        } catch { /* Unknown is different from zero usage. */ }
        this.usage.set(w.id, {bytes: diskBytes, until: Date.now() + 30000});
      }
    }
    return {status: info?.State.Running ? 'running' : info ? 'stopped' : 'new', port: Number(info?.NetworkSettings.Ports?.['7681/tcp']?.[0]?.HostPort) || null, memoryBytes, diskBytes};
  }
  async backend(w) {
    const s = await this.status(w);
    if (s.status !== 'running') throw new Error('工作区尚未启动');
    let token = w.backendToken;
    if (!token) {
      const out = await this.docker(['exec', this.name(w), 'dct', 'gate', '--link', '--url', 'http://localhost']);
      token = out.match(/#t=([a-f0-9]{64})/)?.[1];
      if (!token) throw new Error('工作区尚未就绪，请稍后重试');
      w.backendToken = token;
    }
    return {port: s.port, apiPort: w.apiPort || s.port, token};
  }
  async start(w, all) {
    const current = await this.inspect(w);
    if (current?.State.Running) return this.backend(w);
    // Include managed workspaces outside this process's state (e.g. another
    // staging manager) and the existing shared classroom in the host budget.
    const names = (await this.docker(['ps', '--format', '{{.Names}}'])).trim().split('\n');
    const count = names.filter(n => n === 'dcw-workspace-1' || /^dcw-student-[a-f0-9]{32}$/.test(n)).length;
    if (count >= this.maxRunning) throw new Error('当前运行名额已满，请联系老师结束其他工作区后再试');
    if (current) await this.docker(['start', this.name(w)], 60000);
    else {
      const name = this.name(w), mounts = [];
      for (const [suffix, destination] of [['state', '/home/dc/.dct'], ['claude', '/home/dc/.claude'], ['codex', '/home/dc/.codex'], ['work', '/home/dc/work']]) {
        const volume = `${name}-${suffix}`;
        await this.docker(['volume', 'create', '--label', 'dcw.classroom=1', volume]);
        mounts.push('--mount', `type=volume,source=${volume},target=${destination}`);
      }
      await this.docker(['run', '-d', '--name', name, '--label', 'dcw.classroom=1', '--label', `dcw.student=${w.id}`, '--restart', 'unless-stopped', '--memory', this.memory, '--memory-swap', this.memory, '--pids-limit', '512', '--security-opt', 'no-new-privileges:true', '--cap-drop', 'ALL', '--log-opt', 'max-size=10m', '--log-opt', 'max-file=3', '--env', 'DCW_STUDENT=1', '--env', 'DCW_PUBLIC_URL=https://dataclue.cn', '-p', '127.0.0.1::7681', ...mounts, this.image], 120000);
    }
    delete w.backendToken;
    for (let n = 0; n < 40; n++) {
      try { const backend = await this.backend(w); await localRequest(backend.apiPort, backend.token, '/_dct/projects'); return backend; }
      catch { delete w.backendToken; await new Promise(r => setTimeout(r, 250)); }
    }
    throw new Error('工作区正在启动，请稍后重试');
  }
  // 停一个工作区：先把每个项目归档，再停容器。**顺序不能反**——反过来
  // 就是「停完了才发现归档失败」，而那时候会话已经没了。
  //
  // 共享工作区以前在这里是硬拒的（理由是它背着教师端那条旧链接）。但在
  // 一台只有两个名额的机器上，那条规矩的实际后果是：共享的那个白占一个，
  // 全班只剩一个名额能用。所以改成允许停——代价（教师端链接在它停着的
  // 时候打不开）写在按钮的确认框里，交给点的人判断，而不是替他决定。
  async stop(w) {
    if ((await this.status(w)).status !== 'running') return;
    const b = await this.backend(w);
    const list = await localRequest(b.apiPort, b.token, '/_dct/projects');
    for (const project of list.projects) await localRequest(b.apiPort, b.token, `/_dct/projects/${encodeURIComponent(project.id)}/end`, {});
    await this.docker(['stop', '--time', '20', this.name(w)], 30000);
  }
  async rpc(w, request) {
    const info = await this.inspect(w);
    if (!info?.State.Running) throw new Error('工作区尚未启动');
    const mount = info.Mounts.find(m => m.Destination === '/home/dc/.dct');
    if (!mount) throw new Error('工作区存储配置无效');
    return new Promise((resolve, reject) => {
      const socket = net.connect(mount.Source + '/daemon.sock');
      socket.setEncoding('utf8');
      let response = '', settled = false;
      const fail = () => { if (!settled) { settled = true; reject(new Error('无法读取运行会话，请稍后重试')); } socket.destroy(); };
      socket.setTimeout(5000, fail); socket.on('error', fail); socket.on('end', fail);
      socket.on('connect', () => socket.write(JSON.stringify(request) + '\n'));
      socket.on('data', data => {
        response += data;
        if (response.length > 8 * 1024 ** 2) return fail();
        if (response.includes('\n')) {
          try { const result = JSON.parse(response.split('\n')[0]); settled = true; resolve(result); socket.destroy(); }
          catch { fail(); }
        }
      });
    });
  }
}

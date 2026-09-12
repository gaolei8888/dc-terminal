import fs from 'node:fs';
import path from 'node:path';
import {randomBytes, scryptSync, timingSafeEqual, createHash} from 'node:crypto';

export const secret = () => randomBytes(32).toString('hex');
export const digest = value => createHash('sha256').update(value).digest('hex');
export function equal(a, b) {
  const x = Buffer.from(String(a)), y = Buffer.from(String(b));
  return x.length === y.length && timingSafeEqual(x, y);
}
export class Store {
  constructor(dir, {password, legacy} = {}) {
    fs.mkdirSync(dir, {recursive: true, mode: 0o700});
    this.file = path.join(dir, 'classroom.json');
    this.dir = dir;
    if (fs.existsSync(this.file)) this.data = JSON.parse(fs.readFileSync(this.file, 'utf8'));
    else {
      const initial = password || randomBytes(18).toString('base64url'), salt = secret();
      this.data = {salt, passwordHash: scryptSync(initial, salt, 64).toString('hex'), students: [], audit: []};
      if (legacy) this.data.students.push({id: 'shared', name: '原有共享工作区', shared: true, containerName: legacy, apiPort: 17681, disabled: false, token: secret(), generation: 1});
      this.save();
      if (!password) fs.writeFileSync(path.join(dir, 'admin-password'), initial + '\n', {mode: 0o600, flag: 'wx'});
    }
    this.queue = Promise.resolve();
  }
  password(value) {
    if (typeof value !== 'string' || value.length > 1024) return false;
    return equal(scryptSync(value, this.data.salt, 64).toString('hex'), this.data.passwordHash);
  }
  save() {
    const tmp = this.file + '.' + secret() + '.tmp';
    const fd = fs.openSync(tmp, 'wx', 0o600);
    try { fs.writeFileSync(fd, JSON.stringify(this.data)); fs.fsyncSync(fd); }
    finally { fs.closeSync(fd); }
    fs.renameSync(tmp, this.file);
    const directory = fs.openSync(this.dir, 'r');
    try { fs.fsyncSync(directory); } finally { fs.closeSync(directory); }
  }
  mutate(fn) {
    const operation = this.queue.then(fn);
    this.queue = operation.catch(() => {});
    return operation;
  }
  audit(action, student) {
    this.data.audit.unshift({time: Date.now(), action, name: student?.name || '管理员'});
    this.data.audit = this.data.audit.slice(0, 500);
    this.save();
  }
  student(id) { return this.data.students.find(w => w.id === id); }
  add(names) {
    if (!Array.isArray(names) || !names.length || names.length > 50 || this.data.students.length + names.length > 200) throw new Error('每次添加 1–50 名学生，总数最多 200 名');
    if (names.some(n => typeof n !== 'string' || !n.trim() || n.trim().length > 80 || /[\x00-\x1f\x7f]/.test(n))) throw new Error('学生姓名需要 1–80 个字符');
    const rows = names.map(name => ({id: randomBytes(16).toString('hex'), name: name.trim(), token: secret(), generation: 1, disabled: false, shared: false}));
    this.data.students.push(...rows);
    this.audit(`添加 ${rows.length} 名学生`);
    return rows;
  }
}

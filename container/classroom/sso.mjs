import {secret,digest,equal} from './store.mjs';
const fail=(status,message)=>Object.assign(new Error(message),{status});
const send=(res,status,data)=>{res.writeHead(status,{'Content-Type':'application/json; charset=utf-8','Cache-Control':'no-store','Referrer-Policy':'no-referrer','X-Content-Type-Options':'nosniff'});res.end(JSON.stringify(data));};
const identityKey=x=>JSON.stringify([x.tenant,x.subject]);
function identity(x){
 if(!x||typeof x.tenant!=='string'||!x.tenant.trim()||x.tenant.length>128||/[\x00-\x1f\x7f]/.test(x.tenant)||typeof x.subject!=='string'||!/^\d{1,20}$/.test(x.subject))throw fail(400,'学校或用户身份无效');
 return {tenant:x.tenant,subject:x.subject};
}
async function body(req){let chunks=[],size=0;if(!String(req.headers['content-type']).startsWith('application/json'))throw fail(400,'请求格式无效');for await(const chunk of req){size+=chunk.length;if(size>16384)throw fail(413,'请求过大');chunks.push(chunk);}try{return JSON.parse(Buffer.concat(chunks));}catch{throw fail(400,'请求格式无效');}}
const cookie=req=>(req.headers.cookie||'').split(';').map(s=>s.trim()).find(s=>s.startsWith('dcw_sso_state='))?.slice(14);
export class Federation {
 constructor(app,issuers={},allowHttp=false){this.app=app;this.issuers=issuers;this.cache=new Map();this.pending=new Map();for(const [name,c] of Object.entries(issuers)){if(!['classroom','classeditor'].includes(name)||typeof c.key!=='string'||!c.key)throw Error('Invalid SSO issuer configuration');for(const field of ['launchUrl','permissionsUrl']){const u=new URL(c[field]);if((u.protocol!=='https:'&&!allowHttp)||u.username||u.password)throw Error('SSO issuer requires HTTPS');}}}
 async permissions(external){
  const c=this.issuers[external.issuer];if(!c)throw fail(401,'教学平台登录已停用');
  const key=external.issuer+identityKey(external),cached=this.cache.get(key);if(cached&&cached.until>Date.now())return cached.grant;
  if(this.pending.has(key))return this.pending.get(key);
  const operation=(async()=>{
   let response,data;try{response=await fetch(c.permissionsUrl,{method:'POST',redirect:'error',headers:{Authorization:'Bearer '+c.key,'Content-Type':'application/json'},body:JSON.stringify(identity(external)),signal:AbortSignal.timeout(5000)});if(!response.ok)throw Error();let chunks=[],size=0;for await(const chunk of response.body){size+=chunk.length;if(size>2*1024**2)throw Error();chunks.push(Buffer.from(chunk));}data=JSON.parse(Buffer.concat(chunks).toString('utf8'));}catch{throw fail(503,'暂时无法确认教学平台权限，请稍后重试');}
   if(data.active!==true)throw fail(401,'账号权限已失效，请返回教学平台登录');
   if(external.issuer==='classroom'&&data.role!=='student'||external.issuer==='classeditor'&&!['teacher','platform_admin'].includes(data.role))throw fail(403,'教学平台角色无效');
   if(!['student','teacher','platform_admin'].includes(data.role))throw fail(403,'教学平台角色无效');
   const rows=data.role==='teacher'?data.students:[];if(data.role==='teacher'&&(!Array.isArray(rows)||rows.length>2000))throw fail(503,'班级名单暂时无法读取');
   const students=(rows||[]).map(row=>{const id=identity(row);if(id.tenant!==external.tenant)throw fail(403,'班级名单不能跨学校');return {...id,name:typeof row.name==='string'?row.name.slice(0,80):'学生 '+id.subject,classId:String(row.classId||''),className:String(row.className||'').slice(0,100)};});
   const grant={role:data.role,students,allowed:new Set(students.map(identityKey))};this.cache.set(key,{until:Date.now()+15000,grant});if(this.cache.size>5000)for(const[k,v]of this.cache)if(v.until<Date.now())this.cache.delete(k);return grant;
  })();this.pending.set(key,operation);try{return await operation;}finally{this.pending.delete(key);}
 }
 bind(person){const id=identity(person);let w=this.app.store.data.students.find(w=>w.externalIdentity&&identityKey(w.externalIdentity)===identityKey(id));if(!w){if(this.app.store.data.students.length>=2000)throw fail(409,'工作区数量已达上限，请联系管理员');w={id:secret().slice(0,32),name:String(person.name||'学生 '+id.subject).slice(0,80),token:secret(),generation:1,disabled:false,shared:false,externalIdentity:id};this.app.store.data.students.push(w);}return w;}
 allowed(session,w){if(!session.external)return true;const cached=this.cache.get(session.external.issuer+identityKey(session.external));if(!cached||cached.until<=Date.now())return false;const grant=cached.grant;if(session.role==='student')return session.student===w.id;if(grant.role==='platform_admin')return true;return !!w.externalIdentity&&grant.allowed.has(identityKey(w.externalIdentity));}
 async ensure(session){if(!session?.external)return;let grant;try{grant=await this.permissions(session.external);}catch(e){for(const c of this.app.connections)if(c.session.key===session.key)c.socket.destroy();for(const[id,a]of this.app.assistance)if(a.owner===session.key)this.app.endAssistance(id);throw e;}
  if((session.role==='student')!==(grant.role==='student'))throw fail(403,'身份角色已改变，请重新登录');
  for(const c of this.app.connections)if(c.session.key===session.key&&!this.allowed(session,this.app.workspace(c.student)))c.socket.destroy();
  for(const[id,a]of this.app.assistance)if(a.owner===session.key&&!this.allowed(session,this.app.workspace(id)))this.app.endAssistance(id);
  return grant;
 }
 async syncRoster(session){const grant=await this.ensure(session);if(!grant||grant.role!=='teacher')return;await this.app.store.mutate(()=>{let changed=false;for(const row of grant.students){const before=this.app.store.data.students.length,w=this.bind(row);if(before!==this.app.store.data.students.length)changed=true;if(w.classId!==row.classId||w.className!==row.className){w.classId=row.classId;w.className=row.className;changed=true;}}if(changed)this.app.store.save();});}
 async handle(req,res,u){const route=u.pathname;
  if(route==='/sso/start'&&req.method==='GET'){const issuer=u.searchParams.get('issuer'),c=this.issuers[issuer];if(!c)throw fail(400,'教学平台登录尚未配置');const state=secret(),target=new URL(c.launchUrl);target.searchParams.set('state',state);res.writeHead(302,{Location:target.href,'Cache-Control':'no-store','Referrer-Policy':'no-referrer','Set-Cookie':`dcw_sso_state=${state}; Path=/sso/; HttpOnly; SameSite=Lax; Max-Age=120${this.app.secure?'; Secure':''}`});return res.end();}
  if(route==='/integration/tickets'&&req.method==='POST'){
   const issuer=req.headers['x-dct-issuer'],c=this.issuers[issuer];if(!c||!equal(req.headers.authorization,'Bearer '+c.key))throw fail(401,'教学平台认证失败');const data=await body(req),external={...identity(data),issuer};if(typeof data.state!=='string'||!/^[a-f0-9]{64}$/.test(data.state)||typeof data.name!=='string'||!data.name.trim()||data.name.length>80)throw fail(400,'登录请求无效');await this.permissions(external);
   return this.app.store.mutate(()=>{const rows=(this.app.store.data.ssoTickets||[]).filter(t=>t.expires>Date.now());if(rows.length>=2000)throw fail(429,'登录请求较多，请稍后重试');const token=secret();rows.push({key:digest(token),state:digest(data.state),external,name:data.name,expires:Date.now()+120000});this.app.store.data.ssoTickets=rows;this.app.store.save();return send(res,200,{url:this.app.origin+'/sso/#ticket='+token});});
  }
  if(route==='/sso/'&&req.method==='GET'){res.writeHead(200,{'Content-Type':'text/html; charset=utf-8','Cache-Control':'no-store','Referrer-Policy':'no-referrer','Content-Security-Policy':"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'"});return res.end(`<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>打开学习工作区</title><body style="font:16px/1.7 system-ui;padding:10vh 8vw"><h1>正在打开工作区…</h1><p id="message"></p><script>(async()=>{const ticket=new URLSearchParams(location.hash.slice(1)).get('ticket');history.replaceState(null,'','/sso/');try{const r=await fetch('/sso/consume',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({ticket})});const d=await r.json();if(!r.ok)throw Error(d.error);location.replace(d.url);}catch(e){document.querySelector('h1').textContent='暂时无法打开';document.querySelector('#message').textContent=e.message+'。请回到教学平台重新点击工作区。';}})();</script>`);}
  if(route==='/sso/consume'&&req.method==='POST'){this.app.writeOrigin(req);const data=await body(req),state=cookie(req);if(typeof data.ticket!=='string'||!/^[a-f0-9]{64}$/.test(data.ticket)||!state||!/^[a-f0-9]{64}$/.test(state))throw fail(401,'登录凭证已失效');return this.app.store.mutate(async()=>{const rows=this.app.store.data.ssoTickets||[],t=rows.find(t=>equal(t.key,digest(data.ticket)));if(!t||t.expires<=Date.now()||!equal(t.state,digest(state)))throw fail(401,'登录凭证已失效');const grant=await this.permissions(t.external),w=grant.role==='student'?this.bind({...t.external,name:t.name}):null;if(w?.disabled)throw fail(403,'工作区已停用，请联系老师');this.app.store.data.ssoTickets=rows.filter(row=>row!==t);this.app.store.save();this.app.login(res,grant.role==='student'?'student':'admin',w,t.external);return send(res,200,{url:w?'/w/'+w.id+'/':'/admin/'});});}
  throw fail(404,'接口不存在');
 }
}

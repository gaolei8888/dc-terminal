import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
import {secret,digest,equal} from './store.mjs';
const exec=promisify(execFile),here=fileURLToPath(new URL('.',import.meta.url));
const MAX=100*1024**2,MAX_BODY=145*1024**2;
const fail=(status,message)=>Object.assign(new Error(message),{status});
const send=(res,status,data)=>{res.writeHead(status,{'Content-Type':'application/json; charset=utf-8','Cache-Control':'no-store','X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer'});res.end(JSON.stringify(data));};
const TYPES={html:'text/html; charset=utf-8',css:'text/css; charset=utf-8',js:'text/javascript; charset=utf-8',mjs:'text/javascript; charset=utf-8',json:'application/json',txt:'text/plain; charset=utf-8',svg:'image/svg+xml',png:'image/png',jpg:'image/jpeg',jpeg:'image/jpeg',gif:'image/gif',webp:'image/webp',avif:'image/avif',ico:'image/x-icon',woff:'font/woff',woff2:'font/woff2',ttf:'font/ttf',otf:'font/otf',wasm:'application/wasm',glb:'model/gltf-binary',gltf:'model/gltf+json',bin:'application/octet-stream',mp3:'audio/mpeg',mp4:'video/mp4',webm:'video/webm',ogg:'audio/ogg',wav:'audio/wav',pdf:'application/pdf'};
export function validatePath(name){
 if(typeof name!=='string'||!name||name.length>512||/[\\\x00-\x1f\x7f?#%]/.test(name))throw fail(400,'文件路径无效');
 const parts=name.split('/');if(parts.some(p=>!p||p.startsWith('.')||p==='node_modules'||p.length>200)||/^(package(?:-lock)?|credentials?|secrets?|config|tsconfig)(?:\.|$)/i.test(parts.at(-1)))throw fail(400,'不能发布配置、隐藏文件或目录外文件');
 const ext=parts.at(-1).split('.').at(-1).toLowerCase();if(!TYPES[ext])throw fail(400,'不支持的静态文件类型：'+parts.at(-1));return ext;
}
async function body(req,max=16384){if(!String(req.headers['content-type']||'').startsWith('application/json'))throw fail(400,'需要 JSON 请求');let n=0,chunks=[];const timer=setTimeout(()=>req.destroy(),10000);try{for await(const c of req){n+=c.length;if(n>max)throw fail(413,'发布内容太大');chunks.push(c);}}finally{clearTimeout(timer);}let data;try{data=JSON.parse(Buffer.concat(chunks));}catch{throw fail(400,'请求格式无效');}if(!data||Array.isArray(data)||typeof data!=='object')throw fail(400,'请求格式无效');return data;}
export class Publishing {
 constructor(app,{dir=path.join(app.store.dir,'publishing'),origin='https://works.dataclue.cn',siteOrigin='https://ai.tzspace.cn',serviceKey,allowHttp=false}={}){
  this.app=app;this.dir=dir;this.origin=new URL(origin).origin;this.siteOrigin=new URL(siteOrigin).origin;this.serviceKey=serviceKey;this.cache=new Map();this.records=new Map();this.uploading=new Set();this.uploadRates=new Map();this.busy=false;this.provisioned=new Map();
  for(const u of [new URL(origin),new URL(siteOrigin)])if((u.protocol!=='https:'&&!allowHttp)||u.username||u.password||u.pathname!=='/'||u.search||u.hash)throw Error('Publishing requires HTTPS origins');
  if(this.origin===app.origin||this.origin===this.siteOrigin)throw Error('Student files require a separate origin');
  fs.mkdirSync(dir,{recursive:true,mode:0o700});
  for(const name of fs.readdirSync(dir)){if(!/^[a-f0-9]{32}$/.test(name))continue;const meta=JSON.parse(fs.readFileSync(path.join(dir,name,'meta.json')));this.records.set(name,meta);}
  this.server=http.createServer((req,res)=>this.serve(req,res).catch(e=>{if(!res.headersSent)send(res,e.status||500,{error:e.status?e.message:'作品暂时无法打开'});else res.destroy();}));
 }
 enabled(){if(!this.serviceKey)throw fail(503,'作品发布尚未配置，请联系老师');}
 save(r){const dest=path.join(this.dir,r.version,'meta.json'),tmp=dest+'.'+secret();fs.writeFileSync(tmp,JSON.stringify(r),{mode:0o600,flag:'wx'});fs.renameSync(tmp,dest);}
 record(w,id){if(!/^[a-f0-9]{32}$/.test(id||''))throw fail(404,'没有这个发布版本');const r=this.records.get(id);if(!r||r.workspaceId!==w.id)throw fail(404,'没有这个发布版本');return r;}
 async active(w){if(w.disabled)throw fail(403,'工作区已停用');if(w.externalIdentity)await this.app.sso.permissions({...w.externalIdentity,issuer:'classroom'});}
 view(r){return {version:r.version,title:r.title,summary:r.summary,dir:r.outputDir||'',previewUrl:this.origin+'/preview/'+r.previewToken+'/index.html',confirmUrl:this.app.origin+'/w/'+r.workspaceId+'/?publish='+r.version,status:r.status,files:r.files.length,bytes:r.bytes,...(r.projectUrl?{projectUrl:r.projectUrl}:{})};}
 async agent(req,res,u){
  this.enabled();const auth=req.headers.authorization;const w=this.app.store.data.students.find(w=>w.publishToken&&equal(auth,'Bearer '+w.publishToken));if(!w)throw fail(401,'工作区发布凭据无效，请重新打开工作区');await this.active(w);
  if(u.pathname==='/_agent/publish/status'&&req.method==='GET')return send(res,200,await this.status(w,u.searchParams.get('version')));
  if(u.pathname!=='/_agent/publish/prepare'||req.method!=='POST')throw fail(404,'接口不存在');
  if(!String(req.headers['content-type']||'').startsWith('application/json'))throw fail(400,'需要 JSON 请求');
  const now=Date.now(),recent=(this.uploadRates.get(w.id)||[]).filter(t=>t>now-60000);
  if(this.uploading.has(w.id)||this.uploading.size>=2||recent.length>=6)throw fail(429,'上传较多，请稍后再试');
  this.uploadRates.set(w.id,[...recent,now]);this.uploading.add(w.id);
  const temp=path.join(this.dir,'.upload-'+secret()),fd=fs.openSync(temp,'wx',0o600);let size=0,locked=false;
  const timeout=setTimeout(()=>req.destroy(),120000);
  try{for await(const chunk of req){size+=chunk.length;if(size>MAX_BODY)throw fail(413,'发布内容太大');fs.writeSync(fd,chunk);}clearTimeout(timeout);
   if(this.busy)throw fail(429,'正在准备另一个预览，请稍后再试');this.busy=true;locked=true;
   let data;try{data=JSON.parse(fs.readFileSync(temp));}catch{throw fail(400,'请求格式无效');}if(!data||typeof data!=='object'||Array.isArray(data))throw fail(400,'请求格式无效');
   return send(res,201,await this.prepare(w,data));
  }finally{clearTimeout(timeout);fs.closeSync(fd);fs.rmSync(temp,{force:true});this.uploading.delete(w.id);if(locked)this.busy=false;}
 }
 async prepare(w,data){
  this.enabled();await this.active(w);
  if(typeof data.projectKey!=='string'||!data.projectKey.trim()||data.projectKey.length>128||/[\x00-\x1f\x7f]/.test(data.projectKey)||typeof data.title!=='string'||!data.title.trim()||data.title.length>100||typeof(data.summary||'')!=='string'||(data.summary||'').length>500)throw fail(400,'请填写项目名称和有效的项目标识');
  if(data.outputDir!==undefined&&(typeof data.outputDir!=='string'||data.outputDir.length>512||/[\x00-\x1f\x7f]/.test(data.outputDir)))throw fail(400,'静态目录名称无效');
  if(!Array.isArray(data.files)||!data.files.length||data.files.length>5000)throw fail(400,'静态文件数量需要为 1–5000');
  const seen=new Set(),files=[];let bytes=0;
  for(const f of data.files){if(!f||typeof f!=='object')throw fail(400,'文件格式无效');const ext=validatePath(f.path),lower=f.path.toLowerCase();if(seen.has(lower))throw fail(400,'存在重复或大小写冲突的文件');seen.add(lower);if(typeof f.data!=='string'||f.data.length>Math.ceil(MAX/3)*4)throw fail(400,'文件编码无效');const content=Buffer.from(f.data,'base64');if(content.toString('base64')!==f.data)throw fail(400,'文件编码无效');if(/-----BEGIN [A-Z ]*PRIVATE KEY-----|\bsk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}/.test(content.toString('latin1')))throw fail(400,'静态文件含凭据，请移除后重新生成');bytes+=content.length;if(bytes>MAX)throw fail(413,'静态产物超过 100 MiB');files.push({path:f.path,ext,content});}
  if(bytes===0)throw fail(400,'静态产物不能为空');
  if(!files.some(f=>f.path==='index.html'))throw fail(400,'请选择包含 index.html 的构建目录');
  const existing=[...this.records.values()],manifestBytes=Buffer.byteLength(JSON.stringify(files.map(f=>({path:f.path,ext:f.ext,size:f.content.length}))))+4096;const metadataUsed=rows=>rows.reduce((n,r)=>n+(r.metadataBytes||Buffer.byteLength(JSON.stringify(r))),0);if(metadataUsed(existing)+manifestBytes>64*1024**2||metadataUsed(existing.filter(r=>r.workspaceId===w.id))+manifestBytes>8*1024**2)throw fail(409,'发布版本数量已达上限，请联系老师清理旧预览');if(existing.filter(r=>r.workspaceId===w.id).length>=30||existing.length>=2000||existing.reduce((n,r)=>n+r.bytes,0)+bytes>4*1024**3||existing.filter(r=>r.workspaceId===w.id).reduce((n,r)=>n+r.bytes,0)+bytes>512*1024**2)throw fail(409,'发布空间已满，请联系老师清理旧预览');
  const version=secret().slice(0,32),staging=path.join(this.dir,'.prepare-'+version),dest=path.join(this.dir,version);
  fs.mkdirSync(staging,{mode:0o700});
  try{fs.mkdirSync(path.join(staging,'files'));for(const f of files){const target=path.join(staging,'files',f.path);fs.mkdirSync(path.dirname(target),{recursive:true,mode:0o700});fs.writeFileSync(target,f.content,{mode:0o600,flag:'wx'});}
   const r={workspaceId:w.id,projectKey:data.projectKey,version,title:data.title.trim(),summary:data.summary||'',owner:String(w.name||'学生').slice(0,80),outputDir:data.outputDir||'',files:files.map(f=>({path:f.path,ext:f.ext,size:f.content.length})),bytes,metadataBytes:manifestBytes,previewToken:secret(),previewExpires:Date.now()+7*86400000,status:'prepared',createdAt:Date.now()};
   fs.writeFileSync(path.join(staging,'meta.json'),JSON.stringify(r),{mode:0o600,flag:'wx'});fs.renameSync(staging,dest);this.records.set(version,r);this.app.store.audit('生成作品预览',w);return this.view(r);
  }catch(e){fs.rmSync(staging,{recursive:true,force:true});throw e;}
 }
 async remote(r,payload){
  this.enabled();const url=new URL('/api/dct/projects',this.siteOrigin);if(!payload){url.searchParams.set('workspaceId',r.workspaceId);url.searchParams.set('projectKey',r.projectKey);}
  let response;try{response=await fetch(url,{method:payload?'POST':'GET',redirect:'error',headers:{Authorization:'Bearer '+this.serviceKey,...(payload?{'Content-Type':'application/json'}:{})},body:payload?JSON.stringify(payload):undefined,signal:AbortSignal.timeout(15000)});}catch{throw fail(503,'作品区暂时无法连接，请稍后查看发布状态');}
  if(response.status===404&&!payload)return null;
  if(!response.ok){await response.body?.cancel();throw fail(503,'作品区暂时无法处理请求，请稍后查看发布状态');}
  let chunks=[],n=0;for await(const c of response.body){n+=c.length;if(n>256*1024)throw fail(502,'作品区响应过大');chunks.push(c);}let data;try{data=JSON.parse(Buffer.concat(chunks));}catch{throw fail(502,'作品区响应无效');}if(!data||typeof data!=='object')throw fail(502,'作品区响应无效');return data;
 }
 async submit(w,id){await this.active(w);const r=this.record(w,id);r.attempted=true;this.save(r);const data=await this.remote(r,{workspaceId:r.workspaceId,projectKey:r.projectKey,version:r.version,title:r.title,owner:r.owner,summary:r.summary,previewUrl:this.view(r).previewUrl,assetUrl:this.origin+'/releases/'+r.version+'/index.html',files:r.files.length,bytes:r.bytes});if(!['pending','approved','rejected'].includes(data.status))throw fail(502,'作品区状态无效');r.status=data.status;r.projectUrl=this.projectUrl(data.projectUrl);this.save(r);this.cache.delete(r.workspaceId+':'+r.projectKey);this.app.store.audit('提交作品审核',w);return this.view(r);}
 projectUrl(value){try{const u=new URL(value);if(u.origin===this.siteOrigin&&u.pathname==='/projects')return u.href;}catch{}throw fail(502,'作品区链接无效');}
 async grant(r){const key=r.workspaceId+':'+r.projectKey,c=this.cache.get(key);if(c&&c.until>Date.now())return c.data;const data=await this.remote(r);this.cache.set(key,{until:Date.now()+15000,data});return data;}
 async status(w,id){await this.active(w);const r=this.record(w,id);if(r.attempted){const data=await this.grant(r),v=data?.versions?.find(v=>v.version===id);if(v&&['pending','approved','rejected'].includes(v.status)){r.status=v.status;if(data.projectUrl)r.projectUrl=this.projectUrl(data.projectUrl);this.save(r);}}return this.view(r);}
 async browser(req,res,w,target,prefix,session){
  this.enabled();if(session.role!=='student')throw fail(403,'请由学生本人确认提交作品');await this.active(w);const u=new URL(target,this.app.origin),action=u.pathname.slice('/_dct/publish/'.length);
  if(action==='status'&&req.method==='GET')return send(res,200,await this.status(w,u.searchParams.get('version')));
  if(action==='submit'&&req.method==='POST'){const data=await body(req);return send(res,200,await this.submit(w,data.version));}
  if(action!=='prepare'||req.method!=='POST')throw fail(404,'接口不存在');
  const data=await body(req);if(this.busy)throw fail(429,'正在生成另一个预览，请稍后重试');this.busy=true;
  try{const info=await this.app.driver.inspect(w);if(!info)throw fail(409,'请先打开工作区');const mounts=info.Mounts;const state=mounts.find(m=>m.Destination==='/home/dc/.dct'),work=mounts.find(m=>m.Destination==='/home/dc/work');if(!state||!work)throw fail(409,'工作区存储配置无效');
   const output=await exec('python3',[here+'collect-static.py',work.Source,state.Source,String(data.projectId||''),String(data.dir||'dist')],{timeout:120000,maxBuffer:MAX_BODY});const bundle=JSON.parse(output.stdout);return send(res,201,await this.prepare(w,{...bundle,title:data.title,summary:data.summary,outputDir:data.dir||'dist'}));
  }catch(e){if(e.status)throw e;throw fail(400,'无法读取静态目录；请检查目录、文件类型和大小，且不要使用链接文件');}finally{this.busy=false;}
 }
 async provision(w){
  if(!this.serviceKey)return;const info=await this.app.driver.inspect(w);if(!info?.State?.Running)return;
  if(!w.publishToken){w.publishToken=secret();this.app.store.save();}
  const signature=info.Id+':'+info.State.StartedAt+':'+w.publishToken;if(this.provisioned.get(w.id)===signature)return;
  // docker exec runs as dc; the only written credential is scoped to this workspace.
  const code="import os,json,sys,tempfile; p=os.path.expanduser('~/.dct'); fd,t=tempfile.mkstemp(prefix='publish-',dir=p); os.fchmod(fd,0o600); f=os.fdopen(fd,'w'); f.write(sys.argv[1]); f.close(); os.replace(t,p+'/publish.json')";
  await this.app.driver.docker(['exec','--user','dc',this.app.driver.name(w),'python3','-c',code,JSON.stringify({url:this.app.origin,token:w.publishToken})]);this.provisioned.set(w.id,signature);
 }
 async serve(req,res){
  if(!['GET','HEAD'].includes(req.method))throw fail(405,'只支持读取作品');let u;try{u=new URL(req.url,this.origin);}catch{throw fail(400,'地址无效');}
  const m=u.pathname.match(/^\/(preview|releases)\/([a-f0-9]+)\/(.*)$/);if(!m)throw fail(404,'作品不存在');
  let r;if(m[1]==='preview'){if(!/^[a-f0-9]{64}$/.test(m[2]))throw fail(404,'预览不存在');r=[...this.records.values()].find(r=>equal(r.previewToken,m[2]));if(!r||r.previewExpires<Date.now())throw fail(404,'预览已过期，请重新生成');}
  else{if(!/^[a-f0-9]{32}$/.test(m[2]))throw fail(404,'作品不存在');r=this.records.get(m[2]);if(!r)throw fail(404,'作品不存在');const grant=await this.grant(r);if(grant?.published!==true||grant.currentVersion!==r.version)throw fail(404,'作品尚未发布或已撤下');}
  const w=this.app.store.data.students.find(w=>w.id===r.workspaceId);if(!w||w.disabled)throw fail(404,'作品已停用');
  let name;try{name=decodeURIComponent(m[3]||'index.html');validatePath(name);}catch{throw fail(404,'文件不存在');}const item=r.files.find(f=>f.path===name);if(!item)throw fail(404,'文件不存在');
  res.writeHead(200,{'Content-Type':TYPES[item.ext],'Content-Length':item.size,'Cache-Control':'no-store','X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer','Cross-Origin-Opener-Policy':'same-origin','Permissions-Policy':'camera=(), microphone=(), geolocation=()','Content-Security-Policy':`sandbox allow-scripts allow-same-origin allow-downloads; default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'none'; frame-ancestors ${this.app.origin} ${this.siteOrigin}`});
  if(req.method==='HEAD')return res.end();await new Promise((resolve,reject)=>{const stream=fs.createReadStream(path.join(this.dir,r.version,'files',name));stream.on('error',reject);res.on('close',()=>{stream.destroy();resolve();});stream.pipe(res);stream.on('end',resolve);});
 }
}

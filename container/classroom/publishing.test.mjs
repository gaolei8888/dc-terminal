import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';import os from 'node:os';import path from 'node:path';import http from 'node:http';
import {Publishing} from './publishing.mjs';
const file=(path,text)=>({path,data:Buffer.from(text).toString('base64')});
test('scoped static previews, explicit submit, approval and revocation',async()=>{
 const dir=fs.mkdtempSync(path.join(os.tmpdir(),'dct-pub-'));let approved=false,offline=false,submits=0;const rows=[{id:'shared',name:'甲',publishToken:'a'.repeat(64),generation:1},{id:'b'.repeat(32),name:'乙',publishToken:'b'.repeat(64),generation:1}];
 const site=http.createServer(async(req,res)=>{if(offline){res.writeHead(503);return res.end();}assert.equal(req.headers.authorization,'Bearer service-key');let raw='';for await(const c of req)raw+=c;res.setHeader('Content-Type','application/json');if(req.method==='POST'){submits++;const p=JSON.parse(raw);return res.end(JSON.stringify({projectId:'p',projectUrl:'http://127.0.0.1:'+site.address().port+'/projects#demo',version:p.version,status:'pending'}));}res.end(JSON.stringify({published:approved,currentVersion:version,versions:[{version,status:approved?'approved':'pending'}]}));});await new Promise(r=>site.listen(0,'127.0.0.1',r));
 const app={origin:'http://example.test',store:{dir,data:{students:rows},save(){},audit(){}},sso:{permissions:async()=>({role:'student'})}};
 const pub=new Publishing(app,{dir:path.join(dir,'pub'),origin:'http://127.0.0.1',siteOrigin:'http://127.0.0.1:'+site.address().port,serviceKey:'service-key',allowHttp:true});
 const server=http.createServer((req,res)=>pub.agent(req,res,new URL(req.url,'http://localhost')).catch(e=>{res.writeHead(e.status||500,{'Content-Type':'application/json'});res.end(JSON.stringify({error:e.message}));}));await new Promise(r=>server.listen(0,'127.0.0.1',r));await new Promise(r=>pub.server.listen(0,'127.0.0.1',r));pub.origin='http://127.0.0.1:'+pub.server.address().port;
 const base='http://127.0.0.1:'+server.address().port;const call=(p,data,token=rows[0].publishToken)=>fetch(base+p,{method:data?'POST':'GET',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:data?JSON.stringify(data):undefined});let version;
 try{
 const payload={projectKey:'test-project',title:'咖啡馆',files:[file('index.html','<h1>Cafe</h1>'),file('assets/model.glb','model')]};
 assert.equal((await call('/_agent/publish/prepare',payload,'wrong')).status,401);
 for(const bad of ['../secret.txt','.env','x/.git/config','package.json','x.js.map','x\\evil.js'])assert.equal((await call('/_agent/publish/prepare',{...payload,files:[file('index.html','ok'),file(bad,'oops')]})).status,400,bad);
 pub.uploadRates.clear();assert.equal((await call('/_agent/publish/prepare',{...payload,files:[file('index.html','ok'),file('INDEX.HTML','dup')]})).status,400);
 pub.uploadRates.clear();
 assert.equal((await call('/_agent/publish/prepare',{...payload,outputDir:'a'.repeat(513)})).status,400);
 pub.uploadRates.clear();
 const r=await call('/_agent/publish/prepare',payload);assert.equal(r.status,201);const made=await r.json();version=made.version;assert.match(version,/^[a-f0-9]{32}$/);assert.match(made.confirmUrl,/\/w\/shared\/\?publish=/);assert.equal(submits,0);
 const preview=await fetch(made.previewUrl);assert.equal(preview.status,200);assert.match(await preview.text(),/Cafe/);assert.equal(preview.headers.get('set-cookie'),null);assert.match(preview.headers.get('content-security-policy'),/sandbox/);
 assert.equal((await call('/_agent/publish/status?version='+version,undefined,rows[1].publishToken)).status,404);
 assert.equal((await call('/_agent/publish/submit',{version})).status,404);
 assert.equal((await fetch(pub.origin+'/releases/'+version+'/index.html')).status,404);
 await pub.submit(rows[0],version);assert.equal(submits,1);assert.equal((await pub.status(rows[0],version)).status,'pending');
 approved=true;pub.cache.clear();assert.equal((await fetch(pub.origin+'/releases/'+version+'/index.html')).status,200);
 offline=true;pub.cache.clear();assert.equal((await fetch(pub.origin+'/releases/'+version+'/index.html')).status,503);offline=false;approved=false;pub.cache.clear();assert.equal((await fetch(pub.origin+'/releases/'+version+'/index.html')).status,404);
 rows[0].disabled=true;assert.equal((await call('/_agent/publish/status?version='+version)).status,403);
 }finally{await new Promise(r=>server.close(r));await new Promise(r=>site.close(r));await new Promise(r=>pub.server.close(r));fs.rmSync(dir,{recursive:true,force:true});}
});

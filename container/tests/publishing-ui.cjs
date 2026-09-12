const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'/tmp/dct-theme-browser/node_modules/playwright');
const fs=require('node:fs'),http=require('node:http'),assert=require('node:assert/strict');
(async()=>{
 const panel=fs.readFileSync(require('node:path').join(__dirname,'../upload-panel.html'),'utf8');
 const version='a'.repeat(32),forged='<img src=x onerror="window.publishingXSS=1">';
 let failPrepare=true,failSubmit=true,failStatus=false,phase='prepared',unsafeLinks=false;
 const requests=[];
 const server=http.createServer(async(req,res)=>{
  const u=new URL(req.url,'http://local'),p=u.pathname.replace(/^\/w\/[^/]+/,'');
  const chunks=[];for await(const chunk of req)chunks.push(chunk);
  const body=chunks.length?JSON.parse(Buffer.concat(chunks).toString()):null;requests.push({path:u.pathname,query:u.search,method:req.method,body});
  res.setHeader('Content-Type','application/json');
  if(p==='/_dct/projects')return res.end(JSON.stringify({projects:[{id:'current',name:'我的网页'}]}));
  if(p.endsWith('/files'))return res.end('{"files":[],"excluded":[]}');
  const data={version,title:forged,summary:forged,dir:'dist',status:phase,files:2,bytes:100,previewUrl:unsafeLinks?'javascript:window.publishingXSS=1':'/preview/'+version+'/',confirmUrl:'/w/alice/?publish='+version,projectUrl:phase==='approved'?'/works/student/':null};
  if(p==='/_dct/publish/prepare'){
   if(failPrepare){res.statusCode=400;return res.end('{"error":"没有找到 dist 文件夹，请先让 Agent 生成网页"}');}
   return res.end(JSON.stringify(data));
  }
  if(p==='/_dct/publish/status'){
   if(failStatus){res.statusCode=503;return res.end('{"error":"暂时无法查询，请重试"}');}
   return res.end(JSON.stringify(data));
  }
  if(p==='/_dct/publish/submit'){
   await new Promise(r=>setTimeout(r,180));
   if(failSubmit){res.statusCode=503;return res.end('{"error":"提交失败，请重试"}');}
   phase='pending';return res.end(JSON.stringify({status:phase,projectUrl:null}));
  }
  if(p.startsWith('/_dct/')){res.statusCode=404;return res.end('{}');}
  res.setHeader('Content-Type','text/html');
  if(p.startsWith('/preview/'))return res.end('<h1>预览作品</h1>');
  const prefix=u.pathname.match(/^\/w\/[^/]+/)?.[0]||'';
  res.end('<meta charset="utf-8"><style>html,body{margin:0;height:100%;overflow:hidden}</style><div id="terminal-container"><div class="xterm-viewport" style="background:rgb(20,20,20)"><textarea class="xterm-helper-textarea"></textarea></div></div><script>window.DCW_BASE_PATH='+JSON.stringify(prefix)+'</script>'+panel);
 });
 await new Promise(r=>server.listen(0,'127.0.0.1',r));const origin='http://127.0.0.1:'+server.address().port;
 const browser=await chromium.launch({channel:'chrome',headless:true});try{
  const context=await browser.newContext(),page=await context.newPage(),errors=[];page.setDefaultTimeout(5000);page.on('pageerror',e=>errors.push(e.message));
  const idle=()=>page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);
  const submitted=()=>requests.filter(r=>r.path.endsWith('/publish/submit')).length;
  await page.goto(origin+'/w/alice/');await idle();
  assert.equal(await page.locator('#dcw-publish').count(),1,'Toolbar exposes publishing');
  await page.locator('#dcw-publish').click();await page.locator('#dcw-publishing').waitFor({state:'visible'});
  assert.equal(await page.getByLabel('作品标题').getAttribute('maxlength'),'100');assert.equal(await page.getByLabel('作品简介（可选）').getAttribute('maxlength'),'500');
  assert.equal(await page.getByLabel('静态网页文件夹').inputValue(),'dist');
  await page.getByLabel('作品标题').fill('课堂网页');await page.getByLabel('作品简介（可选）').fill('我的练习');
  await page.locator('#dcw-publish-prepare').click();await page.getByText('没有找到 dist 文件夹，请先让 Agent 生成网页',{exact:true}).waitFor();await idle();
  assert.equal(await page.locator('#dcw-publish-submit').isEnabled(),false);
  failPrepare=false;const popup=page.waitForEvent('popup');await page.locator('#dcw-publish-prepare').click();const preview=await popup;await preview.waitForURL(origin+'/preview/'+version+'/');assert.equal(await preview.evaluate(()=>window.opener),null);await preview.close();await page.locator('#dcw-publish-preview').waitFor({state:'visible'});await idle();
  assert.deepEqual(requests.findLast(r=>r.path.endsWith('/publish/prepare')).body,{projectId:'current',dir:'dist',title:'课堂网页',summary:'我的练习'});
  assert.equal(submitted(),0,'Preparing a preview never submits');
  assert.equal(await page.locator('#dcw-publish-result img').count(),0);assert.match(await page.locator('#dcw-publish-result').innerText(),/<img/);assert.equal(await page.evaluate(()=>window.publishingXSS),undefined);
  await page.locator('#dcw-publish-submit').click();assert.equal(await page.locator('#dcw-publish-submit').isDisabled(),true);
  await page.getByText('提交失败，请重试',{exact:true}).waitFor();await idle();assert.equal(await page.locator('#dcw-publish-submit').isEnabled(),true,'Failed submission can retry');
  failSubmit=false;await page.locator('#dcw-publish-submit').click();await page.getByText('已提交，等待审核',{exact:true}).waitFor();await idle();assert.equal(submitted(),2);assert.deepEqual(requests.findLast(r=>r.path.endsWith('/publish/submit')).body,{version});assert.equal(await page.locator('#dcw-publish-submit').isDisabled(),true,'Pending version cannot be resubmitted');
  phase='approved';await page.locator('#dcw-publish-refresh').click();await page.locator('#dcw-publish-public').waitFor({state:'visible'});assert.equal(await page.locator('#dcw-publish-public').getAttribute('href'),origin+'/works/student/');
  phase='prepared';const previous=submitted();await page.goto(origin+'/w/alice/?publish='+version);await page.locator('#dcw-publish-submit').waitFor({state:'visible'});await idle();assert.equal(await page.locator('#dcw-publishing').isVisible(),true);assert.equal(submitted(),previous,'Confirmation deep link never submits automatically');assert.ok(requests.some(r=>r.path==='/w/alice/_dct/publish/status'&&r.query==='?version='+version));assert.equal(await page.getByLabel('作品标题').inputValue(),forged);
  failStatus=true;await page.reload();await page.getByText('暂时无法查询，请重试',{exact:true}).waitFor();await idle();assert.equal(await page.locator('#dcw-publish-submit').isDisabled(),true);failStatus=false;await page.locator('#dcw-publish-refresh').click();await idle();assert.equal(await page.locator('#dcw-publish-submit').isEnabled(),true,'Deep-link status failure can retry');
  unsafeLinks=true;await page.locator('#dcw-publish-refresh').click();await idle();assert.equal(await page.locator('#dcw-publish-preview').getAttribute('href'),null,'Unsafe URL schemes never become links');assert.equal(await page.evaluate(()=>window.publishingXSS),undefined);
  await page.getByLabel('作品标题').fill('修改后的作品');assert.equal(await page.locator('#dcw-publish-submit').isDisabled(),true,'Changed metadata must be prepared again');assert.equal(await page.locator('#dcw-publish-result').isVisible(),false);
  await page.setViewportSize({width:375,height:812});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));assert.ok(await page.locator('#dcw-drawer').evaluate(e=>e.scrollWidth<=e.clientWidth));assert.deepEqual(errors,[]);
  const queries=requests.filter(r=>r.path.endsWith('/publish/status')).length;await page.goto(origin+'/w/alice/?publish=invalid');await page.getByText('确认链接不完整，请重新打开 Agent 提供的链接',{exact:true}).waitFor();assert.equal(requests.filter(r=>r.path.endsWith('/publish/status')).length,queries);assert.equal(submitted(),previous);
  console.log('PASS publishing UI: prepare/error retry, explicit submit/retry, pending/approved, deep links/status retry, metadata XSS/unsafe URLs, prefixed API and mobile');
 }finally{await browser.close();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exit(1)});

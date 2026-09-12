const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'/tmp/dct-theme-browser/node_modules/playwright');
const fs=require('node:fs'),http=require('node:http'),assert=require('node:assert/strict');
(async()=>{
 const panel=fs.readFileSync(require('node:path').join(__dirname,'../upload-panel.html'),'utf8');
 const files=[{path:'src/Main.TS',size:50},{path:'images/猫.PNG',size:100},{path:'docs/说明.md',size:200},{path:'uploads/输入数据.CSV',size:300},...Array.from({length:150},(_,i)=>({path:'generated/file-'+i+'.txt',size:10}))];
 let failDownload=false,failFiles=false;const requests=[];
 const server=http.createServer((req,res)=>{const u=new URL(req.url,'http://local'),p=u.pathname.replace(/^\/w\/[^/]+/,'');requests.push(u.pathname+u.search);res.setHeader('Content-Type','application/json');
 if(p==='/_dct/projects')return res.end(JSON.stringify({projects:[{id:'current',name:'我的网页'},{id:'second',name:'另一个项目'}]}));
 if(p.endsWith('/files')){if(failFiles){res.statusCode=409;return res.end('{"error":"暂时无法读取文件，请刷新重试"}');}return res.end(JSON.stringify({files:p.includes('/second/')?[{path:'second.txt',size:1}]:files,excluded:[]}));}
 if(p.endsWith('/file')||p.endsWith('/download')){if(failDownload){res.statusCode=409;return res.end('{"error":"文件已经移走，请刷新列表"}');}res.setHeader('Content-Type','application/octet-stream');return res.end('data');}
 if(p.endsWith('/upload')){req.resume();req.on('end',()=>{files.push({path:'uploads/'+u.searchParams.get('name'),size:3});res.end('{}');});return;}
 if(p.startsWith('/_dct/')){res.statusCode=404;return res.end('{}');}
 res.setHeader('Content-Type','text/html');const prefix=u.pathname.match(/^\/w\/[^/]+/)?.[0]||'';res.end('<meta charset="utf-8"><style>html,body{margin:0;height:100%;overflow:hidden}</style><div id="terminal-container"><div class="xterm-viewport" style="background:rgb(20,20,20)"><textarea class="xterm-helper-textarea"></textarea></div></div><script>window.DCW_BASE_PATH='+JSON.stringify(prefix)+'</script>'+panel);
 });await new Promise(r=>server.listen(0,'127.0.0.1',r));const origin='http://127.0.0.1:'+server.address().port;
 const browser=await chromium.launch({channel:'chrome',headless:true});try{
 const context=await browser.newContext();const page=await context.newPage();page.setDefaultTimeout(5000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(origin+'/w/alice/');await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);await page.locator('.dcw-toggle').click();
 assert.equal(await page.locator('#dcw-files-project').count(),1,'Student files have separate source tabs');
 assert.equal(await page.locator('#dcw-files .dcw-file').count(),100,'Large lists render incrementally');assert.doesNotMatch(await page.locator('#dcw-files').innerText(),/输入数据/);
 await page.locator('#dcw-files-more').click();assert.equal(await page.locator('#dcw-files .dcw-file').count(),153);
 await page.getByLabel('搜索文件').fill('MAIN');assert.equal(await page.locator('#dcw-files .dcw-file').count(),1);assert.match(await page.locator('#dcw-files').innerText(),/Main.TS/);
 await page.getByLabel('文件类型').selectOption('image');assert.equal(await page.locator('#dcw-files .dcw-file').count(),0);await page.getByRole('button',{name:'清除筛选'}).click();
 await page.getByLabel('文件类型').selectOption('image');assert.match(await page.locator('#dcw-files').innerText(),/猫.PNG/);
 await page.locator('#dcw-files-uploads').click();assert.equal(await page.getByLabel('文件类型').inputValue(),'all');assert.match(await page.locator('#dcw-files').innerText(),/输入数据/);assert.doesNotMatch(await page.locator('#dcw-files').innerText(),/Main.TS/);
 const download=page.waitForEvent('download');await page.locator('#dcw-files').getByRole('button',{name:/^下载 /}).click();assert.equal(await(await download).failure(),null);await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);
 await page.locator('#dcw-files-downloads').click();assert.match(await page.locator('#dcw-files').innerText(),/输入数据/);assert.match(await page.locator('#dcw-file-hint').innerText(),/最新/);
 await page.reload();await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);await page.locator('.dcw-toggle').click();await page.locator('#dcw-files-downloads').click();assert.equal(await page.locator('#dcw-files .dcw-file').count(),1,'Download history survives reload');
 const again=page.waitForEvent('download');await page.getByRole('button',{name:/^再次下载 /}).click();await again;await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);assert.equal(await page.locator('#dcw-files .dcw-file').count(),1,'Repeated downloads do not duplicate history');assert.ok(requests.some(p=>p.includes('/w/alice/_dct/projects/current/file?path=uploads%2F')));
 failDownload=true;await page.locator('#dcw-download').click();await page.getByText('文件已经移走，请刷新列表',{exact:true}).waitFor();assert.equal(await page.locator('#dcw-files .dcw-file').count(),1,'Failures do not create download records');failDownload=false;
 await page.locator('#dcw-file').setInputFiles({name:'新的需求.md',mimeType:'text/plain',buffer:Buffer.from('new')});await page.waitForFunction(()=>document.querySelector('#dcw-status').textContent.includes('上传完成'));assert.equal(await page.locator('#dcw-files-uploads').getAttribute('aria-selected'),'true');assert.match(await page.locator('#dcw-files').innerText(),/新的需求/);
 await page.locator('#dcw-nav-library').click();await page.locator('#dcw-projects .dcw-card').filter({hasText:'另一个项目'}).getByRole('button',{name:'打开项目'}).click();await page.locator('#dcw-cancel').click();assert.match(await page.locator('#dcw-files').innerText(),/second.txt/);await page.locator('#dcw-files-downloads').click();assert.equal(await page.locator('#dcw-files .dcw-file').count(),0,'Download history is scoped to current project');
 await page.goto(origin+'/w/bob/');await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);await page.locator('.dcw-toggle').click();await page.locator('#dcw-files-downloads').click();assert.equal(await page.locator('#dcw-files .dcw-file').count(),0,'Other student history is isolated');
 failFiles=true;await page.locator('#dcw-files-project').click();await page.locator('#dcw-refresh').click();await page.getByText('暂时无法读取文件，请刷新重试',{exact:true}).waitFor();assert.equal(await page.locator('#dcw-files .dcw-file').count(),0,'Read failure does not leave stale files');failFiles=false;
 await page.setViewportSize({width:375,height:812});await page.locator('#dcw-refresh').click();await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));assert.ok(await page.locator('#dcw-drawer').evaluate(e=>e.scrollWidth<=e.clientWidth));await page.screenshot({path:'/tmp/dcw-file-manager-mobile.png'});assert.deepEqual(errors,[]);
 console.log('PASS file manager: source separation, search/type composition, pagination, download/retry/history persistence and isolation, upload discovery, project switching, errors, mobile');
 }finally{await browser.close();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exit(1)});

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || '/tmp/dct-theme-browser/node_modules/playwright');

test('administrator safely observes, assists, ends assistance, copies and revokes access', async () => {
  assert.ok(fs.existsSync(__dirname + '/admin.html'), 'administrator page exists');
  let logged = false; let observeCalls = 0; let manageAccess=true;
  const calls = [];
  const students = [{id:'s1',name:'小王 <img src=x onerror=alert(1)>',status:'running',classId:'1',className:'一班',memoryBytes:null,diskBytes:0,connections:0},{id:'s2',name:'小李',status:'stopped',classId:'2',className:'二班',shared:true}];
  const server = http.createServer(async (req,res) => {
    let body = ''; for await (const part of req) body += part;
    calls.push({url:req.url,method:req.method,body});
    if(req.url === '/admin/') {res.setHeader('Content-Type','text/html');return res.end(fs.readFileSync(__dirname + '/admin.html'));}
    res.setHeader('Content-Type','application/json');
    if(req.url === '/admin/api/login') {logged = true;return res.end('{}');}
    if(!logged) {res.statusCode = 401;return res.end('{"error":"请先登录"}');}
    if(req.url === '/admin/api/state') return res.end(JSON.stringify({capacity:{running:1,max:4,totalMemory:10000000000},permissions:{manageAccess},students}));
    if(req.url.includes('/observe')) {observeCalls++; return res.end(JSON.stringify({sessions:[{id:1,profile:'项目一',state:'running'},{id:2,profile:'项目二',state:'running'}],screen:{lines:[[{text:'<script>unsafe()</script>'},{text:' 正在工作'}]]}}));}
    if(req.url.endsWith('/end-assist')) return res.end('{}');
    if(req.url.endsWith('/assist')) return res.end('{"url":"/admin/workspaces/s1/view/"}');
    if(req.url.endsWith('/view/')) {res.setHeader('Content-Type','text/html');return res.end('<p>Student terminal</p>');}
    if(req.url.endsWith('/link') || req.url.endsWith('/rotate')) return res.end('{"url":"http://localhost/w/s1/#t=test-secret"}');
    if(req.url === '/admin/api/students' && req.method === 'POST') {students.push({id:'s3',name:JSON.parse(body).names[0],status:'stopped'});return res.end('{}');}
    res.end('{}');
  });
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  const browser = await chromium.launch({channel:'chrome',headless:true});
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.addInitScript(() => Object.defineProperty(navigator,'clipboard',{value:{writeText: async text => {window.copied=text;}}}));
    await page.clock.install();
    await page.goto(`http://127.0.0.1:${server.address().port}/admin/`);
    await page.getByLabel('管理密码').fill('password');
    await page.getByRole('button',{name:'登录',exact:true}).click();
    await page.locator('.name').filter({hasText:students[0].name}).waitFor();
    assert.equal(await page.locator('img').count(),0);
    assert.match(await page.locator('#capacity').innerText(),/1.*4/);
    await page.getByRole('button',{name:'添加学生',exact:true}).click();
    await page.getByLabel('学生姓名，每行一个').fill('小张\n小赵');
    await page.getByRole('button',{name:'确认添加'}).click();
    await page.locator('.name').filter({hasText:'小张'}).waitFor();
    assert.deepEqual(JSON.parse(calls.find(c=>c.url==='/admin/api/students'&&c.method==='POST').body).names,['小张','小赵']);
    const row = page.locator('.student').filter({hasText:students[0].name});
    await row.getByRole('button',{name:'复制链接',exact:true}).click();
    await page.waitForFunction(()=>window.copied?.includes('#t=test-secret'));
    await row.getByRole('button',{name:'查看',exact:true}).click();
    await page.getByText('<script>unsafe()</script> 正在工作',{exact:true}).waitFor();
    assert.equal(await page.locator('#screen script').count(),0);
    assert.equal(await page.locator('iframe').count(),0);
    await page.getByLabel('终端会话').selectOption('2');
    await page.waitForFunction(()=>document.querySelector('#screen').textContent.includes('正在工作'));
    assert.ok(calls.some(c=>c.url.includes('observe?session=2')));
    await page.getByRole('button',{name:'协助',exact:true}).click();
    await page.locator('iframe').waitFor();
    const workspace = await page.locator('iframe').boundingBox();
    assert.ok(workspace.width >= 1260,'Teacher workspace uses the browser width');
    assert.ok(workspace.height >= 620,'Teacher workspace uses remaining browser height');
    assert.equal(await page.evaluate(()=>document.documentElement.scrollHeight<=innerHeight),true,'Workspace has no outer page scrollbar');
    await page.setViewportSize({width:390,height:844});
    const narrow = await page.locator('iframe').boundingBox();
    assert.ok(narrow.width>=370 && narrow.height>=650,'Mobile assistance also fills available space');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    await page.setViewportSize({width:1280,height:720});
    assert.match(await page.locator('iframe').getAttribute('src'),/^\/admin\/workspaces\/s1\/view\/$/);
    await page.clock.fastForward(15000);
    await page.waitForTimeout(100);
    assert.ok(calls.filter(c=>c.url.endsWith('/assist')).length >= 2, 'assistance lease renews');
    await page.getByRole('button',{name:'结束协助',exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector('iframe'));
    assert.ok(calls.some(c=>c.url.endsWith('/end-assist')));
    await page.getByRole('button',{name:'返回学生列表'}).click();
    assert.equal(await page.locator('header').isVisible(),true,'Returning restores the management header');
    await row.getByText('更多').click();
    page.once('dialog',dialog=>dialog.accept());
    await row.getByRole('button',{name:'重置链接'}).click();
    await page.waitForFunction(()=>document.querySelector('#message').textContent.includes('链接'));
    await row.getByText('更多').click();
    let stopText;page.once('dialog',dialog=>{stopText=dialog.message();return dialog.accept();});
    await row.getByRole('button',{name:'停止工作区'}).click();
    await page.waitForTimeout(200);
    assert.match(stopText,/归档/);
    assert.ok(calls.some(c=>c.url.endsWith('/stop')));
    assert.ok(observeCalls>0);
    await page.setViewportSize({width:390,height:844});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth));
    manageAccess=false;await page.reload();await page.locator('#dashboard').waitFor({state:'visible'});
    assert.equal(await page.locator('#add').isVisible(),false,'Federated teachers cannot create local users');
    assert.equal(await page.getByRole('button',{name:'复制链接',exact:true}).count(),0);
    await page.getByLabel('班级').selectOption('1');assert.equal(await page.locator('.student').count(),1);
    logged = false;
    await row.getByRole('button',{name:'查看',exact:true}).click();
    await page.getByLabel('管理密码').waitFor();
    assert.equal(await page.locator('iframe').count(),0);
    assert.equal(await page.locator('#dashboard').isVisible(),false);
  } finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
});

// Run with PLAYWRIGHT_MODULE=/path/to/playwright node container/tests/project-panel.cjs.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const http = require('node:http');
const assert = require('node:assert/strict');
(async () => {
  const fragment = fs.readFileSync(require('node:path').join(__dirname, '../upload-panel.html'), 'utf8');
  const calls = [], projects = [{ id: 'current', name: '现有项目', dir: '/work', saved_at: null }];
  let failSave = false, assisting = false;
  const files=[{ path: 'src/<img onerror=alert(1)>.txt', size: 12 }];
  const server = http.createServer((req, res) => {
    const u = new URL(req.url, 'http://localhost');
    const prefix = u.pathname.startsWith('/w/demo/') ? '/w/demo' : '';
    const apiPath = u.pathname.slice(prefix.length);
    if (!apiPath.startsWith('/_dct/')) { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end('<meta charset="utf-8"><style>html,body{height:100%;margin:0;overflow:hidden}.xterm-screen{width:100%;height:100px}</style><div id="terminal-container"><div class="xterm-viewport" style="background:rgb(20,20,20)"><div class="xterm-screen"></div><textarea class="xterm-helper-textarea"></textarea></div></div>' + (prefix ? '<script>window.DCW_BASE_PATH="/w/demo";window.DCW_STUDENT_NAME="小林";</script>' : '') + fragment); return; }
    calls.push([req.method, u.pathname, u.search]);
    let body = ''; req.on('data', d => body += d); req.on('end', () => {
      res.setHeader('Content-Type', 'application/json');
      let data;
      if(apiPath === '/_dct/presence'){if(!prefix)res.statusCode=404; data={assisting};}
      else if (apiPath === '/_dct/projects') {
        if (req.method === 'POST') { const p = { id: 'new', name: JSON.parse(body).name, dir: '/projects/new', saved_at: null }; projects.push(p); data = p; }
        else data = { projects, max_archive_bytes: 67108864 };
      } else if (u.pathname.endsWith('/files')) data = { files, excluded: ['.env', 'node_modules/'], truncated: false };
      else if (u.pathname.endsWith('/save')) { if (failSave) { res.statusCode = 409; data = { error: '文件正在修改，请重试' }; } else { projects[0].saved_at = 1770000000; data = { saved_at: 1770000000, files: 1, bytes: 12 }; } }
      else if (u.pathname.endsWith('/end')) {if(failSave){res.statusCode=409;data={error:'save_failed'};}else data = { saved_at: 1770000001, stopped: 1, files: 1, bytes: 12 };}
      else if (u.pathname.endsWith('/continue')) { assert.equal(JSON.parse(body).profile, 'codex'); data = { id: 'session-1', dir: '/work' }; }
      else if (u.pathname.endsWith('/upload')) { files.push({path:'uploads/'+u.searchParams.get('name'),size:5}); data = { name: u.searchParams.get('name') }; }
      else if (u.pathname.endsWith('/download') || u.pathname.endsWith('/file')) { res.setHeader('Content-Type', 'application/octet-stream'); res.end('complete download'); return; }
      else { res.statusCode = 404; data = { error: 'unknown' }; }
      res.end(JSON.stringify(data));
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1280, height: 850 } });
    const page = await context.newPage();
    const errors=[]; page.on('pageerror', e => errors.push(e.message));
    const origin = `http://127.0.0.1:${server.address().port}`;
    await page.goto(origin); await page.waitForFunction(() => !document.querySelector('#dcw-download').disabled);
    assert.equal(await page.locator('#dcw-panel').evaluate(el=>el.classList.contains('dcw-collapsed')),true);
    for(const id of ['nav-library','drop','download','project-name']) assert.equal(await page.locator('#dcw-'+id).isVisible(),true);
    assert.equal(await page.locator('#dcw-save').count(),0);
    assert.equal(await page.locator('#dcw-profile').isVisible(),false);
    assert.doesNotMatch(await page.locator('#dcw-panel').innerText(), /归档|uploads\/|\/work/);
    await page.locator('.dcw-toggle').click(); await page.locator('#dcw-files .dcw-file').waitFor();
    assert.equal(await page.locator('#dcw-files img').count(), 0);
    assert.match(await page.locator('#dcw-files').innerText(), /<img/);
    await page.locator('#dcw-nav-library').click();await page.getByRole('button',{name:'打开项目',exact:true}).click();
    await page.locator('#dcw-profile').selectOption('codex');await page.locator('#dcw-confirm').click();await page.getByText('项目已打开，请在终端看板中进入会话。',{exact:true}).waitFor();
    await page.locator('#dcw-file').setInputFiles({ name: 'test.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') });
    await page.getByText('上传完成，文件列表已刷新。', { exact: true }).waitFor();
    for (const locator of ['#dcw-files button', '#dcw-download']) {
      const event = page.waitForEvent('download'); await page.locator(locator).click(); const d = await event; assert.equal(await d.failure(), null);
      await page.waitForFunction(() => !document.querySelector('#dcw-download').disabled);
    }
    await page.locator('#dcw-end').click(); assert.equal(calls.some(c=>c[1].endsWith('/end')),false); await page.locator('#dcw-cancel').click();
    failSave=true;await page.locator('#dcw-end').click();await page.locator('#dcw-confirm').click();await page.getByText('操作未完成，请稍后重试',{exact:true}).waitFor();assert.equal(await page.locator('#dcw-status').getAttribute('data-error'),'true');failSave=false;
    await page.locator('#dcw-confirm').click(); await page.locator('#dcw-library').waitFor({state:'visible'});
    await page.locator('#dcw-new').click(); await page.locator('#dcw-name').fill('我的新项目'); await page.locator('#dcw-confirm').click(); await page.getByRole('heading', { name:'我的新项目', exact:true }).first().waitFor();
    await page.reload(); await page.waitForFunction(() => document.querySelector('#dcw-project-name').textContent === '我的新项目');
    await page.evaluate(() => document.querySelector('.xterm-viewport').style.backgroundColor='rgb(255,255,255)'); await page.waitForFunction(() => document.querySelector('#dcw-panel').dataset.theme==='light');
    await page.evaluate(() => document.querySelector('.xterm-viewport').style.backgroundColor='rgb(20,20,20)'); await page.waitForFunction(() => document.querySelector('#dcw-panel').dataset.theme==='dark');
    assert.equal(await page.locator('#dcw-theme').count(),1,'Workspace has a theme switch');
    const writesBefore=calls.filter(c=>c[0]==='POST').length;
    await page.locator('#dcw-theme').click();
    await page.waitForFunction(()=>document.querySelector('#dcw-panel').dataset.theme==='light');
    assert.equal(await page.locator('#dcw-theme').getAttribute('aria-label'),'切换到深色');
    await page.reload();await page.waitForFunction(()=>document.querySelector('#dcw-panel').dataset.theme==='light');
    // Incoming agent appearance changes must not undo a student's explicit choice.
    await page.evaluate(()=>document.querySelector('.xterm-viewport').style.backgroundColor='rgb(255,255,255)');
    await page.waitForFunction(()=>getComputedStyle(document.querySelector('#terminal-container')).filter==='none');
    await page.evaluate(()=>document.querySelector('.xterm-viewport').style.backgroundColor='rgb(20,20,20)');
    await page.waitForFunction(()=>getComputedStyle(document.querySelector('#terminal-container')).filter!=='none');
    assert.equal(await page.locator('#dcw-panel').getAttribute('data-theme'),'light');
    const otherTab=await page.context().newPage();await otherTab.goto(origin);
    await otherTab.waitForFunction(()=>document.querySelector('#dcw-panel').dataset.theme==='light');
    await page.locator('#dcw-theme').click();await otherTab.waitForFunction(()=>document.querySelector('#dcw-panel').dataset.theme==='dark');
    await page.locator('#dcw-theme').click();
    await otherTab.goto(origin+'/w/demo/');await otherTab.waitForFunction(()=>document.querySelector('#dcw-panel').dataset.theme==='dark');
    assert.equal(calls.filter(c=>c[0]==='POST').length,writesBefore,'Theme changes never type commands or mutate projects');
    const mobile=await browser.newPage({viewport:{width:390,height:844}});await mobile.goto(origin);await mobile.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);
    assert.equal(await mobile.locator('#dcw-panel').evaluate(el=>el.classList.contains('dcw-collapsed')),true);
    await mobile.locator('.dcw-toggle').click(); assert.equal(await mobile.locator('#dcw-backdrop').isVisible(),true);
    assert.ok(await mobile.locator('#dcw-drawer').evaluate(el=>el.getBoundingClientRect().width<=352));
    await mobile.locator('.dcw-toggle').click(); assert.equal(await mobile.evaluate(()=>document.activeElement.className),'xterm-helper-textarea');
    assert.equal(await mobile.evaluate(()=>document.documentElement.scrollWidth),390);
    const prefixed=await browser.newPage();await prefixed.goto(origin+'/w/demo/');await prefixed.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);
    assert.equal(await prefixed.locator('#dcw-project-name').innerText(),'现有项目');
    assert.equal(await prefixed.locator('#dcw-presence').isVisible(),false);assisting=true;
    await prefixed.locator('#dcw-presence').waitFor({state:'visible',timeout:8000});assisting=false;
    await prefixed.locator('#dcw-presence').waitFor({state:'hidden',timeout:8000});
    assert.ok(calls.some(c=>c[1]==='/w/demo/_dct/projects/current/files'));
    assert.ok(calls.some(c=>c[1]==='/w/demo/_dct/presence'));
    assert.equal(calls.some(c=>c[1].endsWith('/save')),false);
    assert.deepEqual(errors,[]);
    console.log('PASS: project APIs, safe filenames, upload, file/ZIP download, plain errors, end confirmation, create, tab selection, dark/light sync, mobile drawer/focus/width, route prefix, presence updates');
  } finally { await browser.close(); server.close(); }
})().catch(e => { console.error(e); process.exit(1); });

// Removing the paste listener, auto-sending images, or ignoring OS-specific hints must fail this test.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'/tmp/dct-theme-browser/node_modules/playwright');
const fs=require('node:fs'),http=require('node:http'),assert=require('node:assert/strict'),path=require('node:path');
(async()=>{
 const panel=fs.readFileSync(path.join(__dirname,'../upload-panel.html'),'utf8');
 const png=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEklEQVR4nGM0SjnBwMDAxAAGAA/8AWKyGls2AAAAAElFTkSuQmCC','base64');
 const uploads=[];let fail=false;
 const server=http.createServer((req,res)=>{
  const u=new URL(req.url,'http://localhost'),route=u.pathname.replace(/^\/w\/demo/,'');res.setHeader('Content-Type','application/json');
  if(route==='/_dct/projects')return res.end(JSON.stringify({projects:[{id:'current',name:'练习项目',dir:'/work/demo space',saved_at:null}]}));
  if(route.endsWith('/files'))return res.end('{"files":[],"excluded":[]}');
  if(route.endsWith('/upload')){const chunks=[];req.on('data',b=>chunks.push(b));req.on('end',()=>{uploads.push({path:u.pathname,name:u.searchParams.get('name'),body:Buffer.concat(chunks)});res.statusCode=fail?409:201;res.end(JSON.stringify(fail?{error:'上传失败，请重试'}:{name:u.searchParams.get('name')}));});return;}
  if(route.startsWith('/_dct/')){res.statusCode=404;return res.end('{}');}
  res.setHeader('Content-Type','text/html; charset=utf-8');res.end('<meta charset="utf-8"><style>html,body{height:100%;margin:0;overflow:hidden}</style><div id="terminal-container"><div class="xterm-viewport" style="background:rgb(20,20,20)"><textarea class="xterm-helper-textarea"></textarea></div></div><script>window.DCW_BASE_PATH="/w/demo";window.sent=[];window.term={paste:text=>window.sent.push(text),focus:()=>document.querySelector(".xterm-helper-textarea").focus()};</script>'+panel);
 });await new Promise(r=>server.listen(0,'127.0.0.1',r));
 const browser=await chromium.launch({channel:'chrome',headless:true});
 try{
  for(const platform of ['MacIntel','Win32','Linux x86_64']){
   const context=await browser.newContext({viewport:{width:390,height:844}});await context.addInitScript(platform=>Object.defineProperty(navigator,'platform',{value:platform}),platform);
   const page=await context.newPage();page.setDefaultTimeout(4000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
   await page.goto(`http://127.0.0.1:${server.address().port}/w/demo/`);await page.waitForFunction(()=>!document.querySelector('#dcw-download').disabled);
   assert.match(await page.locator('#dcw-footbar').innerText(),platform==='MacIntel'?/⌘V 粘贴文字或图片/:/Ctrl\+V 粘贴文字或图片/);
   async function paste(target,text,image=true){return page.locator(target).evaluate((el,{text,image,bytes})=>{const data=new DataTransfer();if(image)data.items.add(new File([new Uint8Array(bytes)],'image.png',{type:'image/png'}));if(text)data.setData('text/plain',text);const event=new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true});el.dispatchEvent(event);return event.defaultPrevented;},{text,image,bytes:[...png]});}
   assert.equal(await paste('.xterm-helper-textarea','普通文字',false),false,'Text paste stays with xterm');
   assert.equal(await paste('#dcw-file-search','',true),false,'Other inputs are not terminal image targets');
   const before=uploads.length;
   assert.equal(await paste('.xterm-helper-textarea','看这张图'),true);
   await page.locator('#dcw-paste-preview img').waitFor();assert.ok(await page.locator('#dcw-paste-preview img').evaluate(img=>img.complete&&img.naturalWidth>0));assert.equal(uploads.length,before,'Preview does not upload or send');assert.deepEqual(await page.evaluate(()=>window.sent),[]);
   await page.getByRole('button',{name:'移除图片',exact:true}).click();assert.equal(await page.locator('#dcw-paste-preview img').count(),0);assert.equal(uploads.length,before);
   await paste('.xterm-helper-textarea','');await page.locator('#dcw-paste-note').fill('帮我分析');
   await page.getByRole('button',{name:'上传图片',exact:true}).click();await page.getByRole('button',{name:'插入当前会话',exact:true}).waitFor({state:'visible'});
   assert.equal(uploads.length,before+1);assert.equal(uploads.at(-1).path,'/w/demo/_dct/projects/current/upload');assert.deepEqual(uploads.at(-1).body,png);assert.deepEqual(await page.evaluate(()=>window.sent),[],'Upload must not type into a possibly changed session');
   await page.getByRole('button',{name:'插入当前会话',exact:true}).click();const sent=await page.evaluate(()=>window.sent);assert.equal(sent.length,1);assert.match(sent[0],/^'\/work\/demo space\/uploads\/paste-[a-z0-9-]+\.png' 帮我分析$/);assert.doesNotMatch(sent[0],/[\r\n\x1b]/,'Do not press Enter for the student');
   fail=true;await paste('.xterm-helper-textarea','');await page.getByRole('button',{name:'上传图片',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#dcw-paste-message').textContent.includes('上传失败'));assert.equal(await page.getByRole('button',{name:'插入当前会话',exact:true}).isVisible(),false);assert.equal(await page.evaluate(()=>window.sent.length),1);
   fail=false;await page.getByRole('button',{name:'上传图片',exact:true}).click();await page.getByRole('button',{name:'插入当前会话',exact:true}).waitFor();
   await page.getByRole('button',{name:'移除图片',exact:true}).click();
   if(platform==='MacIntel'){
    await context.grantPermissions(['clipboard-read','clipboard-write']);
    await page.evaluate(async bytes=>{await navigator.clipboard.write([new ClipboardItem({'image/png':new Blob([new Uint8Array(bytes)],{type:'image/png'})})]);},[...png]);
    await page.locator('#dcw-close').click();await page.locator('.xterm-helper-textarea').focus();
    await page.keyboard.press('Meta+v');await page.locator('#dcw-paste-preview img').waitFor();
    assert.equal(await page.locator('#dcw-paste-preview img').count(),1,'Native Cmd+V reaches the image preview');
   }
   const key=await page.locator('.xterm-helper-textarea').evaluate(el=>{let seen=false;el.addEventListener('keydown',()=>seen=true,{once:true});const event=new KeyboardEvent('keydown',{key:'F5',bubbles:true,cancelable:true});el.dispatchEvent(event);return {prevented:event.defaultPrevented,seen};});
   assert.deepEqual(key,{prevented:false,seen:false},'F5 refresh remains browser-owned and is not sent to xterm');

   assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);assert.deepEqual(errors,[]);
   await context.close();console.log('PASS clipboard:',platform);
  }
 }finally{await browser.close();server.close();}
})().catch(e=>{console.error(e);process.exitCode=1});

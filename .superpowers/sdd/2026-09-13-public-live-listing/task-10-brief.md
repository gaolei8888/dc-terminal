### Task 10: 管理台界面

**Files:**
- Modify: `container/classroom/admin.html`

**Interfaces:**
- Consumes: Task 9 的 `features.publicLive`、`live.public`、`livePublicError`、`live-public` / `live-private` / `live-start {public,title}`。

- [ ] **Step 1: 改渲染与菜单**

`admin.html` 的脚本是压缩成一行的；用 `grep -o` 定位下面几个片段后替换（先 `cp admin.html /tmp/admin.html.bak` 便于比对）。

1. 状态文本：把

```js
(student.live?' · ● 直播中 · '+(student.live.viewers??0)+' 人在看':'')
```

换成

```js
(student.live?(student.live.public?' · ● 公开直播中 · '+student.live.public.title+' · ':' · ● 直播中 · ')+(student.live.viewers??0)+' 人在看':'')+(student.livePublicError?' · 公开失败：'+student.livePublicError:'')
```

（`metrics.textContent = ...` 本来就是 `textContent`，标题不会被当成 HTML。）
2. 在「复制直播链接」那一项之后加两项：

```js
if(features.publicLive&&student.live&&!student.live.public)menuAction('公开这场直播',async()=>{const title=prompt('公开标题（学生姓名和屏幕会出现在 live.dataclue.cn 的公开列表上，任何人都能看）',student.name+' 的工作区');if(title&&title.trim()){await api(route(student.id,'live-public'),{title:title.trim()});await loadState();message('已公开，live.dataclue.cn 上现在看得到这场直播。');}});if(features.publicLive&&student.live&&student.live.public)menuAction('取消公开',async()=>{await api(route(student.id,'live-private'),{});await loadState();message('已取消公开，私密链接照常能看。');},true);
```

3. 「直播这个工作区」的确认之后、调用 `live-start` 之前，加「同时公开」：把 `const r=await api(route(student.id,'live-start'),{});` 换成

```js
let extra={};if(features.publicLive&&confirm('同时公开这场直播？\n\n学生姓名和屏幕会出现在 live.dataclue.cn 的公开列表上，任何人都能看。选「取消」就只开私密直播。')){const title=prompt('公开标题',student.name+' 的工作区');if(title&&title.trim())extra={public:true,title:title.trim()};}const r=await api(route(student.id,'live-start'),extra);
```

4. `loadState` 里保存 `features`：在把 `students` 赋值的地方旁边加 `features=data.features||{};`，并在脚本顶部变量声明处加 `let features={};`（照现有 `students` 的声明方式）。

- [ ] **Step 2: 手工验收**

Run（仓库根）：`cd container/classroom && node --test server.test.mjs`，确认 Task 9 的测试仍然 PASS；另起一个本机管理台（按 `README.md`「验证」一节的方式，传入假的 `live` 选项）用浏览器点一遍：私密开播、同时公开、公开、取消公开、状态文字。**确认标题里写 `<b>x</b>` 时状态行显示的是原样文字。**

- [ ] **Step 3: Commit**

```bash
git add container/classroom/admin.html
git commit -m "feat(classroom): admin controls to publish and unpublish a live workspace"
```

---


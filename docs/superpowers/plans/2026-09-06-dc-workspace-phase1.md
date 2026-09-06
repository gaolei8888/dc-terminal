# dc-workspace 第一期：容器里的看板 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 学生本地什么都不装，浏览器打开一个地址，看到的是**完整的 dct 看板**，
里面的 agent 跑在服务器上。关掉浏览器，会话照跑。

**Architecture:** 第一版把 TUI 和守护进程**一起**放进容器，ttyd 只负责把那个终端搬到
浏览器里。**但鉴权那道门按第二期的最终形态写**——第二期是本地 TUI 直连远程守护进程
（`client.rs` 换传输），那时候要的 token 校验跟现在 ttyd 前面这道门是同一件事。
写成一个模块，第二期就不用返工。

**Tech Stack:** 镜像 Debian slim；`dct` 交叉编到 `x86_64-unknown-linux-gnu`；ttyd；
Node 运行时和 agent CLI 在**构建期**装好，不留给运行时下载。

---

## 已经查实的地基（读代码得来，不是推断）

**一、协议层是白送的。** `src/proto.rs` 的 `Request`/`Response` 已经覆盖界面要的全部动作
（`List`/`Create`/`Input`/`Screen`/`Resize`/`Scroll`/`Mouse`/`Key`/`Diff`/`Undo`）。
`link.rs` 和 `web/routes.rs` 已经各自证明过一遍：这些请求能走网络，走的还是 `daemon.rs`
里**同一个 `handle` 闭包**。所以「把界面接到远处」不是这件事的难点，第二期也不会是。

**二、容器里的进程链比 Windows 浅一层。** `sys::shell::launch_argv` 在 Unix 上原样奉还
（Windows 才垫 `cmd.exe /c`）。所以守护进程 spawn 到的直接就是 agent，
`fc51cc0` 那条「kill 打不穿外面那层壳」在 Linux 上不存在——`sys::job` 在 Unix 是
一个永远返回 `None` 的空壳，那边的保证来自 pty 的前台进程组。

**三、`docker stop` 在这里是真的优雅退出。** `sys::proc` 开头那段说 Windows 没有 SIGTERM
这句话、守护进程被换掉时来不及让 agent 收尾。容器里 PID 1 收到的正是 SIGTERM——
这是搬到 Linux 白捡的一件事，值得在文档里写给运维看。

**四、数字来自 `2dd5381` 那次实测，不是估的。** 一个 agent 常驻约 320 MB；
Node 运行时 95 MB，`claude` 那个 npm 包 416 MB。前者定容器的内存 limit，后者定镜像大小。

---

## 为什么是这条路（三选一的排除过程）

**排掉 A：复用 `link.rs` + `dct-srv` 中转。** 方向反了。`link.rs` 头一句就写着它存在的
理由——「家里的笔记本没有公网地址」。容器天生有地址，让它主动出网连中转是白绕一圈。
而且 `dct-srv` 现在硬性只绑环回（`must_be_loopback`），要开公网得先补鉴权和加密，
那是 `2026-08-23-dc-terminal-srv-phase1.md` 明说还没做的第二期。

**第一期做 C（容器里连 TUI 一起跑）。** dc-terminal 侧零改动，这一期就能发给训练营。
「关掉窗口不影响会话」照样成立，而且成立的理由没变：守护进程和 TUI 本来就是分开的
进程，ttyd 断了守护进程照跑，重连再起一个 TUI 就接回去。

**第二期做 B（`client.rs` 换传输）。** 本地 TUI 直连容器，手感全保住。它的成本已经点清：
`sys::ipc::bind_private`（只有属主能连）和 `peer_pid`（对面是谁）这两条保证在 TCP 上
全部失效，得用 token + TLS 重新建一遍；界面侧选目录现在走本地文件系统
（`ui/app.rs:441` 的 `current_dir`），得改成问远端。

**一次规划到位的那一点，就是那道门。** 第一期不许写成 `ttyd --credential user:pass`
了事——那样第二期要从零建鉴权。见任务 3。

---

## Global Constraints（硬的，不是建议）

**一人一容器。** `sys::ipc::bind_private` 上面那句是前提：谁连得上那个 socket，谁就能
以你的身份在这台机器上执行任意命令。一个容器塞多个学生 = 共用一个 socket =
互相能进对方的会话、读对方的 `secrets.toml`。这条不做取舍。

**PID 1 必须会收尸。** 用 `--init`（或镜像里放 tini）。理由不是守护进程漏收——
它自己 `child.wait()` 收得干净（`pty.rs` 那条 `no_zombie` 测试盯着）——而是**父进程先死时
孤儿会被 reparent 到 PID 1**，而一个不 reap 的 PID 1 会让僵尸堆到进程表满。
这一条我**还没在容器里验过**，任务 2 的验收条件就是验它。

**运行时预装进镜像，不现下。** `runtime.rs` 那套「下一份 Node 放 `~/.dct/runtime/node`」
是给单机用户写的。三十个学生同时开容器各下 500 MB 是另一回事。构建期装好，
国内镜像源（`runtime.rs` 里已有那套逻辑）在这里变成构建参数。

**状态必须落在命名卷上。** 两处，缺一个学生就丢东西：
- `~/.dct` —— `secrets.toml`、`projects.json`、`last-sessions.toml` 全靠
  `store_path_for_socket` 从 socket 路径推出来（`sys::ipc` 那段注释是故意这么设计的），
  所以卷挂在 `~/.dct` 上一次性全覆盖。
- `~/.claude` —— 对话记录，会长，本机一周涨到 185 MB。这个要单独说，因为它决定
  磁盘配额，也决定 `--resume` 还接不接得回去。

**不以 root 跑。** agent 会在项目目录里写文件。root 跑出来的文件在绑定挂载那条路上
会变成宿主上删不掉的东西。

---

## 两种代码位置，两套代价（都支持，文档里说清）

这条决定 git 检查点和撤销还灵不灵：`git.rs` 的快照是在守护进程侧对项目目录做的，
守护进程进了容器，被快照的目录就必须在容器里能看见。

| | 命名卷（推荐给训练营） | 绑定挂载宿主目录 |
|---|---|---|
| IO | 原速，git 检查点原速 | Windows/macOS 上慢，编译和 `node_modules` 明显拖 |
| 改代码 | VS Code Dev Containers / Remote-SSH，或干脆只让 agent 改 | 本地编辑器照常 |
| 咬人的地方 | 宿主上看不到文件，学生要导出得走命令 | 换行（`core.autocrlf`）和文件权限会咬 git 检查点 |

**验收条件是两套路径上各跑一遍检查点 + 撤销**，不是只测一套然后推另一套也行。
撤销是敢关掉权限确认的全部前提，它在哪条路径上是死的，那条路径就不能发。

---

## 不单起项目，但分两处放（镜像 / 编排）

**镜像留在本仓库。** 理由跟 `dct-page` 那段「整个仓库里只有这一份」是同一条：
Dockerfile 装的是这个仓库编出来的二进制，分开放两个 repo，迟早出现镜像里那份和
代码里那份对不上、而且只有在容器里才复现的 bug。另外两条：第二期那道门是 dct
自己的代码（守护进程侧的监听器），本来就只能在这里；CI 也已经在这里，
`release.yml` 打 `v*` tag 出三平台产物，加一个镜像产物是同一条流水线、同一个
tag，版本天然对齐。

**编排去 `dc_deploy`。** 那个仓库已经是这一套的家（compose 栈、keycloak、minio、
emqx，最近还在提交，而且做过 podman 兼容）。在这里再写一份 compose 就是第二套
部署真相，两套迟早走岔；何况训练营那台服务器上跑的不止 dct。

**代价是多一个跨仓库的接缝，所以契约写死成一句：dc-terminal 出一个镜像，
`dc_deploy` 决定怎么起它。** 镜像不许知道自己被起了几份、挂的什么卷、前面有什么
反代——它只认环境变量和卷路径。这条一破，两边就会各自长出一半配置。

**这条依赖一个前提：训练营那台服务器归 `dc_deploy` 管。** 如果不归，编排就退回
`workspace/compose.yaml`，上面那句契约照样成立，只是接缝在同一个仓库里。

```
workspace/              # 本仓库
  Dockerfile            # 多阶段：构建 dct → 运行时镜像
  entrypoint.sh         # 起守护进程、起 ttyd、把 SIGTERM 传下去
  gate/                 # 任务 3 那道门（第二期原样复用）
  README.md             # 镜像认哪些环境变量和卷；两种代码位置的代价表

dc_deploy/              # 另一个仓库
  一人一容器的 compose、卷、内存 limit、反代与证书
```

**旁边那三个 docker 项目都不是这件事的家**，查过了，写下来免得下一个人再查一遍：
`dc_docker_ng`/`dc_docker_ng1` 是两行 pytorch 的 Dockerfile；`dc-docker-on-demand`
是克隆的第三方（CTFd 那套按需起容器的 Python 服务），不是我们写的、也没在动——
将来真需要「网页上点一下发一个容器」时可以回头看它，这一期不用。

---

## Tasks

### 任务 1：镜像骨架，容器里能起守护进程
- [ ] 多阶段 Dockerfile：构建阶段编 `dct`，运行时阶段只留二进制 + Node + agent CLI
- [ ] 非 root 用户，`~/.dct` 和 `~/.claude` 是卷
- [ ] **验收**：容器里 `dct ps` 能连上守护进程；`docker stop` 之后没有留下进程
- [ ] **记下镜像实际大小**，跟上面 511 MB 的估算对一下，对不上就把差额写进本文件

### 任务 2：PID 1 与收尸
- [ ] `--init` 或 tini
- [ ] **验收**：起一个会话，让 agent 的父进程先死，确认孤儿被 reap 掉、进程表干净。
      这是上面那条「我还没验过」的约束，验完把结论写回 Global Constraints

### 任务 3：那道门（第二期原样复用）
- [ ] token 的产生、落盘、常数时间校验，写成一个模块——**不是** `ttyd --credential`
- [ ] 照抄 `web/mod.rs` 那段安全注释里已经踩过的三个坑：认证在路由之前、
      401 不泄露路径存不存在、比对用常数时间循环
- [ ] **验收**：没带对 token 的请求一律 401，且从响应上分不出路径存不存在

### 任务 4：ttyd 接进来
- [ ] 浏览器打开能看到完整看板
- [ ] **验收**：关掉浏览器标签、重开，会话还在，画面接得回去
- [ ] **记下手感上的实际损失**：中文输入法、Ctrl 组合键、复制粘贴各是什么表现。
      这三条是选 C 时明知要付的代价，付得起付不起要有实测才知道

### 任务 5：两种代码位置各验一遍
- [ ] 命名卷：开会话 → 让 agent 改文件 → 检查点 → 撤销
- [ ] 绑定挂载：同一套，外加 `core.autocrlf` 和文件权限那两个坑
- [ ] **podman 也要测一遍**，不能只测 docker：`dc_deploy` 已经做过 podman 兼容，
      而 rootless podman 的 uid 映射咬的正是上面那条文件权限
- [ ] **验收**：两条路径上撤销都真的回滚了。哪条不灵，就在 README 里写明它不灵，
      而不是写「建议使用另一种」

### 任务 6：一人一容器的编排
- [ ] compose 样板：内存 limit 按「一个 agent 320 MB × 几个」定，不拍脑袋
- [ ] 发 N 个学生的脚本
- [ ] **这一整个任务落在 `dc_deploy`，不落在本仓库**（见上面那节）。本仓库这边
      要交付的只有一样：`workspace/README.md` 里那份「镜像认哪些环境变量和卷」的
      清单——那就是契约本身
- [ ] **验收**：两个容器之间互相看不见对方的会话和密钥

### 任务 7（第二期）：`client.rs` 换传输
- [ ] 本地 TUI 直连，复用任务 3 那道门
- [ ] `current_dir` 改成问远端

---

## 第一期不做

- **不做 B。** 第一期交付的是浏览器里的完整看板，本地 TUI 直连是第二期。
- **不碰 `dct-srv`。** 它只监听环回这条硬性约束原样留着，跟 dc-workspace 无关。
- **不做多租户共享容器。** 见 Global Constraints 第一条。
- **不做镜像的自动更新。** 换版本 = 换镜像重建容器，卷带着状态过去。

---

## 这台机器上能验到什么程度（2026-09-06）

Docker 可用（Server 29.7.2、Compose v5.4.0），所以**任务 1 到 6 基本都能在这台机器上真跑**，
不是只能写完等上设备。两处例外，先说清楚：

- **浏览器里的中文输入法和快捷键手感**（任务 4）只能人在真设备上试，我在这里量不出来。
- **绑定挂载的 IO 代价**（任务 5）在 Windows 上跟在 Linux 服务器上是两个数，
  这台机器测出来的数字只能当上界，不能当结论。

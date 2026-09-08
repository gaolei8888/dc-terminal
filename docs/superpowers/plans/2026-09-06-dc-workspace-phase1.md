# dc-workspace 第一期：容器里的看板 —— 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 学生本地什么都不装，浏览器打开一个地址，看到的是**完整的 dct 看板**，
里面的 agent 跑在服务器上。关掉浏览器，会话照跑。

**这一期真正服务的是谁：压根没有合适环境的那批人。** README 里那张硬件表
（Win10 1809 起、8 GB 起）对他们就是一句「你不合格」——老机器、没有管理员权限的
公司电脑、Chromebook、平板。**远程版是那张表的出口**，不是给已经装得上的人多一个
选择。这条决定了下面好几处的取舍，尤其是那道门（任务 3）：门槛必须低到「点开一条
链接」，不是「复制一串 token 粘进去」。

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

**三、`docker stop` 走的是 SIGTERM——但白捡的没有我原先写的那么多。**
~~容器里 PID 1 收到的正是 SIGTERM，守护进程于是能让每个 agent 收尾。~~
**2026-09-06 实测推翻了这一条**：`docker stop` 0.35 秒返回、退出码 143（=128+15），
确实是 SIGTERM 不是 SIGKILL；但 `daemon.rs` 里**没有** SIGTERM 处理器——
`sys::signal` 那一套是给 TUI 还原终端用的，守护进程一次都没调过。所以它是被默认
动作直接打死的，并没有「自己把 pty 收拾干净」。agent 照样会被清掉，但那是**内核**
关掉 pty 主端之后发的 SIGHUP（`sys::job` 模块头描述的正是这条 Unix 路径）。

差别是实打实的：没有 `PtySession::kill` 里那 200ms 宽限期，agent 没机会自己收尾
落盘。**候选改进（不在第一期）**：在 `daemon.rs` 装一个 SIGTERM 处理器走正常停机
路径。那是 dct 的改动，值得单独排。

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
**2026-09-07 验过了**（任务 2）：孤儿确实 reparent 到 PID 1，而且只有 PID 1
是 tini 时才会被收掉——换成一个不 wait 的 PID 1，同一个孤儿就永久留在进程表里。

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

**线划在「一个人」和「一群人」之间，不划在「有没有 compose」上。**

`container/` 里**必须**有一份单人版 compose：`git clone` 之后 `docker compose up`
就得到一个能用的远程 dct，不依赖任何别的仓库。两条理由，第二条是硬的：

1. **独立性。** dc-terminal 一直是自己就能交付一个能跑的东西——`release.yml` 出
   三平台产物、装脚本从 `releases/latest` 拉，谁下谁用，它不认识任何一个用户。
   「远程版」不该是例外。
2. **可测性。** 任务 5 那条「两种代码位置各跑一遍 git 检查点和撤销」要在本仓库的
   CI 里跑，那就必须有一个本仓库自带、起得来的容器。编排全搬走的话，这一期最关键
   的验收条件就没地方跑了。

**「一群人」那部分归部署方**（三十个学生怎么发、反代和证书、跟 keycloak 接、
配额）。那是部署方的关注点，不是产品的——产品不知道自己的部署方是谁，**这恰恰
就是独立性本身，不是它的反面**。当下的部署方是 `dc_deploy`，但那是它的事，
不是本仓库要记住的事。

**依赖方向只许一个：部署方 → dc-terminal。** 检验办法是一句能证伪的规矩，
写在这里以便将来有人能拿它对照：

> **dc-terminal 的任何测试、任何 CI、任何 README 步骤，都不许出现部署方仓库的
> 名字。** 哪天守不住了，说明这条线又划错了，回来重划。

契约反过来说同一件事：**本仓库出一个镜像，部署方决定怎么起它。** 镜像不许知道
自己被起了几份、前面有什么反代——它只认环境变量和卷路径，那份清单就是契约本身，
写在 `container/README.md` 里。

```
container/              # 本仓库，自足（计划最初写的是 workspace/，实际落在 container/）
  Dockerfile            # 多阶段：构建 dct → 运行时镜像
  entrypoint.sh         # 起守护进程 + ttyd（只绑内环回）+ 那道门，信号交给 tini -g
  compose.yaml          # 单人版：一条命令起一个能用的远程 dct，也是任务 5 的夹具
  README.md             # 镜像认哪些环境变量和卷（= 契约）；两种代码位置的代价表

src/gate.rs             # 那道门本身。**不在 container/ 里**——它是 dct 的代码，
                        # 第二期（TUI 直连）要原样复用；容器只是它的第一个用处
src/gate.html           # 门口那一页：把 fragment 里的钥匙换成 cookie
```

**旁边那三个 docker 项目都不是这件事的家**，查过了，写下来免得下一个人再查一遍：
`dc_docker_ng`/`dc_docker_ng1` 是两行 pytorch 的 Dockerfile；`dc-docker-on-demand`
是克隆的第三方（CTFd 那套按需起容器的 Python 服务），不是我们写的、也没在动——
将来真需要「网页上点一下发一个容器」时可以回头看它，这一期不用。

---

## Tasks

### 任务 1：镜像骨架，容器里能起守护进程 ✅（2026-09-06）
- [x] 多阶段 Dockerfile：构建阶段编 `dct`，运行时阶段只留二进制 + Node + agent CLI
- [x] 非 root 用户（uid 1000 `dc`），`~/.dct` / `~/.claude` / `~/work` 是卷
- [x] **验收**：`dct ps` 回「No sessions」（守护进程在、socket 通）；进程树是
      `1 tini → 7 dct`，`docker stop` 0.35 秒返回、退出码 143，之后无残留
- [x] 守护进程的 PATH 是 `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`，
      `node`/`npm`/`git`/`qwen`/`claude` 全在里面——**「dct 一行不用改就认得」这条
      成立，实测过了**。socket 是 `srw-------`、目录 `drwx------`，`bind_private`
      在容器里照常生效

**镜像实际 676 MB，估的是 511 MB，差 165 MB。** 差在哪：

| 层 | 大小 | 估算里有没有 |
|---|---|---|
| debian:bookworm-slim 底 | 74.8 MB | ❌ 没算 |
| apt（ca-certificates + git + tini + procps） | 95 MB | ❌ 没算，git 是大头 |
| node 二进制 | 125 MB | 估的 95 MB，低了 |
| npm 自己 | 11.9 MB | ❌ 没算 |
| `npm i -g` 两个 agent CLI | 358 MB | 估的 416 MB，高了 |
| dct 二进制 | 9.5 MB | ✅ |

结论：**估算漏的全是「底座」，不是 agent。** 以后再估容器大小，先记 180 MB 的
debian+apt 底子。

第一次建出来是 684 MB，其中 9.5 MB 是白扔的——`RUN chmod` 会重写文件，那个二进制
在镜像里存了两份。改成 `COPY --chmod=0755` 之后 676 MB。

构建耗时：**cargo release 38 秒**（32 核），整体首次 66 秒、改一行重建 34 秒。
Docker Hub 在这台机器上拉 `debian:bookworm-slim` 用了 5.2 秒，**基础镜像不需要换
国内源**；npm 仍走 `registry.npmmirror.com`，那是为了跟 `runtime.rs` 的
`CN_NPM_REGISTRY` 对齐，不是因为快慢。

**建镜像之前踩的一个坑，记下来免得重来**：Docker 引擎起不来、每个 API 都回 500，
根因是 Windows 服务 `com.docker.service` 停着（不是 WSL、不是网络）。`docker version`
仍能答客户端版本，正是这个形状。

### 任务 2：PID 1 与收尸 ✅（2026-09-07）

- [x] tini 已经在 ENTRYPOINT 上，进程树实测 `1 tini → 7 bash → {daemon, ttyd, gate}`
- [x] 镜像里装了 `procps`——slim 里没有 `ps`/`pgrep`，**没有它这一条根本问不出来**
- [x] **验收**：起一个会话，让父进程先死，孤儿被 reparent 到 PID 1、进程表干净

怎么验的：看板上开一个 shell 会话，在里面 `sleep 400 &`（`&` 让它自己一个
进程组，所以 `dct kill` 打前台组打不到它），然后 `dct kill 2` 强杀会话。
杀之前 `sleep 400` 的 PPID 是 1695（会话那个 bash），杀之后变成 **1**——
reparent 实测到了。全表扫 `Z`：0 个。

**但「没看到僵尸」本身不算验证**，得有反例。第一次构造的反例是错的：让
PID 1 是一个普通 `sh`，结果它照样把孤儿收了——因为它正阻塞在 `wait()` 上
等自己的子进程，而 `wait` 会顺手收掉**任何**一个子进程，包括刚 reparent
过来的。改成「PID 1 派完子进程就 `exec` 成 `sleep`，从此永不 wait」之后，
对比才成立：

| PID 1 | 进程表 | 僵尸 |
|---|---|---|
| `sleep`（不收尸） | `1 sleep`、`7 Z sh`、`8 Z sleep`（PPID 都是 1） | **2 个，永久** |
| `tini -g` | `1 tini`、`7 sleep`、`8 Z sh`（PPID 是 7） | 1 个，**且不是 reparent 来的那个** |

结论钉死：**reparent 到 PID 1 的那个孤儿，只有 PID 1 是 tini 时才会被收掉。**
Global Constraints 里那条「还没在容器里验过」可以划掉了。

**tini 不是万灵药，顺带记下来**：上表 B 里那个剩下的僵尸挂在 pid 7 底下，
tini 收不了——它不是 tini 的孩子。中间那一层自己 spawn 又从不 wait 的话，
照样漏。我们这儿两层中间进程都收：`entrypoint.sh` 最后是 `wait`，守护进程
是 `child.wait()`（`pty.rs` 那条 `no_zombie` 测试盯着）。

### 任务 3：那道门 ✅（2026-09-07）

**这一条最大的发现是它有一半根本不用写。** 计划里第一句写着「token 的产生、
落盘、常数时间校验，写成一个模块」——那个模块三周前就有了，在 `src/web/mod.rs`
里，是手机端那条路踩出来的：`new_token`（32 字节系统随机数）、`ensure_token`
（**是 ensure 不是 new**，否则每次重启已发出的链接集体失效）、`same_secret`
（常数时间）、fragment 换 cookie。**真正缺的是上游**——一个能把 WebSocket
对穿过去的反向代理。所以新代码几乎全在那一半上。

- [x] `src/gate.rs`：一道只做鉴权的反向代理。没带对钥匙 401，带对了退化成
      一根字节管道，`101` 之后的帧原样对穿
- [x] 钥匙复用 `web::new_token` / `web::same_secret`（`pub(crate)` 出来的，
      **整个仓库仍然只有那一份**常数时间比对），但**自己一个键**
      `secrets::GATE_TOKEN_KEY`：手机端那把给的是只读画面加一行输入，
      这把给的是一整台机器上的终端，共用会让「换掉远程的钥匙」顺手把手机踢下线
- [x] 三个坑照抄 `web/mod.rs`：认证在路由之前、401 body 恒为空、常数时间比对
- [x] **进门是一条链接**（`http://…/#t=<钥匙>`），钥匙在 fragment 不在查询串
      ——`web::is_public` 上那段已经把理由写死了：查询串进浏览器历史、进任何
      中间日志，fragment 根本不上行
- [x] **验收**：没带对钥匙的请求一律 401，且从响应上分不出路径存不存在。
      实测 `/token`（真有）、`/ws`（真有）、`/definitely-not-real`（没有）
      三条响应的 md5 完全相同
- [x] **验收**：一条链接点开就能用。实测走了一遍：链接 → 门口那一页 → 换成
      cookie → 重载 → 完整看板，地址栏里干干净净（钥匙被 `replaceState` 抹掉）
- [x] ttyd 退到 `--interface 127.0.0.1`，门是**唯一**对外的进程。这一条现在是
      网络层面成立的，不是靠「我们没映射那个端口」——同一个 docker 网络里的
      别的容器也够不着 ttyd
- [x] 容器重建之后同一条链接照样能用（钥匙在 `~/.dct` 那个卷上），实测

**`--interface` 只收网卡名是我想当然，实测收 IP。** ttyd 的帮助里写的是
`Network interface to bind (eg: eth0)`，我据此在 Dockerfile 注释里写了一句
「拿 -i 当只绑环回用是想当然」——错的。底下的 libwebsockets 同样收 IP，
日志里会打 `lws_socket_bind: source ads 127.0.0.1`。那句注释已经改掉。

**浏览器上试出来的一个洞，单测全绿也照样有。** 已经打开着门口那一页的人，
看到「要一条完整链接」之后最自然的动作是把链接粘进**同一个标签**的地址栏——
而那只改了 fragment，浏览器按同文档导航处理，脚本一次都不会再跑，页面纹丝
不动，他会以为链接也是坏的。补了一个 `hashchange` 监听器。**跟
`web::is_public` 那个洞是同一类**：那边也是「所有单测都是绿的，把页面真的
用浏览器打开一次才发现」。

**明说的两条边界**（模块头里写着，README 里也写着）：

1. **鉴权是一条 TCP 连接查一次**，在它的第一个请求上。之后是纯字节管道，
   不再解析。要每个请求都查就得完整解析 HTTP 的消息边界，而那正是
   `web/mod.rs` 顶上「不引框架」拒绝做的事。不复用、不解析、不回收的管道
   没有请求走私那一类问题。
2. **这道门不做加密。** 明文 HTTP 上 cookie 里那把钥匙每个请求都在线上裸奔。
   所以端口仍然只绑宿主环回，要给真人用必须先在前面架 TLS。跟 `dct-srv` 的
   `must_be_loopback` 是同一条线。

**还剩一样没做，归到任务 6**：换钥匙现在要手编 `secrets.toml`（删掉
`__gate__` 那一项）。撤销是个人主动动作，该有一条命令。

### 任务 4：ttyd 接进来 + 确认零配置上手真的走得通 ✅（2026-09-07，差最后一步）

**已查实，容器里这条路是通的**（写下来免得下一个人再查一遍）：配对是设备码流程，
`pair_view.rs:552` 把 URL 直接画在配对屏上，有测试 `the_url_is_on_screen_not_only_in_the_browser`
盯着；`open_url` 在容器里必然失败，而失败走的是 `cannot_open_browser` 并把 URL 带在
消息里——那是设计好的路，不是异常。学生本来就在浏览器里，新开一个标签批准即可。
钥匙落在 `~/.dct` 那个卷上，容器重建不丢。

- [x] 浏览器打开能看到完整看板。ttyd 1.7.7（GitHub 静态单文件，1.36 MB，
      钉了 sha256）。镜像 676 → 677 MB
- [x] **验收**：关掉浏览器标签、重开，会话还在，画面接得回去。**实测走了一遍**：
      标签一关，TUI 进程立刻没了，会话的 shell（连它自己的后台任务）还挂在守护
      进程底下、`dct ps` 照答；新开标签重进，整屏滚动历史原样接上
- [x] `docker stop` 在两个子进程下仍是 0.575 秒、退出码 143（`tini -g` 起效，
      没走十秒超时）
- [ ] **验收（这一期最重要的一条）**：拿一个没装过任何东西的环境，只给一条链接，
      走完「打开 → 看板 → 配对 → 开出一个能干活的 agent」。**最后一步还没走**
      ——它要一份真的账号凭据，得本人来。前面三步在容器里已经跑通

**接进来之后才看得见的三个坑，都在容器里，dct 一行没改：**

**一、会话里的 `TERM` 是 `dumb`。** 不是「没设」，是实打实的 `dumb`：`tput` 直接
报 "No value for $TERM"，`clear`/`less`/`vi` 全废。守护进程在容器里没有终端可
继承（实测它的 `environ` 里根本没有 `TERM`），portable-pty 于是给子进程填了
`dumb`。桌面上看不到这个坑，因为那边 dct 是从一个真终端里敲起来的。
**修法在镜像里**（`ENV TERM=xterm-256color`），值不用猜——前面就是 ttyd，它报的
正是这一个。

**二、默认项目是家目录，于是 dct 把家目录 `git init` 了。这是安全问题。**
守护进程的 cwd 就是看板里的默认项目，而 `WORKDIR /home/dc` 让它停在了家目录上。
dct 对「还不是 git 仓库」的项目会自动 `git init`（那是撤销能成立的前提，
`09f3edd` 加的），于是 `/home/dc/.git` 出现了。而 `git.rs` 的检查点是
`git add -A`（含未跟踪文件，只尊重 `.gitignore`，新 init 的仓库没有）——
**`~/.dct/secrets.toml` 会被写进快照对象**，`~/.claude` 那 185 MB 每轮重算一遍。
实测证据：`git status` 在那个新仓库里列着 `?? .dct/`。
修法是 `WORKDIR /home/dc/work`，两个状态目录就都在项目之外了。

**三、看板是英文的。** dct 的语言顺序是 `DCT_LANG` > 用户存过的设置 > 系统 locale
> `En`，而 `LANG=C.UTF-8` 认不出主码，落到的是 En。这个镜像服务的是训练营的
学生，默认给 `DCT_LANG=zh`（0 字节，且发到别处 `-e DCT_LANG=en` 就换回来），
不去装 locale。

**顺带排掉一个我差点白做的改动**：`ncurses-base` 不用装。我先查的是
`/usr/share/terminfo`（空的）就以为 slim 里没有 terminfo，其实 Debian 把它放在
`/lib/terminfo`，`debian:bookworm-slim` 自带 ncurses-base 6.4-4，
`TERM=xterm-256color tput colors` 直接答 256。

- [ ] **记下手感上的实际损失**：中文输入法、Ctrl 组合键、复制粘贴各是什么表现。
      **这一条在我这里做不了**——浏览器自动化工具会把 F5 之类的功能键截走当成
      自己的快捷键，测出来的是工具的行为不是 xterm.js 的。dct 把 F2–F6 都用上了
      （F2 回看板、F3 下一个会话、F4 复制、F5 粘贴图片、F6 配色），而浏览器自己
      占着 F5/F6/F11/F12——**这几个到底谁赢，要人在真浏览器上按一遍**。
      渲染这半已经验过：中文、制表符边框、emoji、256 色全正常

### 任务 4.5：容器里粘不了图，而它现在的说法像是用户的错

**顺带一个信号**：Linux 上编 `dct` 会出 4 条 dead-code 警告，全来自这个模块
（`read_failed`、`paste_dir`、`new_png_path` 在非 macOS/Windows 上没人调用）。
警告本身无害，但它正好指着这一条——修这个任务时顺手让它安静下来。

**已查实，不是推断：** `clipboard.rs` 的 `image_to_file` 在
`cfg(not(any(target_os = "macos", windows)))` 那一支直接返回 `Ok(None)`，
调用方于是给出「剪贴板里没有图片」。dct 因此**一行不用改**就能在容器里跑——
但学生截了图想丢给 agent 看的时候，拿到的是一句听起来像是他自己没复制的话。

不是不支持，是**支持得像是用户的错**。这个仓库对这种坏法有过明确立场
（`sys::proc`、`shell.rs` 里都是同一条：达不到同样强度的地方点名说清楚，不假装）。

- [ ] 至少做到不撒谎：这条路径上说的是「这个环境里粘不了图」，不是「剪贴板里没有图片」
- [ ] 查一下浏览器那一层能不能补上（ttyd 收到粘贴事件时能不能把图送进来）。
      **能补就补在这里，补不了就把那句话说准** —— 后者是本任务的最低验收线
- [ ] 顺带记下另外两样在浏览器终端里会变差的东西（中文输入法、Ctrl 组合键），
      跟任务 4 那份手感实测合在一起写进 `workspace/README.md`

### 任务 5：两种代码位置各验一遍 ✅（2026-09-07）

**这一条挖出两个致命的东西，而且都不是「容器里的小毛病」——它们各自让整个
容器里一个 agent 会话都开不出来。** 之前一路顺是因为我测的全是「命令行」
会话：`session.rs:1152` 那个 `if is_agent` 决定了只有 agent 会话才拍检查点，
所以 shell 会话把这两个坑整个绕过去了。

**坑一（dct 的 bug，不是容器的）：没配 git 身份的机器上，一个 agent 会话都
开不出来。** `git.rs` 的检查点最后一步是 `commit-tree`，而它**一定要一个
身份**；拿不到 `user.email` 时 git 去猜 `<用户名>@<主机名>`，主机名没有域
就直接 fatal：

```text
fatal: unable to auto-detect email address (got 'dc@4c2120397151.(none)')
```

容器里这是必然的，**任何刚装完 git、还没跑过 `git config --global user.email`
的机器上也是必然的——那正是这个产品服务的那批人**。而 `Session::create` 里
第一张检查点拍不上就直接拒绝开会话（`Operation::FirstCheckpoint`），界面上
只有一句「拍不了检查点，这个会话没法安全撤销」。

修法：检查点用 **dct 自己的身份**（`CHECKPOINT_IDENTITY`，env 压过配置）。
这也更诚实——那个 commit 挂在 `refs/dct/` 底下，用户的 `git log` 里看不见，
它不该顶着用户的名字。用户（或 agent）自己敲的 `git commit` 不受影响。

**这个 bug 藏得住，是因为测试夹具替被测代码把条件抹平了。** `git.rs` 的
`init_repo()` 里有两行 `git config user.email/user.name`——现场大量机器上
根本没有那两行。补了两条测试，其中
`a_checkpoint_is_signed_by_dct_not_by_whoever_owns_the_machine` 是真正钉住
修复的那条：在一台配了全局身份的开发机上，撤掉修复它照样红。

**坑二（容器的，不是 dct 的）：绑定挂载那条路上 git 拒绝干活。**

```text
fatal: detected dubious ownership in repository at '/home/dc/work'
```

Docker Desktop 把宿主目录挂进来时全显示成 `root:root 0777`，容器里跑的是
dc(1000)。后果同上——一个 agent 会话都开不出来。

修在**镜像**里（`git config --system --add safe.directory '*'`），不修在 dct
里：那个检查防的是「多用户机器上别人的仓库里藏着 hook」，而这个镜像的
Global Constraints 第一条就是**一人一容器**，它防的事在这里不存在；反过来
让 dct 给每条 git 命令加 `-c safe.directory`，等于把用户**真实的多用户机器**
上那道检查也关掉。**这一行的安全性完全建立在「一人一容器」上**，那条约束
哪天破了，先回来删它。

- [x] 命名卷：开 agent 会话 → 改文件 → `d` 看改动 → `u` 撤销。实测
      `hello.txt` 回到改之前，快照之后新建的文件被清掉
- [x] 绑定挂载：同一套，而且是**在宿主那一侧**看结果——绑定挂载的意义就在
      这儿。同样全过
- [x] 检查点署名实测是 `dct <dct@localhost>`
- [x] ~~podman 也要测一遍~~ **2026-09-07 决定：不测，只支持 docker。**
      原计划里那条理由（「podman 本身就是个真实的运行目标」）现在不成立——
      运行目标由部署方定，而部署方用的是 docker。**代价说清楚**：rootless
      podman 的 uid 映射咬的正是上面坑二那条（`safe.directory` 那一行是按
      docker 的 `root:root 0777` 挂法来的），所以哪天真要上 podman，这一条
      必须回来重跑，不能拿上面 docker 的结论顶。

**两个预料中的坑，一个成立一个不成立：**

- **文件权限：成立，而且比预想的更细。** 0777 的绑定挂载让 git 把每个文件都
  记成 `100755`。容器里前后一致所以检查点和撤销不受影响，但同一个仓库在宿主
  上用真 git 打开，会看到**每个文件都是模式变更**。
- **换行（`core.autocrlf`）：没咬。** 容器里的 git 没设 `core.autocrlf`
  （Linux 默认 false），CRLF 文件在快照里是逐字节原样的 `\r\n`，`d` 也没把
  它误报成改动。风险其实在**宿主**那一侧：宿主的 git 如果 `autocrlf=true`，
  会在 agent 干活的同时改写工作区文件。

**还有一件事没定，留给你**：容器里没有 git 身份，所以**学生（或 agent）自己
敲 `git commit` 会失败**。检查点已经不受影响了，但真提交仍然要身份。
在镜像里塞一个默认署名会把假名字永久写进他的提交历史；不塞就是一句英文报错。
我倾向不塞、写进文档，但这是产品决定，没替你做。

### 任务 6：一人一容器（本仓库这一半）
- [ ] 单人版 `compose.yaml` 里的内存 limit 按「一个 agent 320 MB × 几个」定，
      不拍脑袋
- [ ] **起一份就产出一条能直接发出去的链接。** 这样「发三十个学生」就是跑三十次，
      不需要任何别的仓库——边界不变，但单人版必须自足到这个程度，否则这个产品对
      它最重要的那批用户是不可用的
- [ ] **本仓库到此为止。** 「三十个学生怎么发、反代、证书、配额」是部署方的事
      （见上面那节）。本仓库这边只再交付一样：`container/README.md` 里那份
      「镜像认哪些环境变量和卷」的清单——那就是契约本身
- [ ] **验收**：两个容器之间互相看不见对方的会话和密钥，用单人版 compose 起两份
      就能验，不需要任何别的仓库

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

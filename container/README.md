# dc-workspace —— 浏览器里的 dct

学生本地什么都不装，打开一个地址，看到的就是完整的 dct 看板，agent 跑在服务器上。
关掉浏览器，会话照跑。

服务的是**压根没有合适环境的那批人**：老机器、没有管理员权限的公司电脑、
Chromebook、平板。根目录 README 里那张硬件表对他们是一句「你不合格」，
这个镜像是那张表的出口。

计划和进度：[`../docs/superpowers/plans/2026-09-06-dc-workspace-phase1.md`](../docs/superpowers/plans/2026-09-06-dc-workspace-phase1.md)

## 起一份

```sh
docker compose -f container/compose.yaml up -d --build
```

然后浏览器打开 <http://127.0.0.1:7681>。

停掉、连状态一起删掉：

```sh
docker compose -f container/compose.yaml down     # 留着卷，下次起来东西还在
docker compose -f container/compose.yaml down -v  # 连卷一起删，全清
```

## ⚠️ 现在还**没有**那道门

ttyd 前面此刻什么都没有。**谁连上那个端口，谁就拿到一个能在容器里执行任意
命令的终端**，身份是容器里那个用户。所以 `compose.yaml` 里端口是绑在
`127.0.0.1` 上的，不是 `0.0.0.0`。

在鉴权（计划里的任务 3）做完之前：

- 别把这个端口直接挂到局域网或公网上；
- 真要给别人用，自己在前面架一层带鉴权的反代。ttyd 认 `-H/--auth-header`，
  可以把鉴权整个交给反代。

这跟 `dct-srv` 里 `must_be_loopback` 拦的是同一件事，理由也是同一条。

## 契约：镜像只认这几样

**这个镜像不知道自己被起了几份、挂了什么卷、前面有什么反代。** 它只认下面
这张表。加一样就是改契约，改 Dockerfile 的时候要同时改这里。

### 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `HOME` | `/home/dc` | `~/.dct` 下面那一堆（`secrets.toml` / `projects.json` / `last-sessions.toml`）全靠它推出来 |
| `LANG` | `C.UTF-8` | 不给就是 POSIX，中文在看板里变问号 |
| `TERM` | `xterm-256color` | **别删。**守护进程在容器里没有终端可继承，不给的话会话里是 `dumb`：`tput` 报错、`clear`/`less`/`vi` 全废 |
| `DCT_LANG` | `zh` | 看板语言。发到别处用 `-e DCT_LANG=en`。学生在界面里改过之后以存的为准 |
| `DCW_PORT` | `7681` | ttyd 监听的端口。**只有端口，没有绑定地址**——限制谁连得上是宿主侧端口映射的事 |

### 卷

| 路径 | 丢了会怎样 |
|---|---|
| `/home/dc/.dct` | 密钥、项目列表、上次的会话全没了 |
| `/home/dc/.claude` | 对话记录没了，`--continue` 接不回去。会长，实测一周 185 MB |
| `/home/dc/work` | 学生的代码 |

### 端口

容器里 `7681`（HTTP + WebSocket）。

### 构建参数

| 参数 | 默认 | 用途 |
|---|---|---|
| `NPM_REGISTRY` | `https://registry.npmmirror.com` | 跟 `runtime.rs` 的 `CN_NPM_REGISTRY` 保持同一个源 |
| `TTYD_URL` / `TTYD_SHA256` | 空（走 GitHub） | 换国内镜像时**两个一起给**。只给 URL 不给哈希 = 谁给什么装什么 |
| `RUST_TAG` | `1-slim-bookworm` | 构建阶段的 rust 镜像 |

基础镜像不需要换国内源：Docker Hub 在这里拉 `debian:bookworm-slim` 是 5 秒。
GitHub 拉 ttyd（1.36 MB）也通，网络不行的地方才需要 `TTYD_URL`。

只钉了 **amd64** 的 ttyd 校验和。arm64 要自己下 `ttyd.aarch64` 算出 sha256
用 `--build-arg` 传进来——不传的话构建会直接失败并说明原因，不会悄悄建出一个
跑不起来的镜像。

## 代码放哪儿：两种，两套代价

`git.rs` 的检查点是守护进程在项目目录上做的，守护进程在容器里，所以被快照的
目录必须在容器里看得见。

| | 命名卷（默认，推荐给训练营） | 绑定挂载宿主目录 |
|---|---|---|
| IO | 原速，git 检查点原速 | Windows/macOS 上慢，编译和 `node_modules` 明显拖 |
| 改代码 | VS Code Dev Containers / Remote-SSH，或干脆只让 agent 改 | 本地编辑器照常 |
| 咬人的地方 | 宿主上看不到文件，要导出得走命令 | 换行（`core.autocrlf`）和文件权限会咬 git 检查点 |

换成绑定挂载：把 `compose.yaml` 里 `- work:/home/dc/work` 改成
`- ./work:/home/dc/work`。

**两套路径上的检查点和撤销还没各跑一遍**（计划里的任务 5）。撤销是敢关掉权限
确认的全部前提，哪条路径上它是死的，那条路径就不能发——所以这一条验完之前，
上面这张表只是设计意图，不是实测结论。

## 已知会变差的地方

- **粘图片。** `clipboard.rs` 在 Linux 上直接返回「没有图片」，学生截了图丢给
  agent 会得到一句听起来像是他自己没复制的话。计划里的任务 4.5。
- **中文输入法、Ctrl 组合键、复制粘贴。** 浏览器终端跟本地终端不是一回事，
  这三样具体差多少要人在真设备上试，还没试。
- **一个容器一个人。** 谁连得上 `~/.dct/daemon.sock`，谁就能以那个身份执行任意
  命令。一个容器塞多个学生 = 互相能进对方的会话、读对方的密钥。这条不做取舍。

## 内存

一个 agent 常驻约 320 MB（实测），容器底子约 300 MB。`compose.yaml` 里
`mem_limit: 2g` 是按「一个人同时开三个 agent 还有余量」定的。

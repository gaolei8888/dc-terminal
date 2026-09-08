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

然后从日志里拿那条链接（下一节），用浏览器打开它。**直接打开
<http://127.0.0.1:7681> 只会看到「要一条完整链接」**——门在那儿。

停掉、连状态一起删掉：

```sh
docker compose -f container/compose.yaml down     # 留着卷，下次起来东西还在
docker compose -f container/compose.yaml down -v  # 连卷一起删，全清
```

## 进门要一条链接

起来之后，钥匙和链接印在日志里：

```sh
docker compose -f container/compose.yaml logs dc | grep '#t='
# http://127.0.0.1:7681/#t=<64 个十六进制字符>
```

**那条链接本身就是钥匙。** 点开一次就换成一个 30 天的 cookie，地址栏里那一截
随即被抹掉，之后刷新、重开浏览器都不用再找它。别把它发到公开的地方，也别把
容器日志贴出去。

只想重新拿一次链接（不重启）：

```sh
docker compose -f container/compose.yaml exec dc dct gate --link --url http://127.0.0.1:7681
```

**换钥匙**（作废所有已经发出去的链接）：删掉 `~/.dct/secrets.toml` 里的
`__gate__` 那一项再重启容器。还没有一条专门的命令，那在计划的任务 6 里。

门的行为，一句话：没带对钥匙的请求一律 401，且**从响应上分不出路径存不存在**
（`/ws`、`/token`、`/随便什么` 三条响应逐字节相同）。唯一的例外是 `GET /`
——它答的是门口那一页，因为钥匙在 fragment 里，而 fragment 根本不发给服务器，
把 `/` 也锁上就成了一个自己锁死自己的循环。那一页里没有任何用户数据。

## ⚠️ 但它**不加密**

明文 HTTP 上，cookie 里那把钥匙每个请求都在线上裸奔。所以 `compose.yaml` 里
端口仍然绑在 `127.0.0.1`，不是 `0.0.0.0`。

要给别人用：

1. 在前面架一层带 TLS 的反代；
2. 把 `DCW_PUBLIC_URL` 指到反代那个地址上（门用它拼链接，容器自己不可能
   知道外面看到的是什么地址）；
3. 端口映射改成只让反代够得着。

这跟 `dct-srv` 里 `must_be_loopback` 拦的是同一件事，理由也是同一条：
没有加密之前，不许当成对外可用。

另外两条明说的边界：

- **鉴权是一条 TCP 连接查一次**，在它的第一个请求上。之后就是一根字节管道。
  拿到过钥匙的人可以在同一条连接上继续发请求——而他本来就有钥匙。
- **ttyd 只绑容器内环回**（`--interface 127.0.0.1`），门是唯一对外的进程。
  这一条是网络层面成立的，不是靠「我们没映射那个端口」。

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
| `DCW_PORT` | `7681` | **对外**那个端口，`dct gate` 守着它。不是 ttyd 的端口 |
| `DCW_TTYD_PORT` | `7682` | ttyd 在容器内环回上监听哪个。基本不用动 |
| `DCW_PUBLIC_URL` | 空 | 外面看到的地址前缀。门用它拼那条发得出去的链接；不给就按容器网桥地址猜一条，并明说是猜的 |

### 卷

| 路径 | 丢了会怎样 |
|---|---|
| `/home/dc/.dct` | 密钥、项目列表、上次的会话全没了 |
| `/home/dc/.claude` | 对话记录没了，`--continue` 接不回去。会长，实测一周 185 MB |
| `/home/dc/work` | 学生的代码 |

### 端口

容器里 `7681`：**只有那道门**在上面（HTTP + WebSocket）。ttyd 在 `7682`，
只绑容器内环回，从容器外和同网络的别的容器都够不着。

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

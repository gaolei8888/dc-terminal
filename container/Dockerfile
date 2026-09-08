# syntax=docker/dockerfile:1
#
# dc-workspace：把 dct 和它要用的一切装进一个镜像。
#
# 计划：`docs/superpowers/plans/2026-09-06-dc-workspace-phase1.md`
# 任务 4 + 任务 3：守护进程 + ttyd（只绑容器内环回）+ `dct gate`（唯一对外的
# 那个端口，只放带对钥匙的人进去）。浏览器里就是完整的 dct 看板。
# **门不加密**——见 `compose.yaml` 里端口那段：前面没有 TLS 之前，
# 这个端口只许绑在宿主的环回上。
#
# 契约（计划里那一条，写在这里是因为改 Dockerfile 的人最容易破它）：
# **这个镜像不知道自己被起了几份、挂了什么卷、前面有什么反代。**
# 它只认下面 ENV 和 VOLUME 声明的那几样。

# ---------------------------------------------------------------------------
# 构建阶段
# ---------------------------------------------------------------------------
# 在 Linux 容器里编，所以开发机是 Windows 也**不需要任何交叉编译工具链**。
# 仓库里 Windows 专属的依赖（`uds_windows`、`windows-sys`）都在
# `cfg(windows)` 门里，Linux 这边根本不会被拉进来。
ARG RUST_TAG=1-slim-bookworm
FROM rust:${RUST_TAG} AS build

WORKDIR /src
COPY . .

# 用 BuildKit 的缓存挂载，而不是「先拷 Cargo.toml 建一个假 main.rs」那套。
# 后者在 workspace 里要给四个 crate 各造一份假源码，多出来的那些文件迟早跟
# 真的 members 列表走岔；缓存挂载没有这个问题，代价只是这一层不进镜像层缓存
# （而它本来也不该进——`target/` 是 45 GB）。
#
# `--bin dct`：这个 workspace 里 `dct-srv` 也有 main，不点名会连它一起编，
# 而中转服务不属于这个镜像。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --bin dct \
 && mkdir -p /out && cp target/release/dct /out/dct

# ---------------------------------------------------------------------------
# Node 从官方镜像里搬，不走 apt 也不走 nodesource
# ---------------------------------------------------------------------------
# 理由跟 `runtime.rs` 里那套「自己下一份 Node」是同一条：要的是一份确定的、
# 跟发行版无关的运行时。差别只在时机——那边是用户机器上运行时下载，这里是
# 构建期一次装好。三十个学生同时开容器各下 500 MB 是另一回事。
FROM node:22-bookworm-slim AS node

# ---------------------------------------------------------------------------
# ttyd：把那个终端搬进浏览器的那一层
# ---------------------------------------------------------------------------
# **不走 apt**——Debian bookworm 里根本没这个包（`apt-cache policy ttyd` 是空表，
# 查过了）。官方发布的是一个静态链接的单文件，1.36 MB，没有运行时依赖。
#
# **校验和钉死，理由不是防 GitHub 变卦**（release 资产本来就不可变），而是：
# 下面那个 URL 允许用 `--build-arg TTYD_URL=` 换成国内镜像。URL 换了、校验和
# 没换，取回来的东西不对就当场失败。没有这一条，「换个源」就等于「谁给我
# 什么我装什么」。
#
# **所以换源时只要给 URL，不用再给哈希**：amd64 那一个是同一份文件，哈希不变。
# 这是有意的——换镜像的人不该还得自己去算一遍一个已知文件的哈希，那既麻烦又
# 正好给了他一个「算不出来就随便填一个」的机会。
#
# 2026-09-08 在国内的机器上实测：直连 GitHub 会在 302 之后卡死
# （objects.githubusercontent.com 拉不动），下面这两个都通、都是 1362040 字节、
# 哈希对得上：
#   --build-arg TTYD_URL=https://gh-proxy.com/https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.x86_64
#   --build-arg TTYD_URL=https://ghfast.top/https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.x86_64
# 这类代理来来去去，能随便换正是因为哈希钉死了。
#
# 单独开一个阶段，是为了把 curl 和它那串 apt 留在这里、不进运行镜像。
FROM debian:bookworm-slim AS ttyd
ARG TARGETARCH
# **换 Debian 源，不换就慢到没法用。**
#
# 2026-09-08 在目标机器（国内，电信）上实测：**一个 `apt-get update && install`
# 步骤 1343 秒（22 分钟）**，而这样的步骤有两个。走 Debian 官方源就是这个数。
#
# 这跟 `NPM_REGISTRY` 是同一条：源的选择是**构建参数**，不写死。默认留空
# （官方源），要快就传一个，例如：
#   --build-arg APT_MIRROR=http://mirrors.aliyun.com/debian
#
# **必须是 `http://`，不能是 `https://`。** 这一步要装的正是
# `ca-certificates`——在它装上之前，apt 验不了任何 TLS 证书，给 https 的源
# 会直接失败（实测）：
#   Certificate verification failed: The certificate is NOT trusted.
# Debian 自己的默认源用 http 也是这个原因。安全性不靠 TLS：apt 验的是每个
# 包的 GPG 签名，换 http 不降低这一条。
#
# bookworm 的源在 deb822 格式的 `debian.sources` 里，老格式的 `sources.list`
# 也一并替换——两个文件哪个存在就改哪个，都不存在也不报错。
ARG APT_MIRROR=""
RUN set -eu; \
    if [ -n "$APT_MIRROR" ]; then \
      for f in /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list; do \
        [ -f "$f" ] || continue; \
        sed -i "s|https\?://deb.debian.org/debian|$APT_MIRROR|g; \
                s|https\?://security.debian.org/debian-security|$APT_MIRROR-security|g" "$f"; \
      done; \
    fi

ARG TTYD_VERSION=1.7.7
ARG TTYD_URL=""
ARG TTYD_SHA256=""
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*
# 只钉了 amd64 那一个哈希——那是真下过、真算过的。别的架构不假装支持：这里
# 直接失败并说清怎么办，好过建出一个装着 x86 二进制、跑起来才报
# "exec format error" 的 arm 镜像。arm64 的资产叫 ttyd.aarch64。
RUN set -eu; \
    url="$TTYD_URL"; sha="$TTYD_SHA256"; \
    case "${TARGETARCH:-amd64}" in \
      amd64) \
        [ -n "$url" ] || url="https://github.com/tsl0922/ttyd/releases/download/${TTYD_VERSION}/ttyd.x86_64"; \
        [ -n "$sha" ] || sha="8a217c968aba172e0dbf3f34447218dc015bc4d5e59bf51db2f2cd12b7be4f55" ;; \
      *) \
        if [ -z "$url" ] || [ -z "$sha" ]; then \
          echo "本 Dockerfile 只钉了 amd64 的 ttyd 校验和，当前是 ${TARGETARCH}。" >&2; \
          echo "自己下一份算 sha256，然后 --build-arg TTYD_URL=... --build-arg TTYD_SHA256=..." >&2; \
          exit 1; \
        fi ;; \
    esac; \
    # `--retry`：目标机器上实测这一步会**断流**（curl 退出码 18，
    # "not closed cleanly before end of the underlying stream"），跨境链路上
    # 是常态。断了重来比整个构建失败便宜得多。
    curl -fsSL --retry 5 --retry-delay 2 --retry-all-errors -o /ttyd "$url"; \
    echo "$sha  /ttyd" | sha256sum -c -


# ---------------------------------------------------------------------------
# 运行镜像
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# npm 默认走国内源。`registry.npmmirror.com` 不是随手挑的——`runtime.rs`
# 的 `CN_NPM_REGISTRY` 就是它，两处保持同一个源，学生在容器里和在本地装到的
# 是同一份东西。要换源：`--build-arg NPM_REGISTRY=...`。
ARG NPM_REGISTRY=https://registry.npmmirror.com

# **换 Debian 源，不换就慢到没法用。**
#
# 2026-09-08 在目标机器（国内，电信）上实测：**一个 `apt-get update && install`
# 步骤 1343 秒（22 分钟）**，而这样的步骤有两个。走 Debian 官方源就是这个数。
#
# 这跟 `NPM_REGISTRY` 是同一条：源的选择是**构建参数**，不写死。默认留空
# （官方源），要快就传一个，例如：
#   --build-arg APT_MIRROR=http://mirrors.aliyun.com/debian
#
# **必须是 `http://`，不能是 `https://`。** 这一步要装的正是
# `ca-certificates`——在它装上之前，apt 验不了任何 TLS 证书，给 https 的源
# 会直接失败（实测）：
#   Certificate verification failed: The certificate is NOT trusted.
# Debian 自己的默认源用 http 也是这个原因。安全性不靠 TLS：apt 验的是每个
# 包的 GPG 签名，换 http 不降低这一条。
#
# bookworm 的源在 deb822 格式的 `debian.sources` 里，老格式的 `sources.list`
# 也一并替换——两个文件哪个存在就改哪个，都不存在也不报错。
ARG APT_MIRROR=""
RUN set -eu; \
    if [ -n "$APT_MIRROR" ]; then \
      for f in /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list; do \
        [ -f "$f" ] || continue; \
        sed -i "s|https\?://deb.debian.org/debian|$APT_MIRROR|g; \
                s|https\?://security.debian.org/debian-security|$APT_MIRROR-security|g" "$f"; \
      done; \
    fi

# `git` 是硬依赖，不是可选：dct 的检查点和撤销全靠它（`git.rs`），而撤销是
# 敢让 agent 不问就动手的全部前提。
# `tini` 见 `entrypoint.sh` 里那段——任务 2 会真正验它。
# `procps` 给的是 `ps`/`pgrep`。它不是「方便调试」——**任务 2 的验收条件是
# 「确认孤儿被 reap 掉、进程表干净」，没有 ps 就问不出这句话**。第一次建完
# 镜像去查进程树时才发现 slim 里没有，记在这儿免得有人当成冗余给删掉。
# **terminfo 不用装，已经在里面了**：debian:bookworm-slim 自带 ncurses-base 6.4-4，
# `TERM=xterm-256color tput colors` 直接答 256。写在这儿是因为我差点白加一个包：
# 我先查的是 `/usr/share/terminfo`（空的），而 Debian 把库放在 `/lib/terminfo`。
# dct 自己本来也不查 terminfo（走 ratatui/crossterm），要它的是看板里开出来的
# 那种 shell 会话——`clear`、`less`、`vi`。两边都已经满足，这一条不用做任何事。
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git tini procps \
 && rm -rf /var/lib/apt/lists/*

# **绑定挂载那条路上，git 会拒绝干活。** 实测（2026-09-07）：Docker Desktop
# 把宿主目录挂进来时全部显示成 `root:root 0777`，而容器里跑的是 dc(1000)，
# 于是每一条 git 命令都是
#
#   fatal: detected dubious ownership in repository at '/home/dc/work'
#
# 后果跟 git 身份那条一样重：第一张检查点拍不上，`Session::create` 直接拒绝，
# **一个 agent 会话都开不出来**。命名卷那条路没这个问题（那个目录属于 dc）。
#
# **修在镜像里，不修在 dct 里。** 那个检查防的是「多用户机器上，别人的仓库
# 里藏着 hook 和 config」——而这个镜像的 Global Constraints 第一条就是
# **一人一容器**：这里面只有一个用户，它防的那件事在这里不存在。
# 反过来，让 dct 给每条 git 命令都加 `-c safe.directory`，就等于在**用户
# 真实的多用户机器上**也把这道检查关掉了，那是另一回事。
#
# **这一行的安全性完全建立在「一人一容器」上。** 哪天那条约束破了，
# 先回来删掉这一行。
RUN git config --system --add safe.directory '*'

# **学生自己敲 `git commit` 也要有个身份，否则报的是一句英文。**
#
# 这跟上面 dct 检查点那件事**不是**同一回事：检查点用的是 dct 自己的署名
# （`git.rs` 的 `CHECKPOINT_IDENTITY`，环境变量压过一切配置），跟这里配什么
# 无关，也不会被这两行影响。这两行只管学生（和 agent）自己发起的提交。
#
# 之前不敢配，怕把一个假名字永久写进他真实的提交历史。**2026-09-07 定了：
# 这一期学生的 git 只做本地提交**，不往任何地方推，那份历史不会流到别人
# 那里去，于是「先让他 commit 得成」压过「署名要准」。
#
# 落在 `--system`（`/etc/gitconfig`），不是 `--global`：学生哪天自己
# `git config --global user.email` 了，他那份就赢——这两行只是在他还没配的
# 时候兜住，不会盖掉他的。
#
# 名字取得一眼就看得出是占位符：读 `git log` 的人应该立刻想到「这台机器没
# 设过名字」，而不是去猜「dc 是谁」。
RUN git config --system user.name "dc-workspace user" \
 && git config --system user.email "user@dc-workspace.local"

COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# 装到系统 PATH 上，**不是**装进 `~/.dct/runtime`。
# 这一条是「加一个目录就够了」成立的关键：`runtime::runtime_bin_dirs` 只把
# 真实存在的目录挂上 PATH，`~/.dct/runtime` 空着时 `activate` 是空操作，dct
# 就退回系统 PATH——而 `command_exists` 查的正是 PATH。所以 dct 一行不用改
# 就认得这里装的东西。
#
# 两个：`@qwen-code/qwen-code` 是 DC（默认那个）要的，
# `@anthropic-ai/claude-code` 是 claude profile 要的。
RUN npm config set registry "$NPM_REGISTRY" \
 && npm i -g @qwen-code/qwen-code @anthropic-ai/claude-code \
 && npm cache clean --force

# **权限位写在 COPY 上，不用事后 `RUN chmod`。** 两件事：
#
# 一、必须显式给，不能指望源文件——镜像在 Windows 上构建，而 Windows 的构建
# 上下文没有 Unix 权限位，COPY 进来的脚本是 0644，`tini -- entrypoint.sh` 会
# 直接 Permission denied。仓库里那个文件确实有 +x、git 也记着，但它到不了这里。
#
# 二、**用 `--chmod` 而不是 `RUN chmod`**：chmod 会重写文件，于是那个 9.54 MB
# 的二进制在镜像里存了两份。第一次建出来 684 MB，`docker history` 里明晃晃两条
# 9.54 MB——白扔的。
COPY --from=build --chmod=0755 /out/dct /usr/local/bin/dct
COPY --from=ttyd --chmod=0755 /ttyd /usr/local/bin/ttyd
COPY --chmod=0755 container/entrypoint.sh /usr/local/bin/entrypoint.sh

# 不以 root 跑：agent 会在项目目录里写文件，root 写出来的东西在绑定挂载那条
# 路上会变成宿主上删不掉的。uid 固定 1000，绑定挂载时好对。
#
# **建目录并交给 dc，放在 `USER` 之前**：下面 VOLUME 声明的三个路径，第一次
# 起容器时由 Docker 按镜像里该路径的属主创建。这里不先建好、不 chown，卷就会
# 是 root 的，而容器里跑的是 dc——学生第一次开会话就写不进去。
RUN useradd -m -u 1000 -s /bin/bash dc  && mkdir -p /home/dc/.dct /home/dc/.claude /home/dc/work  && chown -R dc:dc /home/dc

USER dc
# **WORKDIR 必须是 ~/work，不能是 ~。这一条是安全问题，不是整洁问题。**
# 守护进程的 cwd 就是看板里的默认项目，而 dct 对「还不是 git 仓库」的项目会
# 自动 `git init`（那是撤销能成立的前提）。cwd 停在 /home/dc 的话，被 init 的
# 就是家目录本身，而 `git.rs` 的检查点是 `git add -A`（含未跟踪文件）——
# **`~/.dct/secrets.toml` 会被写进快照对象**，`~/.claude` 那 185 MB 对话记录
# 每轮都要重算一遍。2026-09-07 实测：cwd=/home/dc 时 `git status` 在新建的
# 仓库里已经列着 `?? .dct/`。挪到 ~/work 之后，两个状态目录都在项目之外。
WORKDIR /home/dc/work

# 状态必须落在卷上，两处缺一个学生就丢东西：
#   ~/.dct    —— secrets.toml / projects.json / last-sessions.toml /
#                pair-models.toml。它们全靠 `store_path_for_socket` 从 socket
#                路径推出来，所以挂在这一个目录上就一次性全覆盖。
#   ~/.claude —— 对话记录，会长（实测一周 185 MB），也决定 `--continue`
#                还接不接得回去。
#   ~/work    —— 学生的代码。命名卷还是绑定挂载由起容器的人决定，见任务 5。
VOLUME ["/home/dc/.dct", "/home/dc/.claude", "/home/dc/work"]

# **下面这几样就是契约本身**（计划里那句「镜像只认环境变量和卷路径」）。
# 加一样就是改契约，得同时改 container/README.md。
#   HOME      —— `store_path_for_socket` 靠它推出 ~/.dct 里的一切
#   LANG      —— 不给就是 POSIX，中文在看板里直接变问号
#   TERM      —— **不给的话会话里是 `dumb`**，不是「没设」。守护进程在容器里
#                没有终端可继承（实测它的 environ 里根本没有 TERM），
#                portable-pty 于是给子进程填了 `dumb`：`tput` 直接报错、
#                `clear`/`less`/`vi` 全废、agent 也认不出这是彩色终端。桌面上
#                看不到这个坑，因为那边 dct 是从一个真终端里敲起来的。填什么
#                值不用猜——前面就是 ttyd，它报的正是 xterm-256color。
#   DCT_LANG  —— 看板语言。dct 的顺序是 DCT_LANG > 用户存过的设置 > 系统
#                locale > En，而 LANG=C.UTF-8 认不出主码，落到的是 **En**。
#                这个镜像服务的是训练营的学生，默认给 zh；发到别处就
#                `-e DCT_LANG=en`。学生在界面里改过之后以存的为准。
#   DCW_PORT  —— **对外**那个端口，`dct gate` 守着它。它不是 ttyd 的端口：
#                ttyd 只绑容器内环回（见 entrypoint.sh），门是唯一对外的
#                入口，而且这一条是网络层面成立的，不是靠「我们没映射它」。
#   DCW_TTYD_PORT —— ttyd 在容器内环回上监听哪个端口。基本不用动，留着是
#                因为「门和上游是两个端口」这件事得能看见。
#   DCW_PUBLIC_URL —— 外面看到的地址前缀（`https://box.example.com`）。门用
#                它拼那条能直接发出去的链接。**容器不知道自己被映射到哪儿**
#                ——这正是那条契约（镜像不知道自己被起了几份）的落点。
#                不给的话门会按容器网桥地址猜一条，并明说那是猜的。
ENV HOME=/home/dc \
    LANG=C.UTF-8 \
    DCW_PORT=7681 \
    TERM=xterm-256color \
    DCT_LANG=zh \
    DCW_TTYD_PORT=7682

EXPOSE 7681

# `tini -g`：信号发给子进程**所在的进程组**，不只是那一个子进程。任务 1 那会儿
# entrypoint 是 `exec dct daemon`，底下只有一个进程，`-g` 没意义。现在它下面
# 挂着守护进程、ttyd、门三个，没有 `-g` 的话 `docker stop` 只打到中间那个
# shell，那三个要等十秒超时后被 SIGKILL。理由的另一半（凭什么说那几个进程
# 确实在同一个组里）写在 entrypoint.sh 里。
ENTRYPOINT ["/usr/bin/tini", "-g", "--", "/usr/local/bin/entrypoint.sh"]

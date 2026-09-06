# syntax=docker/dockerfile:1
#
# dc-workspace：把 dct 和它要用的一切装进一个镜像。
#
# 计划：`docs/superpowers/plans/2026-09-06-dc-workspace-phase1.md`
# 这一份是**任务 1 的骨架**——容器里能起守护进程，`dct ps` 连得上。
# ttyd（把终端搬进浏览器）和那道门是任务 3、4，还没在这儿。
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
# 运行镜像
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# npm 默认走国内源。`registry.npmmirror.com` 不是随手挑的——`runtime.rs`
# 的 `CN_NPM_REGISTRY` 就是它，两处保持同一个源，学生在容器里和在本地装到的
# 是同一份东西。要换源：`--build-arg NPM_REGISTRY=...`。
ARG NPM_REGISTRY=https://registry.npmmirror.com

# `git` 是硬依赖，不是可选：dct 的检查点和撤销全靠它（`git.rs`），而撤销是
# 敢让 agent 不问就动手的全部前提。
# `tini` 见 `entrypoint.sh` 里那段——任务 2 会真正验它。
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git tini \
 && rm -rf /var/lib/apt/lists/*

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

COPY --from=build /out/dct /usr/local/bin/dct
COPY container/entrypoint.sh /usr/local/bin/entrypoint.sh

# **必须显式 chmod，不能指望源文件的权限位。** 这个镜像是在 Windows 上构建的
# （见计划里那节），而 Windows 的构建上下文没有 Unix 权限位——COPY 进来的
# 脚本是 0644，`tini -- entrypoint.sh` 会直接 Permission denied。仓库里那个
# 文件确实有 +x，git 也记着，但它到不了这里。
RUN chmod 0755 /usr/local/bin/dct /usr/local/bin/entrypoint.sh

# 不以 root 跑：agent 会在项目目录里写文件，root 写出来的东西在绑定挂载那条
# 路上会变成宿主上删不掉的。uid 固定 1000，绑定挂载时好对。
#
# **建目录并交给 dc，放在 `USER` 之前**：下面 VOLUME 声明的三个路径，第一次
# 起容器时由 Docker 按镜像里该路径的属主创建。这里不先建好、不 chown，卷就会
# 是 root 的，而容器里跑的是 dc——学生第一次开会话就写不进去。
RUN useradd -m -u 1000 -s /bin/bash dc  && mkdir -p /home/dc/.dct /home/dc/.claude /home/dc/work  && chown -R dc:dc /home/dc

USER dc
WORKDIR /home/dc

# 状态必须落在卷上，两处缺一个学生就丢东西：
#   ~/.dct    —— secrets.toml / projects.json / last-sessions.toml /
#                pair-models.toml。它们全靠 `store_path_for_socket` 从 socket
#                路径推出来，所以挂在这一个目录上就一次性全覆盖。
#   ~/.claude —— 对话记录，会长（实测一周 185 MB），也决定 `--continue`
#                还接不接得回去。
#   ~/work    —— 学生的代码。命名卷还是绑定挂载由起容器的人决定，见任务 5。
VOLUME ["/home/dc/.dct", "/home/dc/.claude", "/home/dc/work"]

ENV HOME=/home/dc \
    LANG=C.UTF-8

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]

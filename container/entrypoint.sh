#!/bin/sh
# dc-workspace 的 PID 1 下面那一层。
#
# **为什么是 `exec` 而不是「起在后台再 wait」**：`exec` 之后守护进程直接顶替
# 这个 shell，`docker stop` 发的 SIGTERM 就落在它自己身上。中间垫一个 shell 的
# 话，信号停在 shell 上，守护进程要等到十秒超时后被 SIGKILL 才知道。
#
# **但别把这里发生的事说大了**（2026-09-06 实测，退出码 143 = 128+15）：
# `daemon.rs` 里**没有** SIGTERM 处理器——`sys::signal` 那一套是给 TUI 还原
# 终端用的，守护进程一次都没调过。所以它是被 SIGTERM 的默认动作直接打死的，
# 并没有「自己把 pty 收拾干净」。agent 照样会被清掉，但那是**内核**关掉 pty
# 主端之后发的 SIGHUP（`sys::job` 模块头描述的正是这条 Unix 路径），是内核的
# 功劳不是 dct 的。
#
# 差别是实打实的：现在没有那 200ms 宽限期（`PtySession::kill` 里那一档），
# agent 没有机会自己收尾落盘。要补的话是在 `daemon.rs` 里装一个 SIGTERM
# 处理器走正常停机路径——那是 dct 的改动，不是这个文件的，也不在第一期。
#
# **tini 仍然在前面**（见 Dockerfile 的 ENTRYPOINT），因为 `exec` 只解决信号，
# 不解决收尸：父进程先死时孤儿会被 reparent 到 PID 1，而一个不 reap 的 PID 1
# 会让僵尸堆到进程表满。守护进程自己 `child.wait()` 收得干净（`pty.rs` 那条
# `no_zombie` 测试盯着），它管不到的是别人留下的孤儿。
# **这一条还没在容器里实测**，任务 2 的验收条件就是验它。
set -eu

# ttyd 在这里加进来（任务 4）。那时候这个文件会变成「起两个东西」，信号转发
# 也要跟着改——所以上面那段 `exec` 的理由到时候要重读一遍，不是照搬。

exec dct daemon

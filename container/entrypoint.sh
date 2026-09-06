#!/bin/sh
# dc-workspace 的 PID 1 下面那一层。
#
# **为什么是 `exec` 而不是「起在后台再 wait」**：`exec` 之后守护进程直接顶替
# 这个 shell，`docker stop` 发的 SIGTERM 就落在它自己身上。而 Unix 上的
# SIGTERM 正是 `sys::proc` 开头抱怨 Windows 没有的那一句——「请你自己收拾干净
# 再走」。守护进程收到它会把 pty 关掉、让每个 agent 有机会收尾。中间垫一个
# shell 的话，信号停在 shell 上，守护进程要等到容器被强杀才知道，那正好丢掉
# 搬到 Linux 白捡的这个好处。
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

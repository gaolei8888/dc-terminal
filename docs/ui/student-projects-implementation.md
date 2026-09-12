# 学生项目库首期 · 实现与验收

2026-09-10 实现；dataclue.cn 于 2026-09-11 UTC 上线。

## 已实现

- 真实项目库、新建项目、原有工作目录无移动接入。
- 递归文件列表、上传进度、同名上传拒绝覆盖、单文件快照下载。
- 完整 ZIP 下载、原子归档发布、服务端确认的最近归档时间。
- 继续所选 Claude/Codex/Shell 项目，复用已有匹配会话。
- 保存后结束当前项目会话，再次归档；失败保留作品和旧归档。
- 可折叠桌面侧栏、手机抽屉、主题同步、收起恢复终端输入焦点。

项目代码目录保留，不删除依赖、缓存或项目文件。归档排除敏感配置、
依赖、Git 内部文件和链接；不是完整机器备份。自行脱离 dct 的后台进程
不在结束会话承诺内。同一链接共享工作区，独立学生仍需独立工作区和卷。
命名版本、ZIP 导入、删除和回收站不属于本期。

## 存储

原有项目在 `/home/dc/work`。新项目与元数据在
`/home/dc/.dct/student-projects/`，使用已有持久化 `.dct` 卷。
归档使用独立 ZIP 文件，并以原子更新项目 JSON 的方式确认新归档。
更新记录失败不会覆盖旧 ZIP；成功后清除上一份归档。
归档时间代表最近确认的快照，不代表随后修改已再次归档。

## 验证记录

- `cargo test --lib -- --test-threads=1`：1235 通过。
- 全量并行首跑有两项既有 PTY/session 时序测试失败；单独复跑和完整串行复跑均通过。
- `python3 container/tests/project-api.py`：真实 daemon/gate 验证鉴权、上传、下载、
  会话复用、超限保存失败不停止会话、结束只停止所选项目、重启持久化和恢复。
- `PLAYWRIGHT_MODULE=/tmp/dct-theme-browser/node_modules/playwright node container/tests/project-panel.cjs`：
  前端交互、错误、文件名转义、主题、390px 布局与焦点通过。
- 独立 Linux 容器 + Chrome：创建、上传、保存、ZIP 下载、Shell 启动与结束、
  刷新持久化和移动端通过。
- 线上只读检查：新 UI、真实项目/文件列表、认证与未认证请求隔离通过；
  原终端 WebSocket 与 3 GiB 限制检查通过。

真实浏览器测试发现普通 HTTP 连接复用会把项目请求送进 ttyd。已在 gate
对普通转发请求设置 `Connection: close`，保留 WebSocket 握手；新增回归测试。
Caddy 仍保持 `keepalive off`。

## 线上部署与回滚

构建目录：`/opt/dc-terminal/student-projects`。
镜像：`dc-workspace:0.2.14-student-projects`。
配置：`/opt/dc-terminal/deployment/compose.yaml`、`/etc/caddy/Caddyfile`。

为保留正在运行的学生会话，未重建 `dcw-workspace-1`。新增
`dcw-project-api-1` 服务共享 `.dct` 与 `work` 卷，通过同一 daemon socket
管理会话；Caddy 把 `/_dct/projects*` 转至宿主环回 17681。
终端仍使用环回 7681，页面文件已原子替换。API 服务限 256 MiB；学生
运行工作区仍限 3 GiB。Compose 已记录新镜像，后续正常重建工作区也有新 UI。

上线前工作目录、状态、页面、Compose 和 Caddy 配置备份位于：
`/opt/dc-terminal/deployment/before-student-projects/`（仅管理员可读）。
备份不包括运行程序的内存状态。

回滚时恢复该目录中的旧页面与 Caddy/Compose 配置，验证配置后重载 Caddy，
再停止项目 API 服务。不要删除持久化卷，不要用旧备份覆盖上线后的学生作品。
项目数据保留在新目录中，可在修复后重新接入。

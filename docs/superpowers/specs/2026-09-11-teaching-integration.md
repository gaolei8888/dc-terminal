# 教学平台与 DCT 集成

用户已确认：dc_classroom 为学生入口，dc_classeditor 为教师入口，DCT 提供独立工作区，ai-mania 提供审核后的作品展示。保持现有独立卷、会话和管理员恢复入口。

## 第一期：身份和权限

账号主键是持久化 User.schoolName + User.id，不使用姓名、客户端传入角色或请求里的学校作为登录身份。保留 classroom canUseWorkspace 权限。教师按照 TeacherCourseContext.teacher -> taughtCourse.schoolClass -> User.schoolClass 查询本班名单；学校管理员仅本校，只有 DEV/SUPERUSER 为平台管理员。

登录协议（HTTPS）：
1. 应用已登录用户点击 DctBridgeController.launch，无 state 时跳到 DCT `/sso/start?issuer=classroom|classeditor`。
2. DCT 创建 32 字节随机 state，写入 HttpOnly Secure SameSite=Lax、Path=/sso/、120 秒 cookie；跳转到配置的 issuer launchUrl，附带 state。
3. 业务应用从当前认证用户生成 `{tenant,subject,name,state}`，后端 POST DCT `/integration/tickets`。Headers: `Authorization: Bearer <该应用独立的服务密钥>`、`X-DCT-Issuer: classroom|classeditor`、JSON。不得由浏览器传服务密钥。
4. DCT 校验发行方和当前权限，返回 `{url:"https://dataclue.cn/sso/#ticket=<64hex>"}`。应用验证返回 origin、pathname 与配置一致后跳转。
5. DCT landing 脚本移除 fragment，并 POST `/sso/consume` `{ticket}`；验证 cookie state、一次使用、120 秒到期，建立作用域 session，返回 `{url:"/w/<id>/"|"/admin/"}`。旧页面不携带长期 workspace token。

服务配置：DCT `CLASSROOM_SSO_ISSUERS_FILE` 指向仅 root 可读 JSON：`{"classroom":{"key":"...","launchUrl":"https://stu.tzspace.cn/dctBridge/launch","permissionsUrl":"https://stu.tzspace.cn/dctBridge/permissions"},"classeditor":{...}}`。
业务应用 `dc.dct.enabled`, `dc.dct.baseUrl`, `dc.dct.serviceKey`，环境变量分别 `DCT_ENABLED`, `DCT_BASE_URL`, `DCT_SERVICE_KEY`。classroom/classeditor issuer 固定在各自服务中。

DCT 后端权限复核：POST issuer permissionsUrl，Bearer 为对应服务密钥，JSON `{tenant,subject}`。
返回 `{active:boolean,role:"student"|"teacher"|"platform_admin",students:[{tenant,subject,name,classId,className}]}`。
学生无 students 数组，active 同时检查账户启用、锁定、到期以及 workspace entitlement。
教师名单由后端查库；active=false 或接口失败即拒绝，不用历史名单兜底。
DCT 权限结果最多缓存 15 秒；长连接每 5 秒触发检查，到期/撤权关闭连接和协助租约。
教师看不到添加账号、长期链接、重置链接、停用账号等本地管理员控制；可启动、保存结束、查看和协助允许的学生。

首次学生登录或者教师名单同步仅创建绑定记录，不启动容器；打开工作区才按 3 GiB 限额启动。
现有未绑定工作区不会按姓名自动合并，原有共享工作区保留。

## 后续阶段

私有预览：独立预览 origin、固定开发端口、服务端鉴权、HTTP/WS 转发、容器网络阻止学校内网/宿主与其他学生访问；DNS 未准备时不暴露未隔离的路径代理。
发布：复用 ai-mania 审核、版本和固定静态产物；agent 共用发布规则，不持有站点管理凭据。
备份：独立备份任务，将工作区持久卷及管理状态备份到另一台已授权服务器，明确恢复路径和失败告警。

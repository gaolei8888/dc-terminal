# SDD ledger — plan: docs/superpowers/plans/2026-08-04-dct-adaptive-color.md

Branch: feat/adaptive-color (off main @ 7402767)
Tasks: 5

Task 1: complete (commits 7402767..db63d76, review clean)
FOLLOW-UP (out of plan scope, raised by user 2026-08-04): 退出 agent 后会话视图停在一片空白，没有意义。见 attached-session 空屏。待本计划完成后单独处理。
Task 2: complete (commits db63d76..3fb468c, review clean)
Task 2: minor (deferred): parse_colorfgbg 先 rsplit 再查 contains(';')，顺序颠倒但功能正确（theme.rs:203-207）
Task 2: minor (deferred): parse_osc11 只找 "rgb:" 子串，不校验 \x1b]11; 头部；噪声缓冲里巧合含 "rgb:" 会被解析（theme.rs:164）。Task 3 控制传入的缓冲，已在其 dispatch 里点出。
Task 3: review 1 — spec OK; 1 Important (buffered std::io::stdin paired with libc::poll on fd 0 → stranded bytes invisible to poll and crossterm). Fix round 1 dispatched (resumed implementer adaddd068362c1ed4).
Task 3: minor (deferred): BEL 终止符全缓冲扫描，用户误敲 Ctrl-G 会提前结束读取（theme.rs:178）
Task 3: minor (deferred): from_utf8 严格，缓冲里一个非 UTF-8 字节会废掉整个回复（theme.rs:60）
Task 3: minor (deferred): 注释说「上取整」但代码是下取整+零特例（theme.rs:209）
Task 3: minor (deferred): poll 返回 >0 时未检查 revents（POLLERR/HUP/NVAL 也算）（theme.rs:211-213）
Task 3: minor (deferred): 缺一条「非 override 路径下 reader 只被调一次」的断言
Task 3: NOTE for Task 4 review — reviewer flagged as unverifiable here: init_theme() 必须在 enable_raw_mode() 之后、EnterAlternateScreen 之前调用。Task 4 审查必须确认这一点。
Task 3: fix round 1/5 (1 addressed, 0 open — libc::read on raw fd replaces buffered stdin; commits ec05c35..b2212ac)
Task 3: complete (commits 3fb468c..b2212ac, review clean)
Task 3: CAVEAT — 全部 22 个测试都走 CannedReader，从不执行 StdinReader::read_reply（libc::poll / libc::read 一行没跑过）。绿色测试不能证明真实读取路径可用；Task 5 的手工终端验收是这段代码唯一的实证。
Task 4: complete (commits b2212ac..553f65c, review clean)
Task 5: 自动化部分完成（controller 直接跑，无提交）：
  - cargo build 零 warning；cargo test 全绿，215 个测试，0 失败
  - 四级探测链在无 tty 环境下逐级验证通过（临时集成测试，跑完已删除，未进仓库）：
      DCT_THEME=light -> Light；DCT_THEME=DARK -> Dark（大小写不敏感）
      DCT_THEME=lite（非法）+ COLORFGBG=15;0 -> Dark（正确忽略非法值并降级）
      COLORFGBG=0;15 -> Light
      两个变量都不设 -> Unknown
  - 无挂起：三次 detect() 走到 OSC 11 读取，stdin 立即 EOF，总耗时 0.61s（含编译），远未触及 150ms 上限
Task 5: BLOCKED on human — 步骤 2/3/4 的肉眼验收需要真实 tty + 真实配色方案。controller 的 shell 没有 tty（OSC 11 探测无回复），无法代替用户确认「深色/浅色背景下九行菜单是否都看得见」。这正是当初测试全绿也没拦住的那类 bug，不能声称已验证。
FINAL REVIEW (opus, 7402767..553f65c): merge-ready-with-caveats. 1 Important + 3 Minor.
  Important: OSC 11 迟到的回复没人排空 -> crossterm 当按键读走。十六进制含 c/d，Board 视图里 c 开密钥页、d 武装删除；背景如 #cddddd 会产出 c 加五个 d。从「终端慢了 200ms」到「密钥被删」，用户没按任何键。分支引入的新失败模式。
  Fix wave dispatched (FIX_BASE=553f65c): DA1 哨兵 + isatty 门 + ALT/META 按键防护 + reader-called-once 断言 + 设计文档措辞更正。
FINAL FIX re-review (opus, 553f65c..6ea8c01): 5/5 findings ADDRESSED, no functional regression. 但引入 1 个 Important：
  src/ui.rs:1390-1393 的注释声称漏出的转义字节会被报成「带 Alt 的 Char」，因此 ALT 门能让泄漏无害。这是错的。
  crossterm 0.28 只给紧跟 ESC 的那一个字节加 ALT（parse.rs:78-86），发出事件后立刻清空解析缓冲（tty.rs:244-265），
  所以 `11;rgb:cdcd/dddd/dddd` 之后每个字节都是**不带修饰键**的 Char。演示过的 c -> d -> d 删除链条照样触发，
  is_plain_key 一次都不会挡住。=> 1c 不是 1a 的后备层，1a 是单点。
  两条残余泄漏路径（必须记录为已知风险）：(1) 256 字节上限在 DA1 之前返回；(2) 终端/多路复用器本地应答 DA1
  但把 OSC 11 代理到上游，破坏「按顺序应答」的前提。
  裁定：注释是在断言一个不存在的安全属性，未来读者会依赖它去削弱 1a。派一次纯文字修正（不改行为，无需再评审）。
Prose fix: 5e83ecb（is_plain_key 的安全理由改写为真实机制；DA1 哨兵记为唯一防线；两条残余泄漏路径 + 无测试覆盖写进设计文档「已知风险」）
  验证：git diff 6ea8c01..HEAD -- src/ 只有 /// 注释行变动，零非注释改动；cargo build 0 warning；222 测试全绿。
BRANCH STATE: feat/adaptive-color，7 个提交（7402767..5e83ecb），零 warning，222 测试全绿。未合并。
  未完成：Task 5 步骤 2/3 的肉眼验收（深色/浅色终端各跑一次）。需要真实 tty，controller 做不了。
  这一项不是可选的收尾——StdinReader::read_reply 至今没有任何测试执行过，poll/read/DA1 哨兵全靠读代码验证。
  当初那个 bug 也正是测试全绿没拦住的。合并前值得花一分钟在真终端上跑一次。

# 直播观众链接：中转怎么部署

日期：2026-09-12
配套设计文档：`docs/superpowers/specs/2026-09-12-live-viewer-link-design.md`

这份文档是那篇设计里这一段的落地：

> 公网上只放行 `/live/*`（它自带鉴权），`/link/*` 继续只听本机。反向代理上
> 一条路径白名单的事，但它必须写进部署文档，不能靠谁记得。
>
> **这是本功能唯一的外部依赖，也是唯一能把它做砸的地方**：哪天有人图省事
> 把整个中转开到公网上，配对那条链路就裸奔了。

所以下面三件事一件都不能省。照着做，不要改顺序。

## 为什么非要一条路径白名单

`dct-srv` 这一个进程上挂着两组路由：

| 路由 | 谁在用 | 有没有鉴权 |
| --- | --- | --- |
| `/live/*` | 学生浏览器、老师的推帧线程 | **有**：两把钥匙，学生 `viewer_token` 只读，老师 `push_secret` 只写 |
| `/link/*`（`/link/poll`、`/link/send`、`/link/ask`） | 手机端与守护进程的配对信封 | **没有**。`token` 眼下没人验（要等接 dc_classroom），内容也还没有端到端加密 |

`/live/*` 自带鉴权，可以对公网开。`/link/*` 一旦漏到公网上，任何人都能冒充
任何一台设备收发信封——那是老师终端的读写通道。

进程本身有一道硬闸：`dct-srv` 的 `main.rs` 拒绝绑非环回地址
（`must_be_loopback`）。所以中转只听 `127.0.0.1`，对外必须经过反向代理，
而白名单就写在反代上。**不要为了"方便"去掉那道硬闸**：去掉它，这份文档
里的一切都不再成立。

## 一、反向代理：只放行 `/live/*`

线上用的是 Caddy。下面这段是完整可用的最小配置：

```caddyfile
live.tzspace.cn {
	# 只有这一组路径对公网开。写成 handle 而不是 reverse_proxy 一把梭：
	# handle 之外的一切都落到最后那个兜底 respond 上。
	handle /live/* {
		reverse_proxy 127.0.0.1:8787 {
			# 建房限流按来源分桶，中转只认这个头（见 dct-srv 的
			# client_key）。Caddy 的 reverse_proxy 默认会追加
			# X-Forwarded-For，这里写出来是为了让「它必须在」这件事
			# 是显式的——换别的反代时别忘了设。
			header_up X-Forwarded-For {remote_host}

			# 学生那条 ?wait=1 的长轮询最多挂 25 秒（WAIT_TIMEOUT）。
			# 上游读超时必须明显大于它，否则每一次正常的长轮询都会被
			# 反代判成超时，整个带宽模型（画面不变时不传字节）当场
			# 失效，退化成 200 个学生不停重连。
			transport http {
				read_timeout 60s
			}
		}
	}

	# 兜底：除了 /live/* 之外，什么都不转发。
	# **这一条就是那条白名单。** 没有它，/link/* 会跟着一起上公网。
	handle {
		respond "Not Found" 404
	}
}
```

要点，逐条核对：

1. `handle /live/*` 是**唯一**一条 `reverse_proxy`。多写一条就是多开一个口。
2. 兜底的 `handle { respond 404 }` 必须在。只写 `handle /live/*` 而不写兜底，
   Caddy 对别的路径的行为取决于站点块里还有什么——别赌，明写。
3. 上游读超时要大于 25 秒。这是最容易踩的一脚：配置本身"能用"，只是每个
   学生每 25 秒掉一次线，而且现象像是网不好。
4. TLS 由 Caddy 自动办。学生那把 token 在 URL 的 fragment 里（`#t=`），
   浏览器不会把它发给服务器，所以它不进 Caddy 的访问日志——**但前提是
   没人把它挪到查询串上**。

改完 `caddy validate --config /etc/caddy/Caddyfile`，再 `caddy reload`。

### 验收（改完必须跑一遍）

```bash
# 学生页那条路：通（401 或 200 都算通——它到了中转，是中转在答话）
curl -s -o /dev/null -w '%{http_code}\n' https://live.tzspace.cn/live/deadbeef

# 配对那三条：必须是 404，而且是 Caddy 答的，不是中转答的
curl -s -o /dev/null -w '%{http_code}\n' https://live.tzspace.cn/link/poll
curl -s -o /dev/null -w '%{http_code}\n' https://live.tzspace.cn/link/send
curl -s -o /dev/null -w '%{http_code}\n' https://live.tzspace.cn/link/ask

# 根路径（手机端那页）：也必须是 404
curl -s -o /dev/null -w '%{http_code}\n' https://live.tzspace.cn/
```

`/link/*` 任意一条不是 404，就是白名单没生效，**立刻把站点停掉再排查**。

## 二、老师那一侧：`DCT_RELAY` 指到这个域名

守护进程的推帧线程从环境变量 `DCT_RELAY` 里读中转地址，没设就用内置默认值
（`src/live.rs` 的 `DEFAULT_RELAY_BASE`）。它同时也是拼给学生的那条链接的
origin——**设错了，老师拿到的链接指向一个打不开的地方**。

```bash
# 老师的机器上，写进 shell 配置里（尾部斜杠有没有都行）
export DCT_RELAY=https://live.tzspace.cn
```

设完重开 dct 的守护进程（推帧线程在守护进程启动时才起）。验收：在 dct 里
按 `L` 开一场直播，看顶栏那行常驻提示——它说「正在直播」而**不带**「正在
连接中转」或「开播失败」，就说明中转已经收下这场直播了（那一行读的是
`LiveInfo::readiness`，它只在 `POST /live/start` 真的成功之后才翻成
就绪）。

## 三、中转进程：`LimitNOFILE`

**一个学生一个 socket。** 系统默认的 `1024` 个文件描述符，200 人够用，
1000 人会 `EMFILE`——症状是新学生连不上，而已经连上的那些看起来一切正常，
最难排查的那一种。

systemd unit 里加一行：

```ini
[Unit]
Description=dct-srv（直播中转 + 配对信封）
After=network.target

[Service]
# 只听本机：对外一律经 Caddy，见本文第一节。
ExecStart=/usr/local/bin/dct-srv 127.0.0.1:8787
Restart=always
RestartSec=2

# 一个学生一个 socket。默认 1024 撑不到 1000 人；65536 是个够宽的余量，
# 内存代价可以忽略（描述符本身不占什么）。
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

`systemctl daemon-reload && systemctl restart dct-srv`，然后核一遍真的
生效了（**要看进程的，不是看 unit 文件的**）：

```bash
cat /proc/$(pidof dct-srv)/limits | grep 'open files'
```

Caddy 自己也要够：它同样是一个学生一个连接。Caddy 官方的 systemd unit
默认已经带 `LimitNOFILE=1048576`，如果是自己写的 unit，照样加上这一行。

## 边界值一览（都在 `crates/dct-link/src/live.rs` 里）

调这些值只改那一个文件，两侧共用；不要在部署脚本里复制一份。

| 常量 | 值 | 部署上要注意什么 |
| --- | --- | --- |
| `WAIT_TIMEOUT` | 25 秒 | 反代的上游读超时必须大于它 |
| `LIVE_TTL` | 60 秒 | 老师断线 60 秒后直播自己蒸发 |
| `MAX_FRAME_BYTES` | 256 KB | 反代的请求体上限不能小于它 |
| `MAX_LANES` | 4 | 一场直播最多几路 |
| `MAX_ROOMS` | 64 | 中转上最多同时几场；最坏内存 64 × 4 × 256 KB = 64 MB |
| `MAX_STARTS_PER_WINDOW` / `START_RATE_WINDOW` | 10 / 60 秒 | 按来源建房节流，靠 `X-Forwarded-For` 分桶 |

## 出了问题先看哪儿

| 现象 | 多半是 |
| --- | --- |
| 学生打开是白页/打不开 | `DCT_RELAY` 指错了，或者 Caddy 的 `/live/*` 那条没生效 |
| 学生每 25 秒闪一下、流量居高不下 | 反代上游读超时小于 `WAIT_TIMEOUT` |
| 人多了之后新学生连不上，老学生正常 | `LimitNOFILE` 没提，`EMFILE` |
| 老师屏幕上一直「正在连接中转」 | `POST /live/start` 没成功：`DCT_RELAY` 错了，或者白名单把它也挡了 |
| 老师屏幕上「开播失败（状态码 429）」 | 建房节流或者中转房间满了，见上表两条 |
| `/link/poll` 在公网上答得出话 | **白名单没生效，立刻停掉站点** |

# ESP32 摄像头：dct 的一只眼睛

板子只做一件事：**拍一张、交出去。** 不分析、不判断、不存储——判断全部在大模型那边做
（见 `docs/superpowers/specs/2026-09-26-dct-control-layer-design.md`「ESP32 摄像头」）。

## 这块板（2026-09-26 实测）

| | |
|---|---|
| 芯片 | ESP32-S3（QFN56，v0.2），内置 8MB PSRAM，16MB flash（N16R8） |
| 摄像头 | **OV3660** |
| 引脚 | `data=[11,9,8,10,12,18,17,16] pclk=13 vsync=6 href=7 sda=4 scl=5 xclk=15`，pwdn/reset 不接 |
| 时钟 | `xclk_freq=10MHz` |
| 画面格式 | **RGB565 能用；JPEG 不能用**（构造时报「拍不到第一帧」，运行中切到 JPEG 拍出 0 字节，原因未查） |

两个 USB-C 口：
- **UART 口（WCH 串口，macOS 上是 `/dev/cu.usbserial-*`）——刷机和传文件用这个。** esptool 能自动让板子进下载模式。
  **只能用默认的 115200**：921600 和 460800 切换波特率之后串口就断。
- 芯片自带的 USB 口：出厂演示程序占着它，esptool 进不了下载模式，按 BOOT/RST 也没成功。别用它刷机。

## 固件

cnadler86/micropython-camera-API v0.6.2 的 `mpy_cam-v1.27.0-ESP32_GENERIC_S3-SPIRAM_OCT.zip`（MicroPython 1.27）：

    https://github.com/cnadler86/micropython-camera-API/releases/download/v0.6.2/mpy_cam-v1.27.0-ESP32_GENERIC_S3-SPIRAM_OCT.zip
    firmware.bin sha256 = 5431ca9809e3f9812851d0ffdcee090ffb25f5ccbfb00768e96b3d7f981e7be4

不进仓库：别人的项目，按上面的地址随时取回。本机一份在 `~/Downloads/esp32/fw/firmware.bin`。

⛔ 不要用 lemariva/micropython-camera-driver（`camera.init(0, d0=…)` 那套 API）：它停在 MicroPython 1.21，不支持 ESP32-S3。

```sh
PORT=/dev/cu.usbserial-XXXX
python3 -m esptool --port $PORT erase-flash
python3 -m esptool --port $PORT write-flash 0 firmware.bin
```

**刷之前先备份**：擦除会清掉整块 flash。出厂演示程序的分区只用到 0x110000，
备份在 `~/Downloads/esp32/backup-2884854-4b434-2026-09-26-0x0-0x110000.bin`
（sha256 `4dd0476f2da2284928220d214392ff174a8482a633b01f0fd4c99cc23c019481`），恢复用 `write-flash 0 <备份文件>`。不进仓库：那是卖家的程序。

## 装服务

```sh
python3 -m mpremote connect $PORT cp dct_cam.py :dct_cam.py + cp main.py :main.py + reset
```

板上还要一个 `/wifi.json`（**只存在板子上，永远不进仓库**）：

```json
{"ssid": "你的 2.4GHz Wi-Fi", "password": "...", "token": "一串随机字符"}
```

没有这个文件、或者 Wi-Fi 连不上，服务只打印一句就退回 REPL——板子插在充电头上时，开机卡死等于变砖。

## 接口

都要带请求头 `X-Token: <token>`，不对就回 401（常数时间比较）。明文 HTTP：同一个 Wi-Fi 里抓包看得见 token 和画面。

| | |
|---|---|
| `GET /status` | `{"ip", "rssi", "sensor", "uptime_ms", "free"}` |
| `GET /capture` | 一帧原始画面。响应头 `X-Width`、`X-Height`、`X-Format: rgb565be`（高字节在前）。QVGA 一帧 153600 字节 |

## 2026-09-26 实测：整条路通了

- 板子连上家里的 2.4GHz 网络，拿到局域网地址，信号 -45 dBm。
- 不带 token → 401；\`/status\` → 正常；\`/capture\` → 320×240 RGB565，153600 字节，**经 Wi-Fi 2.35 秒**。
- Wi-Fi 密码从 Mac 钥匙串读出、直接写进板子的 \`/wifi.json\`，没显示、没落盘。
  板子的 token 存在 Mac 钥匙串：服务名 \`dct-esp32-cam\`，账户 \`token\`
  （\`security find-generic-password -s dct-esp32-cam -a token -w\`）。

⚠️ **macOS 的「本地网络」权限按程序单独给。** pyenv 装的 Python 连板子报 \`No route to host\`（errno 65），
同一时刻系统自带的 \`curl\` 能连、\`ping\` 也通。dct 第一次连板子时 macOS 会弹窗问要不要允许——
dct 要在弹窗之前先用一句话说明，跟手机端「防火墙会问你」同一个做法。

⚠️ ESP32 只能连 2.4GHz。路由器把两个频段分成两个名字时（比如 `xxx-2.4g` / `xxx-5G`），要选 2.4g 那个。

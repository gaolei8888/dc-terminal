# ESP32 摄像头：dct 的一只眼睛

同一份 `dct_cam.py` 支持两块板：开机先按 KIDVIEDU 的接法试（OV2640 硬件 JPEG），不行再按 N16R8 的接法（OV3660 + 板上软件 JPEG）。

板子只做一件事：**拍一张、交出去。** 不分析、不判断、不存储——判断全部在大模型那边做
（见 `docs/superpowers/specs/2026-09-26-dct-control-layer-design.md`「ESP32 摄像头」）。

## 这块板（2026-09-26 实测）

| | |
|---|---|
| 芯片 | ESP32-S3（QFN56，v0.2），内置 8MB PSRAM，16MB flash（N16R8） |
| 摄像头 | **OV3660** |
| 引脚 | `data=[11,9,8,10,12,18,17,16] pclk=13 vsync=6 href=7 sda=4 scl=5 xclk=15`，pwdn/reset 不接 |
| 时钟 | `xclk_freq=24MHz`，`fb_count=2`，`GrabMode.LATEST` |
| 画面格式 | 传感器拍 RGB565，板上用固件自带的 `jpeg` 模块压成 JPEG（见下）。**传感器自己的 JPEG 在这版驱动里用不了** |

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
| `GET /capture` | 一张 JPEG（320×240，约 3.8KB）。加 `?raw=1` 回原始 RGB565（`X-Format: rgb565be`，153600 字节） |
| `GET /stream` | MJPEG 视频流，浏览器直接能播。浏览器的 `<img>` 带不了请求头，所以这里也认网址里的 `?t=<token>`。**一次只接一个观看者**：推流期间别的请求都在排队 |

## 2026-09-26 实测：整条路通了

- 板子连上家里的 2.4GHz 网络，拿到局域网地址，信号 -45 dBm。
- 不带 token → 401；`/status` → 正常；`/capture` → 320×240 RGB565，153600 字节，**经 Wi-Fi 2.35 秒**。
- Wi-Fi 密码从 Mac 钥匙串读出、直接写进板子的 `/wifi.json`，没显示、没落盘。
  板子的 token 存在 Mac 钥匙串：服务名 `dct-esp32-cam`，账户 `token`
  （`security find-generic-password -s dct-esp32-cam -a token -w`）。

⚠️ **macOS 的「本地网络」权限按程序单独给。** pyenv 装的 Python 连板子报 `No route to host`（errno 65），
同一时刻系统自带的 `curl` 能连、`ping` 也通。dct 第一次连板子时 macOS 会弹窗问要不要允许——
dct 要在弹窗之前先用一句话说明，跟手机端「防火墙会问你」同一个做法。

⚠️ ESP32 只能连 2.4GHz。路由器把两个频段分成两个名字时（比如 `xxx-2.4g` / `xxx-5G`），要选 2.4g 那个。

## JPEG：为什么在板上压、压得多快

**传感器自带的 JPEG 用不了。** 36 组参数（时钟 5/8/10/16/20/24MHz × 画质 20/50/85 × 缓冲 1/2 × 取帧方式）全部报
「拍不到第一帧」；先用 RGB565 启动再 `reconfigure` 到 JPEG，拍出来 0 字节。别人在同款传感器上结论一样：
[XIAO-ESP32S3-Sense-Setup](https://github.com/jouellnyc/XIAO-ESP32S3-Sense-Setup) 写着
「`reconfigure()` and constructor-time `pixel_format=PixelFormat.JPEG` cause init failures on OV3660」；
乐鑫驱动 [issue #862](https://github.com/espressif/esp32-camera/issues/862)（S3 + OV3660，初始化成功但一帧都拿不到）没有官方结论。

**所以拍 RGB565、在板上用固件自带的 `jpeg` 模块压**（cnadler86 固件带的 mp_jpeg，底下是乐鑫的 esp_new_jpeg，S3 向量指令加速）。
压缩必须在板上做：瓶颈是 Wi-Fi 那一段，到了电脑上再压就晚了。压缩只是打包，不是判断。实测：

| 分辨率 / 画质 | 压一帧 | 大小 |
|---|---|---|
| 320×240 / 60 | 33 ms | 150KB → 3.4KB（45 倍） |
| 640×480 / 60 | 188 ms | 600KB → 7.3KB（84 倍） |

**拍才是瓶颈**（10MHz、一个缓冲时拍一帧 270–360 ms）。320×240、画质 60 时「拍 + 压」的帧率：

| 时钟 | 1 个缓冲，等下一帧 | 2 个缓冲，取最新一帧 |
|---|---|---|
| 10MHz | 2.7 帧/秒 | 5.5 |
| 16MHz | 4.4 | 8.8 |
| 20MHz | 5.5 | 11.0 |
| **24MHz** | 7.3 | **14.3** |

更狠的压缩（H.264，乐鑫有给 S3 的软件编码器，静止画面比 JPEG 再小 5–10 倍）不在这版 MicroPython 固件里，
要自己用 C 编固件。JPEG 够用之前不碰。

## 第二块板：KIDVIEDU（OV2640，2026-09-26 实测）

| | |
|---|---|
| 芯片 | ESP32-S3（QFN56，v0.2），内置 8MB PSRAM，16MB flash |
| 串口 | 板载 WCH CH343（macOS 上是 `/dev/cu.usbmodem5ABA…`），esptool 自动复位能用 |
| 摄像头 | **OV2640**，插在单独的模组座上 |
| 引脚 | 数据线同上，但 **`xclk_pin=-1`**：模组自带时钟（板上 `/pin_definition.py` 写的就是 `XCLK=None`） |
| 画面格式 | **传感器硬件 JPEG 完好**：320×240 **27 帧/秒**、每帧 4.4KB；640×480 6.8 帧/秒、11KB；全部合格、零警告。RGB565 反而会丢数据（`cam_hal: FB-SIZE: N != 153600`），不用它 |

⚠️ **模组要插到位。** 前两次测都报 errno 95（扫不到任何传感器），重新插了第三次才通。判断法：
`Pin(4, Pin.IN, Pin.PULL_DOWN).value()` 和 5 号脚读 1 = 模组接上了（外部上拉压过内部下拉）；读 0 = 没插到位。
（这块板的 GPIO0 读 0，不能拿它当对照。）

板上出厂就是 camera 版 MicroPython 1.27，不用刷机，直接装服务。实测单张 `/capture` 5.7KB、0.72 秒。

## KIDVIEDU 的屏幕和喇叭（2026-09-26 实测）

**屏幕** ST7789 240×320，用 russhughes 的 `st7789py.py`（[上游](https://github.com/russhughes/st7789py_mpy)，放在板上 `/lib/`）：

```python
spi = SPI(1, baudrate=40_000_000, polarity=1, sck=Pin(46), mosi=Pin(3))   # polarity=1 不能省
tft = st7789py.ST7789(spi, 240, 320, dc=Pin(1, Pin.OUT), cs=Pin(14, Pin.OUT))
```

- **`polarity=1` 不能省**：不写就不亮，而且 `init` 照样不报错。这组参数和 dc_desktop
  `packages/device-peripherals/src/display/driver.py` 里已验证过的一致；SPI 用 1 号——2 号在 SPIRAM_OCT 固件上被八线 PSRAM 占着，会崩。
- **模组要插实**：几次「完全黑 / 微微有一点光」都是没插到位，跟摄像头同一个毛病。程序跑完不报错，不代表屏幕亮了——要问看的人。

**喇叭** I2S，在主板上（不用插模组）：

```python
spk = I2S(0, sck=Pin(42), ws=Pin(39), sd=Pin(41), mode=I2S.TX, bits=16, format=I2S.MONO, rate=16000, ibuf=16384)
```

- **振幅 2000（满格的约 6%）是默认音量**：8000（24%）和 4000（12%）都破音，1000 偏小。
- **整块送数据**（比如一次 0.1 秒），别一个波形周期一个周期地 `write`：MicroPython 跟不上会断音。

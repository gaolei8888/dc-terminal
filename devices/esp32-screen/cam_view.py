# 屏幕板：从摄像头板拉 MJPEG 视频流，逐帧解码铺满屏幕。只显示，不判断。
# 摄像头地址和 token：token 与摄像头共用 /wifi.json 里的那把；地址默认 192.168.1.38，可在 /wifi.json 里写 "camera"。
# 控制：本板 80 端口，每显示完一帧看一眼有没有指令（都要 X-Token）：
#   /status            在干什么、每秒几帧
#   /mode?m=camera|off 看摄像头 / 关屏
#   /beep?f=880&ms=300 喇叭响一声（喇叭平时关着）
#   /listen?ms=800     用板上麦克风听一会儿，回响度（喇叭关着时就是现场噪音）
#   /beep?check=1      响的同时用麦克风录，回「这个频率比安静时强多少倍」——喇叭自检
#   /reset             重启
import json, network, socket, time, gc, math, struct, machine
from machine import Pin, SPI
import jpeg
import st7789py as st

def _wifi(c):
    w = network.WLAN(network.STA_IF); w.active(True)
    try: w.config(txpower=8.5)
    except Exception: pass
    try: w.config(pm=w.PM_NONE)          # 省电模式会让收数据慢十几倍（DJ 实测 30–56 ms 一帧 → 关掉后约 1 ms）
    except Exception: pass
    if not w.isconnected():
        w.connect(c["ssid"], c.get("password", "")); t = time.ticks_ms()
        while not w.isconnected():
            if time.ticks_diff(time.ticks_ms(), t) > 20000:
                return None
            time.sleep_ms(200)
    return w

def _readline(s):
    line = b""
    while not line.endswith(b"\r\n"):
        ch = s.recv(1)
        if not ch: raise OSError("closed")
        line += ch
    return line

def _readexact(s, buf, n):
    mv = memoryview(buf); got = 0
    while got < n:
        k = s.readinto(mv[got:n])
        if not k: raise OSError("closed")
        got += k

_idle = None

def speaker_off():
    # 让喇叭安静：线悬空会拾杂音（用户听到「全是噪音」）；把线拉低也不行——功放丢了时钟会自己发嘶嘶声，
    # 麦克风量到 4559–9190。最安静的是让 I2S 开着、一直送静音（2286）：DMA 在后台自动补零，不占 CPU。
    global _idle
    from machine import I2S
    if _idle is None:
        _idle = I2S(0, sck=Pin(42), ws=Pin(39), sd=Pin(41), mode=I2S.TX, bits=16, format=I2S.MONO, rate=16000, ibuf=4096)
    _idle.write(bytearray(4096))

def _speaker_release():
    global _idle
    if _idle is not None:
        _idle.deinit(); _idle = None

def beep(freq=880, ms=300, amp=3000):
    from machine import I2S
    RATE = 16000; n = RATE // 50; buf = bytearray(n * 2)
    for i in range(n):
        struct.pack_into("<h", buf, i * 2, int(amp * math.sin(2 * math.pi * freq * i / RATE)))
    _speaker_release()
    spk = I2S(0, sck=Pin(42), ws=Pin(39), sd=Pin(41), mode=I2S.TX, bits=16, format=I2S.MONO, rate=RATE, ibuf=8192)
    for _ in range(max(1, ms // 20)): spk.write(buf)
    spk.write(bytearray(RATE // 5)); spk.deinit(); speaker_off()

def _mic():
    from machine import I2S
    return I2S(1, sck=Pin(21), ws=Pin(40), sd=Pin(2), mode=I2S.RX, bits=32, format=I2S.STEREO, rate=16000, ibuf=60000)

def _left(buf):
    return [struct.unpack_from("<i", buf, i)[0] >> 8 for i in range(0, len(buf), 8)]

def _power(x, f):
    k = 2 * math.cos(2 * math.pi * f / 16000); s1 = s2 = 0.0
    for v in x: s0 = v + k * s1 - s2; s2 = s1; s1 = s0
    return (s1 * s1 + s2 * s2 - k * s1 * s2) ** 0.5

def listen(ms=800):
    mic = _mic(); chunk = bytearray(320 * 8)
    for _ in range(15): mic.readinto(chunk)                 # 丢掉上电瞬态约 0.3 秒
    xs = []
    for _ in range(max(1, ms // 20)): mic.readinto(chunk); xs += _left(chunk)
    mic.deinit(); m = sum(xs) / len(xs)
    return {"rms": int((sum((v - m) ** 2 for v in xs) / len(xs)) ** 0.5), "ms": ms}

def beep_check(freq=880, ms=400, amp=3000):
    from machine import I2S
    RATE = 16000; n = 320; tone = bytearray(n * 2)
    for i in range(n): struct.pack_into("<h", tone, i * 2, int(amp * math.sin(2 * math.pi * freq * i / RATE)))
    silent = bytearray(n * 2); chunk = bytearray(n * 8)
    _speaker_release()
    spk = I2S(0, sck=Pin(42), ws=Pin(39), sd=Pin(41), mode=I2S.TX, bits=16, format=I2S.MONO, rate=RATE, ibuf=4096); mic = _mic()
    for _ in range(15): spk.write(silent); mic.readinto(chunk)
    quiet = []
    for _ in range(10): spk.write(silent); mic.readinto(chunk); quiet += _left(chunk)
    loud = []
    for j in range(max(10, ms // 20)):
        spk.write(tone); mic.readinto(chunk)
        if j >= 5: loud += _left(chunk)
    spk.write(bytearray(RATE // 5)); spk.deinit(); mic.deinit(); speaker_off()
    a = _power(loud[:len(quiet)], freq); b = _power(quiet, freq)
    return {"beep": True, "freq": freq, "ratio": round(a / max(b, 1), 1), "heard": a / max(b, 1) > 20}

state = {"mode": "camera", "fps": 0.0, "camera": "", "ip": ""}

def _same(a, b):
    if len(a) != len(b): return False
    r = 0
    for x, y in zip(a, b): r |= ord(x) ^ ord(y)
    return r == 0

def poll(srv, token, tft):
    # 非阻塞地看一眼有没有指令；有就处理完再回去显示
    try:
        cl, _ = srv.accept()
    except OSError:
        return
    try:
        cl.settimeout(2)
        req = cl.recv(1024).decode()
        path = req.split(" ")[1] if " " in req else "/"
        got = ""
        for h in req.split("\r\n")[1:]:
            if h.lower().startswith("x-token:"): got = h.split(":", 1)[1].strip()
        q = {}
        if "?" in path:
            for kv in path.split("?", 1)[1].split("&"):
                k, _, v = kv.partition("="); q[k] = v
        if not _same(got, token):
            body, code = {"error": "token"}, "401 Unauthorized"
        elif path.startswith("/status"):
            gc.collect(); body, code = dict(state, free=gc.mem_free()), "200 OK"
        elif path.startswith("/mode"):
            m = q.get("m", "")
            if m in ("camera", "off"):
                state["mode"] = m
                if m == "off": tft.fill(st.BLACK)
                body, code = {"mode": m}, "200 OK"
            else:
                body, code = {"error": "m 只能是 camera 或 off"}, "400 Bad Request"
        elif path.startswith("/beep"):
            f = int(q.get("f", 880)); ms = int(q.get("ms", 300))
            if q.get("check") == "1": body = beep_check(f, ms)
            else: beep(f, ms); body = {"beep": True}
            code = "200 OK"
        elif path.startswith("/listen"):
            body, code = listen(int(q.get("ms", 800))), "200 OK"
        elif path.startswith("/reset"):
            cl.send(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"reset\": true}"); cl.close()
            time.sleep_ms(200); machine.reset()
        else:
            body, code = {"error": "path"}, "404 Not Found"
        data = json.dumps(body).encode()
        cl.send(b"HTTP/1.0 %s\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n" % (code.encode(), len(data)) + data)
    except Exception as e:
        print("cam_view: 指令出错", e)
    finally:
        try: cl.close()
        except Exception: pass

def run():
    speaker_off()
    try:
        c = json.load(open("/wifi.json"))
    except Exception:
        print("cam_view: 没有 /wifi.json，不启动"); return
    tft = st.ST7789(SPI(1, baudrate=40000000, polarity=1, sck=Pin(46), mosi=Pin(3)), 240, 320,
                    dc=Pin(1, Pin.OUT), cs=Pin(14, Pin.OUT), rotation=1)      # 横屏 320x240，正好是摄像头画面大小
    WAIT = st.color565(0, 30, 80)
    tft.fill(WAIT)
    if not _wifi(c):
        print("cam_view: Wi-Fi 连不上"); return
    host = c.get("camera", "192.168.1.38"); token = c["token"]
    state["camera"] = host; state["ip"] = network.WLAN(network.STA_IF).ifconfig()[0]
    srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", 80)); srv.listen(2); srv.setblocking(False)
    print("cam_view: 控制地址 http://%s/" % state["ip"])
    dec = jpeg.Decoder(pixel_format="RGB565_BE")
    buf = bytearray(60000)
    while True:
        s = None
        if state["mode"] == "off":
            poll(srv, token, tft); time.sleep_ms(100); continue
        try:
            s = socket.socket(); s.settimeout(5)
            s.connect(socket.getaddrinfo(host, 80)[0][-1])
            s.send(b"GET /stream?t=%s HTTP/1.0\r\nHost: %s\r\n\r\n" % (token.encode(), host.encode()))
            status = _readline(s)
            if b" 200 " not in status:
                raise OSError("camera said " + status.decode().strip())
            while _readline(s) != b"\r\n": pass          # 跳过响应头
            print("cam_view: 在看 %s" % host)
            n = 0; t0 = time.ticks_ms()
            while True:
                line = _readline(s)
                if not line.startswith(b"--frame"): continue
                size = 0
                while True:
                    h = _readline(s)
                    if h == b"\r\n": break
                    if h.lower().startswith(b"content-length:"): size = int(h.split(b":")[1])
                if size <= 0 or size > len(buf): raise OSError("bad frame size %d" % size)
                _readexact(s, buf, size)
                img = dec.decode(memoryview(buf)[:size])
                tft.blit_buffer(img, 0, 0, 320, 240)
                poll(srv, token, tft)
                if state["mode"] != "camera": break
                n += 1
                if n % 100 == 0:
                    state["fps"] = round(n * 1000 / time.ticks_diff(time.ticks_ms(), t0), 1)
                    print("cam_view: %.1f 帧/秒" % state["fps"]); n = 0; t0 = time.ticks_ms()
                    gc.collect()
        except Exception as e:
            print("cam_view: 连不上摄像头（%s），3 秒后重试" % e)
            poll(srv, token, tft)
            try: tft.fill(WAIT)
            except Exception: pass
        finally:
            if s: s.close()
        time.sleep(3)

# dct 摄像头服务：Wi-Fi 上给 dct 提供画面。凭据在板上 /wifi.json：{"ssid":..., "password":..., "token":...}
# 规矩：任何一步失败都只打印一句、退回 REPL——板子挂在充电头上时，开机卡死等于变砖。
import json, network, socket, time, gc

def _load():
    try:
        with open("/wifi.json") as f:
            c = json.load(f)
        if c.get("ssid") and c.get("token"):
            return c
        print("dct_cam: /wifi.json 缺 ssid 或 token，不启动")
    except OSError:
        print("dct_cam: 没有 /wifi.json，不启动（回到 REPL）")
    except ValueError:
        print("dct_cam: /wifi.json 不是合法 JSON，不启动")
    return None

def _wifi(c, timeout_s=20):
    w = network.WLAN(network.STA_IF)
    w.active(True)
    try:
        w.config(txpower=8.5)   # 发射功率太高会让 WPA2 握手失败，报 202，长得跟密码错一样
    except Exception:
        pass
    if not w.isconnected():
        w.connect(c["ssid"], c.get("password", ""))
        t = time.ticks_ms()
        while not w.isconnected():
            if time.ticks_diff(time.ticks_ms(), t) > timeout_s * 1000:
                print("dct_cam: Wi-Fi 连不上（status %s），不启动" % w.status())
                return None
            time.sleep_ms(200)
    return w

def _camera():
    from camera import Camera, PixelFormat, FrameSize
    return Camera(data_pins=[11, 9, 8, 10, 12, 18, 17, 16], pclk_pin=13, vsync_pin=6, href_pin=7,
                  sda_pin=4, scl_pin=5, xclk_pin=15, powerdown_pin=-1, reset_pin=-1,
                  xclk_freq=10000000, pixel_format=PixelFormat.RGB565, frame_size=FrameSize.QVGA, fb_count=1)

def _same(a, b):
    # 常数时间比较，别让响应时间泄露 token 对了几位
    if len(a) != len(b):
        return False
    r = 0
    for x, y in zip(a, b):
        r |= ord(x) ^ ord(y)
    return r == 0

def _reply(cl, status, body=b"", ctype="application/json", extra=""):
    head = "HTTP/1.0 %s\r\nContent-Type: %s\r\nContent-Length: %d\r\n%sConnection: close\r\n\r\n" % (status, ctype, len(body), extra)
    cl.send(head.encode())
    if body:
        mv = memoryview(body)
        while mv:
            n = cl.send(mv)
            mv = mv[n:]

def serve(port=80):
    c = _load()
    if not c:
        return
    w = _wifi(c)
    if not w:
        return
    try:
        cam = _camera()
        sensor = cam.get_sensor_name()
    except Exception as e:
        print("dct_cam: 摄像头起不来：", e)
        cam, sensor = None, None
    ip = w.ifconfig()[0]
    print("dct_cam: http://%s:%d/  sensor=%s" % (ip, port, sensor))
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", port))
    s.listen(2)
    token = c["token"]
    while True:
        cl, _ = s.accept()
        try:
            cl.settimeout(5)
            req = cl.recv(1024).decode()
            line = req.split("\r\n", 1)[0]
            parts = line.split(" ")
            path = parts[1] if len(parts) > 1 else "/"
            got = ""
            for h in req.split("\r\n")[1:]:
                if h.lower().startswith("x-token:"):
                    got = h.split(":", 1)[1].strip()
            if not _same(got, token):
                _reply(cl, "401 Unauthorized", b'{"error":"token"}')
            elif path.startswith("/status"):
                gc.collect()
                body = json.dumps({"ip": ip, "rssi": w.status("rssi"), "sensor": sensor,
                                   "uptime_ms": time.ticks_ms(), "free": gc.mem_free()}).encode()
                _reply(cl, "200 OK", body)
            elif path.startswith("/capture"):
                if cam is None:
                    _reply(cl, "503 Service Unavailable", b'{"error":"camera"}')
                else:
                    img = cam.capture()
                    img = cam.capture()   # 第一帧常是上一次留下的旧画面
                    extra = "X-Width: %d\r\nX-Height: %d\r\nX-Format: rgb565be\r\n" % (cam.get_pixel_width(), cam.get_pixel_height())
                    _reply(cl, "200 OK", bytes(img), "application/octet-stream", extra)
            else:
                _reply(cl, "404 Not Found", b'{"error":"path"}')
        except Exception as e:
            print("dct_cam: 请求出错：", e)
        finally:
            cl.close()

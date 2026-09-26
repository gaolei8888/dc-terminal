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
    # 两块板，两种接法，按顺序试，第一个成功的就用（返回 cam, enc；enc 为 None 表示传感器自己出 JPEG）：
    # 1. KIDVIEDU（OV2640）：模组自带时钟，xclk 不接（-1）；传感器硬件 JPEG 完好，QVGA 27 帧/秒、每帧 4.4KB。
    #    它的 RGB565 反而会丢数据（cam_hal: FB-SIZE 不符），所以只用 JPEG。
    # 2. N16R8（OV3660）：时钟接 15；这版驱动里 OV3660 出不了硬件 JPEG，只能拍 RGB565，
    #    再用固件自带的 jpeg 模块（esp_new_jpeg，S3 向量指令加速）压：QVGA 33 ms、150KB → 3.8KB，拍+压 14 帧/秒。
    # 压缩只是打包，不是判断——判断全在大模型那边。
    from camera import Camera, PixelFormat, FrameSize, GrabMode
    pins = dict(data_pins=[11, 9, 8, 10, 12, 18, 17, 16], pclk_pin=13, vsync_pin=6, href_pin=7,
                sda_pin=4, scl_pin=5, powerdown_pin=-1, reset_pin=-1,
                frame_size=FrameSize.QVGA, fb_count=2, grab_mode=GrabMode.LATEST)
    try:
        cam = Camera(xclk_pin=-1, xclk_freq=24000000, pixel_format=PixelFormat.JPEG, jpeg_quality=85, **pins)
        cam.capture()
        return cam, None
    except Exception as e:
        print("dct_cam: 不是 KIDVIEDU 接法（%s），换 N16R8 接法" % e)
    import jpeg
    cam = Camera(xclk_pin=15, xclk_freq=24000000, pixel_format=PixelFormat.RGB565, **pins)
    enc = jpeg.Encoder(pixel_format="RGB565_BE", quality=60,
                       width=cam.get_pixel_width(), height=cam.get_pixel_height())
    return cam, enc

def _jpeg(cam, enc):
    img = cam.capture()
    return bytes(img) if enc is None else enc.encode(img)

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
        cam, enc = _camera()
        sensor = cam.get_sensor_name()
    except Exception as e:
        print("dct_cam: 摄像头起不来：", e)
        cam, enc, sensor = None, None, None
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
            # 浏览器的 <img> 带不了请求头，所以 /stream 也认网址里的 ?t=（局域网里本来就是明文）
            if not got and "?" in path:
                for kv in path.split("?", 1)[1].split("&"):
                    if kv.startswith("t="):
                        got = kv[2:]
            if not _same(got, token):
                _reply(cl, "401 Unauthorized", b'{"error":"token"}')
            elif path.startswith("/status"):
                gc.collect()
                body = json.dumps({"ip": ip, "rssi": w.status("rssi"), "sensor": sensor,
                                   "uptime_ms": time.ticks_ms(), "free": gc.mem_free()}).encode()
                _reply(cl, "200 OK", body)
            elif cam is None and (path.startswith("/capture") or path.startswith("/stream")):
                _reply(cl, "503 Service Unavailable", b'{"error":"camera"}')
            elif path.startswith("/capture"):
                img = cam.capture()
                extra = "X-Width: %d\r\nX-Height: %d\r\n" % (cam.get_pixel_width(), cam.get_pixel_height())
                if "raw=1" in path and enc is not None:
                    _reply(cl, "200 OK", bytes(img), "application/octet-stream", extra + "X-Format: rgb565be\r\n")
                else:
                    _reply(cl, "200 OK", bytes(img) if enc is None else enc.encode(img), "image/jpeg", extra)
            elif path.startswith("/stream"):
                # MJPEG：浏览器原生就能播。一直推到对方断开；推流期间这个单线程服务不接别的请求。
                cl.settimeout(None)
                cl.send(b"HTTP/1.0 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-cache\r\n\r\n")
                while True:
                    out = _jpeg(cam, enc)
                    cl.send(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: %d\r\n\r\n" % len(out))
                    mv = memoryview(out)
                    while mv:
                        n = cl.send(mv)
                        mv = mv[n:]
                    cl.send(b"\r\n")
            else:
                _reply(cl, "404 Not Found", b'{"error":"path"}')
        except Exception as e:
            print("dct_cam: 请求出错：", e)
        finally:
            cl.close()

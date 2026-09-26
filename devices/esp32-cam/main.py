# 开机自启。出任何错都退回 REPL，不卡死。
try:
    import dct_cam
    dct_cam.serve()
except Exception as e:
    print("main: dct_cam 没起来：", e)

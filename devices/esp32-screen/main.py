# 开机自动运行：显示摄像头直播。出错只打印一句，退回 REPL，不卡死。
try:
    import cam_view
    cam_view.run()
except Exception as e:
    print("main: cam_view 没起来：", e)

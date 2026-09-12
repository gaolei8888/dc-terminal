"""Real gate + daemon lifecycle integration, using disposable local state only.

Run after cargo build: python3 container/tests/project-api.py
"""
import io
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import zipfile

binary = Path(os.environ.get("DCT_BIN", "target/debug/dct")).resolve()
with tempfile.TemporaryDirectory(prefix="dct-project-api-") as tmp:
    home = Path(tmp).resolve()
    work = home / "work"
    work.mkdir()
    env = dict(os.environ, HOME=str(home), DCT_LANG="zh", TERM="xterm-256color")
    # Establish the gate token before the daemon loads the shared secret store.
    link = subprocess.check_output([binary, "gate", "--link", "--url", "http://localhost"], cwd=work, env=env, stderr=subprocess.DEVNULL, text=True)
    token = link.strip().split("#t=")[-1]
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    origin = f"http://127.0.0.1:{port}"
    children = []

    def start(*args):
        p = subprocess.Popen([binary, *args], cwd=work, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        children.append(p)
        return p

    def api(path="", body=None, method=None, authenticated=True):
        headers = {"Cookie": "dct_gate=" + token} if authenticated else {}
        if body is not None and not isinstance(body, bytes):
            body = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(origin + "/_dct/projects" + path, data=body, headers=headers, method=method)
        with urllib.request.urlopen(req, timeout=20) as response:
            data = response.read()
            return json.loads(data) if "json" in response.headers.get("Content-Type", "") else data

    def ipc(request):
        with socket.socket(socket.AF_UNIX) as s:
            s.connect(str(home / ".dct/daemon.sock"))
            s.sendall(json.dumps(request).encode() + b"\n")
            return json.loads(s.makefile("rb").readline())

    try:
        daemon = start("daemon")
        gate = start("gate", "--port", str(port), "--upstream", "127.0.0.1:1")
        deadline = time.monotonic() + 10
        while True:
            try:
                api()
                if (home / ".dct/daemon.sock").exists():
                    break
            except (OSError, urllib.error.URLError):
                pass
            if time.monotonic() > deadline:
                raise AssertionError("gate/daemon did not start")
            time.sleep(.05)
        try:
            api(authenticated=False)
            raise AssertionError("unauthenticated request allowed")
        except urllib.error.HTTPError as e:
            assert e.code == 401
        project = api(body={"name": "真实接口练习"})
        path = "/" + project["id"]
        api(path + "/upload?name=hello.txt", b"hello student")
        assert api(path + "/files")["files"] == [{"path": "uploads/hello.txt", "size": 13}]
        saved = api(path + "/save", {})
        assert saved["saved_at"] > 0
        archive = zipfile.ZipFile(io.BytesIO(api(path + "/download")))
        assert archive.read("uploads/hello.txt") == b"hello student"
        first = api(path + "/continue", {"profile": "shell"})
        assert api(path + "/continue", {"profile": "shell"})["id"] == first["id"]
        other = api("/current/continue", {"profile": "shell"})
        oversized = Path(project["dir"]) / "too-large.bin"
        with oversized.open("wb") as f:
            f.truncate(129 * 1024 * 1024)
        try:
            api(path + "/end", {})
            raise AssertionError("oversized project ended without saving")
        except urllib.error.HTTPError as e:
            assert e.code == 409
            assert "尚未停止会话" in json.load(e)["error"]
        states = {s["id"]: s["state"] for s in ipc("List")["Sessions"]}
        assert states[first["id"]] != "Stopped"
        assert oversized.exists()
        oversized.unlink()
        ended = api(path + "/end", {})
        assert ended["stopped"] == 1
        states = {s["id"]: s["state"] for s in ipc("List")["Sessions"]}
        assert states[first["id"]] == "Stopped"
        assert states[other["id"]] != "Stopped"
        assert api(path + "/file?path=uploads%2Fhello.txt") == b"hello student"
        gate.terminate()
        gate.wait(timeout=5)
        gate = start("gate", "--port", str(port), "--upstream", "127.0.0.1:1")
        deadline = time.monotonic() + 5
        while True:
            try:
                listed = api()["projects"]
                break
            except OSError:
                if time.monotonic() > deadline:
                    raise
                time.sleep(.05)
        assert any(p["id"] == project["id"] and p["saved_at"] for p in listed)
        assert api(path + "/continue", {"profile": "shell"})["id"] != first["id"]
        print("PASS: real authentication, create/upload/list/file/ZIP, save, session reuse, end preserves files and other sessions, restart persistence, resume")
    finally:
        for p in reversed(children):
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGTERM)
                p.wait(timeout=5)

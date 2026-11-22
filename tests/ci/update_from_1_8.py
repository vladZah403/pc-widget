"""CI: 1.8's own updater installs the 2.0 zip.

python tests/ci/update_from_1_8.py dist2/PC-Monitor.zip <work folder>

Loads widget.pyw (1.8.1) with %APPDATA% in <work folder>/appdata holding a 1.8
settings file, serves PC-Monitor.zip to 1.8's Updater instead of GitHub (same
SHA-256 check, same unpacking), checks the staged 2.0 program, then calls 1.8's
swap_and_restart and exits: its PowerShell waits for this process, swaps the
folders and starts 2.0, which should take over the 1.8 settings. The next CI
step checks that (it writes WORK=... to $GITHUB_ENV for it).
"""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
zip_path, work = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
appdata = os.path.join(work, "appdata")
app = os.path.join(work, "PC-Monitor")
os.makedirs(os.path.join(appdata, "PC-Monitor"), exist_ok=True)
os.environ["APPDATA"] = appdata  # 1.8 and the 2.0 it starts use this data folder

# a 1.8 settings file, as 1.8.1 writes it
with open(os.path.join(appdata, "PC-Monitor", "widget_config.json"), "w", encoding="utf-8") as f:
    json.dump({"x": 100, "y": 120, "theme": "neon", "compact": True, "lang": "en", "alert_temp": 85,
               "hidden": ["uptime"], "tray": True, "check_updates": True, "update_seen": "1.8.1"}, f)

# a 1.8 program folder (only its shape matters: it gets swapped out)
os.makedirs(os.path.join(app, "_internal"), exist_ok=True)
for name in ("PC-Monitor.exe", os.path.join("_internal", "python313.dll"), "old-1.8.txt"):
    with open(os.path.join(app, name), "wb") as f:
        f.write(b"1.8")

sys.path.insert(0, ROOT)  # translations.py
loader = importlib.machinery.SourceFileLoader("widget", os.path.join(ROOT, "widget.pyw"))
spec = importlib.util.spec_from_loader("widget", loader)
w = importlib.util.module_from_spec(spec)
loader.exec_module(w)
print("1.8 code loaded:", w.APP_VERSION)

data = open(zip_path, "rb").read()


class Resp:
    def __init__(self, body):
        self.body, self.pos = body, 0

    def read(self, n=-1):
        n = len(self.body) - self.pos if n < 0 else n
        chunk = self.body[self.pos:self.pos + n]
        self.pos += len(chunk)
        return chunk

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


w.urllib.request.urlopen = lambda req, timeout=None, context=None: Resp(data)
asset = {"url": "https://github.com/vladZah403/pc-widget/releases/download/v2.0.0/PC-Monitor.zip",
         "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
up = w.Updater(asset, (app, app + ".new"))
up.run()
if up.state != "ready":
    sys.exit(f"1.8's updater refused the 2.0 zip: {up.state} {up.error}")
staged = sorted(os.listdir(up.staging))
print("staged:", staged)
if "PC-Monitor.exe" not in staged or "_internal" not in staged:
    sys.exit("the staged folder has no 2.0 program")

with open(os.environ.get("GITHUB_ENV", os.devnull), "a", encoding="utf-8") as f:
    f.write(f"WORK={work}\n")
w.swap_and_restart(app, up.staging)
print("swap scheduled; exiting so it can run")

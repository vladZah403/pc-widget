"""PC Monitor — desktop widget for Windows.

Shows CPU load (number + 60-second histogram), RAM, disk space and I/O,
network speed, battery, uptime and CPU/GPU temperatures. Temperatures come
from the bundled LibreHardwareMonitorLib (needs admin rights) or, as a
fallback, from LibreHardwareMonitor's Remote Web Server on this computer,
because Windows does not expose real sensor temperatures to normal programs.

Settings and the log live in %APPDATA%\\PC-Monitor.

The whole widget is drawn with Pillow (supersampled for smooth edges) and
shown in a borderless tkinter window with transparent rounded corners.
One colour scale (cool -> hot) colours every value, so colour always means
"how loaded / how hot".

Controls: drag with the left mouse button, right click for the menu.
"""

import ctypes
import json
import logging
import logging.handlers
import math
import os
import re
import socket
import struct
import sys
import threading
import time
import tkinter as tk
import urllib.request

import psutil
from PIL import Image, ImageDraw, ImageFont, ImageTk

APP_VERSION = "1.1"

# folder of the .exe when frozen, else of this script
APP_DIR = (os.path.dirname(sys.executable) if getattr(sys, "frozen", False)
           else os.path.dirname(os.path.abspath(__file__)))
FONT_DIR = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts")


def _data_dir():
    """%APPDATA%\\PC-Monitor: writable wherever the program itself is installed."""
    path = os.path.join(os.environ.get("APPDATA") or os.path.expanduser("~"), "PC-Monitor")
    try:
        os.makedirs(path, exist_ok=True)
        return path
    except OSError:
        return APP_DIR


DATA_DIR = _data_dir()
CONFIG_PATH = os.path.join(DATA_DIR, "widget_config.json")
OLD_CONFIG_PATH = os.path.join(APP_DIR, "widget_config.json")  # before 1.1
LOG_PATH = os.path.join(DATA_DIR, "pc-monitor.log")

log = logging.getLogger("pcmonitor")
_last_error = {}


def setup_logging():
    """Errors go to pc-monitor.log (max ~2 x 256 KB) instead of vanishing."""
    try:
        handler = logging.handlers.RotatingFileHandler(
            LOG_PATH, maxBytes=256 * 1024, backupCount=1, encoding="utf-8", delay=True)
    except OSError:
        return
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s [%(process)d] %(message)s"))
    log.addHandler(handler)
    log.setLevel(logging.INFO)
    sys.excepthook = lambda *exc: log.critical("uncaught error", exc_info=exc)
    threading.excepthook = lambda a: log.error(
        "thread %s crashed", a.thread.name if a.thread else "?",
        exc_info=(a.exc_type, a.exc_value, a.exc_traceback))


def log_error(where):
    """Call from an except block. Logs the traceback at most once a minute per
    place, so a fault that repeats every second does not flood the log."""
    now = time.monotonic()
    if now - _last_error.get(where, -1e9) >= 60:
        _last_error[where] = now
        log.exception("%s failed", where)


LHM_URL = "http://127.0.0.1:8085/data.json"  # this computer only
LHM_POLL_S = 3
UPDATE_MS = 1000
HISTORY_LEN = 60
SS = 3  # supersampling factor for shapes
HERO_H = 128  # header + CPU block, logical px
NET_GRAPH_FLOOR = 64 * 1024  # the network graph's scale never goes below 64 KB/s

# window colour that becomes fully transparent (rounded corners)
KEY = (1, 2, 3)
KEY_HEX = "#010203"

CARD = (28, 33, 43)
BORDER = (44, 51, 64)
TRACK = (40, 46, 58)
HIST_TRACK = (34, 40, 51)
TEXT = (232, 234, 239)
DIM = (139, 147, 163)
FAINT = (88, 96, 112)
CLOSE_HOVER = (232, 68, 90)

# colour scales: low load -> high load
THEMES = {
    "heat": ("Тепло", ["#2ec4b6", "#9ccc65", "#f2b33d", "#f27a3d", "#e8445a"]),
    "neon": ("Неон", ["#3a86ff", "#8338ec", "#d61f9a", "#ff2e63"]),
    "ice": ("Лёд", ["#4f6a8f", "#6f9fd8", "#a9cdf5", "#eef6ff"]),
}

def _windows_build():
    try:
        return sys.getwindowsversion().build
    except AttributeError:  # not Windows
        return 0


# Windows 11 has a new taskbar where the task buttons can't be narrowed, so the
# mini panel may cover icons there: it starts switched off (can be enabled in the menu)
IS_WIN11 = _windows_build() >= 22000

# run_as_admin is off by default: the widget asks for admin rights only when the
# user switches on temperatures in the menu (least privilege)
DEFAULT_CONFIG = {"x": None, "y": None, "topmost": True, "alpha": 1.0, "theme": "heat",
                  "pinned": True, "taskbar": not IS_WIN11, "alerts": True, "compact": False,
                  "run_as_admin": False, "hidden": [],
                  "alert_temp": 90, "alert_ram": 90, "alert_battery": 15}

# alerts fire at the limit chosen in the menu and re-arm ALERT_HYST below it
# (above it for the battery), so a value hovering around the limit doesn't spam;
# each alert at most once per ALERT_COOLDOWN_S. GPU limit = CPU limit - 3.
ALERT_HYST = 5
ALERT_COOLDOWN_S = 600
CLOSE_CONFIRM_MS = 3000


# ---------- helpers ----------

def hex_rgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


def lerp(c1, c2, t):
    return tuple(int(round(a + (b - a) * t)) for a, b in zip(c1, c2))


class Scale:
    """Maps 0..1 to a colour along the theme's stops."""

    def __init__(self, stops):
        self.stops = [hex_rgb(s) for s in stops]

    def __call__(self, t):
        t = max(0.0, min(1.0, t))
        n = len(self.stops) - 1
        i = min(int(t * n), n - 1)
        return lerp(self.stops[i], self.stops[i + 1], t * n - i)


def fmt_gb(n_bytes):
    return f"{n_bytes / 2**30:.1f}".replace(".", ",")


def fmt_speed(bytes_per_s):
    if bytes_per_s < 1024:
        return f"{bytes_per_s:.0f} Б/с"
    if bytes_per_s < 1024**2:
        return f"{bytes_per_s / 1024:.0f} КБ/с"
    return f"{bytes_per_s / 1024**2:.1f} МБ/с".replace(".", ",")


def fmt_mem(n_bytes):
    if n_bytes < 2**30:
        return f"{n_bytes / 2**20:.0f} МБ"
    return f"{n_bytes / 2**30:.1f} ГБ".replace(".", ",")


def ping_color(scale, ms):
    if ms is None:
        return scale(1.0)
    return scale(0.05 if ms < 50 else 0.55 if ms < 100 else 0.95)


def ellipsize(text, font, max_w):
    if font.getlength(text) <= max_w:
        return text
    while text and font.getlength(text + "…") > max_w:
        text = text[:-1]
    return text + "…"


def fmt_left(seconds):
    """Battery time left: '2 ч 15 мин', '45 мин'."""
    minutes = int(seconds) // 60
    hours, minutes = divmod(minutes, 60)
    return f"{hours} ч {minutes} мин" if hours else f"{minutes} мин"


def cpu_extra_text(data):
    """'3,2 ГГц · 12 Вт' from the sensors, or '' when they don't give it."""
    parts = []
    clock, power = data.get("cpu_clock"), data.get("cpu_power")
    if clock:
        parts.append(f"{clock:.1f} ГГц".replace(".", ","))
    if power:
        parts.append(f"{power:.0f} Вт" if power >= 10 else f"{power:.1f} Вт".replace(".", ","))
    return "  ·  ".join(parts)


def fmt_uptime(seconds):
    seconds = int(seconds)
    days, rest = divmod(seconds, 86400)
    hours, rest = divmod(rest, 3600)
    minutes = rest // 60
    if days:
        return f"{days} д {hours} ч"
    return f"{hours} ч {minutes} мин"


def sanitize_config(raw):
    """Only known keys with values of the right type: the file is user-writable
    and the widget may run as admin, so nothing in it is trusted."""
    cfg = json.loads(json.dumps(DEFAULT_CONFIG))  # fresh copy: no shared lists
    if not isinstance(raw, dict):
        return cfg
    for key, default in DEFAULT_CONFIG.items():
        if key not in raw:
            continue
        v = raw[key]
        if key in ("x", "y"):
            if isinstance(v, int) and not isinstance(v, bool) and abs(v) < 100_000:
                cfg[key] = v
        elif key == "alpha":
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                cfg[key] = min(1.0, max(0.3, float(v)))
        elif key == "theme":
            if v in THEMES:
                cfg[key] = v
        elif isinstance(default, bool):
            if isinstance(v, bool):
                cfg[key] = v
        elif key in CONFIG_CHOICES:
            if v in CONFIG_CHOICES[key]:
                cfg[key] = v
        elif key == "hidden":
            if isinstance(v, list):
                cfg[key] = [s for s in v if s in SECTIONS]
    return cfg


# key -> allowed values
CONFIG_CHOICES = {"alert_temp": (80, 85, 90, 95), "alert_ram": (80, 85, 90, 95),
                  "alert_battery": (10, 15, 20, 30)}
# sections that can be hidden from the menu: key -> menu title
SECTIONS = {"ram": "Память", "gpu": "Видеокарта", "disks": "Диски", "temps": "Температура",
            "net": "Сеть", "net_graph": "График сети", "ping": "Пинг",
            "disk_io": "Чтение и запись диска", "uptime": "Время без перезагрузки"}


def load_config():
    for path in (CONFIG_PATH, OLD_CONFIG_PATH):  # the old place is read once, to migrate
        try:
            with open(path, encoding="utf-8") as f:
                return sanitize_config(json.load(f))
        except FileNotFoundError:
            continue
        except (OSError, ValueError):
            log.warning("bad config %s, using defaults", path, exc_info=True)
            break
    return sanitize_config({})


def save_config(cfg):
    """Write to a temp file and swap it in, so a crash never leaves half a file."""
    tmp = CONFIG_PATH + ".tmp"
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(cfg, f, indent=2)
        os.replace(tmp, CONFIG_PATH)
    except OSError:
        log_error("saving config")


# ---------- pinning to the desktop (Win32) ----------
# A window owned by the desktop icon view (SHELLDLL_DefView) lives in the
# desktop layer: other windows cover it, and Win+D (which raises the desktop)
# brings it along instead of hiding it.

GWLP_HWNDPARENT = -8
GW_OWNER = 4
HWND_BOTTOM = 1
SWP_FLAGS = 0x1 | 0x2 | 0x10  # NOSIZE | NOMOVE | NOACTIVATE

try:
    from ctypes import wintypes as _wt
    _user32 = ctypes.WinDLL("user32")
    _user32.FindWindowW.argtypes = [_wt.LPCWSTR, _wt.LPCWSTR]
    _user32.FindWindowW.restype = _wt.HWND
    _user32.FindWindowExW.argtypes = [_wt.HWND, _wt.HWND, _wt.LPCWSTR, _wt.LPCWSTR]
    _user32.FindWindowExW.restype = _wt.HWND
    _user32.SetWindowLongPtrW.argtypes = [_wt.HWND, ctypes.c_int, ctypes.c_ssize_t]
    _user32.SetWindowLongPtrW.restype = ctypes.c_ssize_t
    _user32.GetWindow.argtypes = [_wt.HWND, _wt.UINT]
    _user32.GetWindow.restype = _wt.HWND
    _user32.IsWindow.argtypes = [_wt.HWND]
    _user32.SetWindowPos.argtypes = [_wt.HWND, _wt.HWND, ctypes.c_int, ctypes.c_int,
                                     ctypes.c_int, ctypes.c_int, _wt.UINT]
    _user32.GetWindowRect.argtypes = [_wt.HWND, ctypes.POINTER(_wt.RECT)]
    _user32.ScreenToClient.argtypes = [_wt.HWND, ctypes.POINTER(_wt.POINT)]
    _user32.GetForegroundWindow.restype = _wt.HWND
    _user32.GetClassNameW.argtypes = [_wt.HWND, _wt.LPWSTR, ctypes.c_int]
except (AttributeError, OSError):  # not Windows
    _user32 = None


def find_desktop_view():
    """HWND of the desktop icon view, or None."""
    if _user32 is None:
        return None
    progman = _user32.FindWindowW("Progman", None)
    view = _user32.FindWindowExW(progman, None, "SHELLDLL_DefView", None)
    worker = None
    while not view:  # with a wallpaper slideshow the view lives in a WorkerW
        worker = _user32.FindWindowExW(None, worker, "WorkerW", None)
        if not worker:
            return None
        view = _user32.FindWindowExW(worker, None, "SHELLDLL_DefView", None)
    return view


def set_owner(hwnd, owner):
    if _user32 is not None:
        _user32.SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner or 0)


def owner_alive(hwnd):
    if _user32 is None:
        return False
    owner = _user32.GetWindow(hwnd, GW_OWNER)
    return bool(owner) and bool(_user32.IsWindow(owner))


def send_to_back(hwnd):
    """Bottom of the z-order; an owned window still stays above its owner."""
    if _user32 is not None:
        _user32.SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_FLAGS)


# ---------- taskbar (Win32) ----------
# The mini panel is a separate topmost window owned by the taskbar
# (Shell_TrayWnd), so it always stays above it. To make room, the task
# button area (MSTaskSwWClass) is narrowed — the same trick TrafficMonitor
# uses — and restored on exit.

SWP_NOZORDER = 0x4
SWP_NOACTIVATE = 0x10
SWP_ASYNC = 0x4000  # never block on explorer.exe


def window_rect(hwnd):
    r = _wt.RECT()
    _user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r.left, r.top, r.right, r.bottom


def class_name(hwnd):
    buf = ctypes.create_unicode_buffer(64)
    _user32.GetClassNameW(hwnd, buf, 64)
    return buf.value


def find_taskbar():
    """(taskbar, tray_notify, task_list, rebar) handles; missing ones are None."""
    if _user32 is None:
        return None, None, None, None
    bar = _user32.FindWindowW("Shell_TrayWnd", None)
    if not bar:
        return None, None, None, None
    notify = _user32.FindWindowExW(bar, None, "TrayNotifyWnd", None)
    rebar = _user32.FindWindowExW(bar, None, "ReBarWindow32", None)
    tasks = _user32.FindWindowExW(rebar, None, "MSTaskSwWClass", None) if rebar else None
    return bar, notify, tasks, rebar


def set_child_right(child, parent, right_screen):
    """Resize a child window so its right edge is at right_screen (screen px)."""
    left, top, _, bottom = window_rect(child)
    pt = _wt.POINT(left, top)
    _user32.ScreenToClient(parent, ctypes.byref(pt))
    width = max(0, right_screen - left)
    _user32.SetWindowPos(child, None, pt.x, pt.y, width, bottom - top,
                         SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNC)


def fullscreen_app_active():
    """True when the foreground window covers the whole primary screen (games, video)."""
    fg = _user32.GetForegroundWindow()
    if not fg or class_name(fg) in ("Progman", "WorkerW", "Shell_TrayWnd"):
        return False
    left, top, right, bottom = window_rect(fg)
    gsm = ctypes.windll.user32.GetSystemMetrics
    return left <= 0 and top <= 0 and right >= gsm(0) and bottom >= gsm(1)


def taskbar_uses_light_theme():
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize") as key:
            return winreg.QueryValueEx(key, "SystemUsesLightTheme")[0] == 1
    except OSError:
        return False


# ---------- autostart & hotkey ----------

RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
RUN_NAME = "PCMonitorWidget"


def autostart_command():
    exe, args = launch_target()
    return f'"{exe}" {args}'.strip()


TASK_NAME = "PCMonitorWidget"


def _system32():
    """System folder from the kernel, not from an environment variable."""
    buf = ctypes.create_unicode_buffer(260)
    try:
        if ctypes.windll.kernel32.GetSystemDirectoryW(buf, 260):
            return buf.value
    except (AttributeError, OSError):
        pass
    return r"C:\Windows\System32"


def _program_files():
    """Program Files from the shell (FOLDERID_ProgramFiles): %ProgramFiles% could be faked."""
    try:
        from ctypes import wintypes as wt

        class GUID(ctypes.Structure):
            _fields_ = [("a", wt.DWORD), ("b", wt.WORD), ("c", wt.WORD), ("d", ctypes.c_ubyte * 8)]

        guid, out = GUID(), ctypes.c_wchar_p()
        ctypes.oledll.ole32.CLSIDFromString("{905E63B6-C1BF-494E-B29C-65B732D3D21A}",
                                            ctypes.byref(guid))
        ctypes.oledll.shell32.SHGetKnownFolderPath(ctypes.byref(guid), 0, None, ctypes.byref(out))
        try:
            return out.value
        finally:
            ctypes.windll.ole32.CoTaskMemFree(out)
    except (AttributeError, OSError):
        return r"C:\Program Files"


# full path: a bare "schtasks" would also be looked up in the current folder
SCHTASKS = os.path.join(_system32(), "schtasks.exe")


# ---------- protected copy for admin mode ----------
# Whatever runs with admin rights must not be replaceable without them, or any
# program running as the user could swap a file (the .exe, a DLL) and get admin
# rights at the next start. So in admin mode the built program runs from a copy
# in Program Files, and only that copy may autostart elevated (scheduled task,
# no UAC prompt). The script version can't be protected like that — Python and
# its packages live in user-writable folders — so it never gets the task.

INSTALL_DIR = os.path.join(_program_files(), "PC-Monitor")
EXE_NAME = "PC-Monitor.exe"


def is_frozen():
    return bool(getattr(sys, "frozen", False))


def _same_path(a, b):
    return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


def running_installed():
    """True when this is the protected copy in Program Files."""
    return is_frozen() and _same_path(APP_DIR, INSTALL_DIR)


def installed_exe():
    path = os.path.join(INSTALL_DIR, EXE_NAME)
    return path if os.path.isfile(path) else None


def build_fingerprint(folder, exe):
    """Hash of the .exe (it carries the code) plus names and sizes of _internal files."""
    import hashlib
    h = hashlib.sha256()
    with open(exe, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    internal = os.path.join(folder, "_internal")
    for root, dirs, files in os.walk(internal):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(root, name)
            h.update(f"{os.path.relpath(path, internal)}:{os.path.getsize(path)}\n".encode())
    return h.hexdigest()


def installed_is_current():
    """The Program Files copy is this very build."""
    exe = installed_exe()
    if not exe or not is_frozen():
        return False
    if running_installed():
        return True
    try:
        return build_fingerprint(INSTALL_DIR, exe) == build_fingerprint(APP_DIR, sys.executable)
    except OSError:
        return False


def install_copy():
    """Copy this build (the .exe and _internal) to Program Files. Needs admin rights."""
    import shutil
    src_internal = os.path.join(APP_DIR, "_internal")
    if not is_frozen() or running_installed() or not os.path.isdir(src_internal):
        return False
    new, old = INSTALL_DIR + ".new", INSTALL_DIR + ".old"
    try:
        for path in (new, old):
            shutil.rmtree(path, ignore_errors=True)
        os.makedirs(new)
        shutil.copy2(sys.executable, os.path.join(new, EXE_NAME))
        shutil.copytree(src_internal, os.path.join(new, "_internal"))
        if os.path.exists(INSTALL_DIR):
            os.rename(INSTALL_DIR, old)
        os.rename(new, INSTALL_DIR)
    except OSError:
        log.exception("installing to %s failed", INSTALL_DIR)
        shutil.rmtree(new, ignore_errors=True)
        if not os.path.exists(INSTALL_DIR) and os.path.exists(old):
            os.rename(old, INSTALL_DIR)  # put the previous copy back
        return False
    shutil.rmtree(old, ignore_errors=True)
    log.info("installed %s to %s", APP_VERSION, INSTALL_DIR)
    return True


def start_installed():
    """Start the Program Files copy (it inherits our admin rights)."""
    import subprocess
    exe = installed_exe()
    if not exe:
        return False
    try:
        subprocess.Popen([exe], cwd=INSTALL_DIR, close_fds=True)
        return True
    except OSError:
        log.exception("starting %s failed", exe)
        return False


def launch_target():
    """(program, arguments) that start this widget."""
    if is_frozen():
        return sys.executable, ""
    exe = sys.executable
    windowless = os.path.join(os.path.dirname(exe), "pythonw.exe")
    if os.path.exists(windowless):
        exe = windowless
    return exe, f'"{os.path.abspath(__file__)}"'


def elevation_target():
    """What to start with admin rights: the protected copy when it is this build."""
    if is_frozen() and not running_installed() and installed_is_current():
        return installed_exe(), ""
    return launch_target()


def relaunch_as_admin():
    """Ask UAC to restart us elevated. True if the elevated copy was started."""
    exe, args = elevation_target()
    try:
        rc = ctypes.windll.shell32.ShellExecuteW(None, "runas", exe, args, os.path.dirname(exe), 1)
    except (AttributeError, OSError):
        return False
    return rc > 32  # <= 32: error, e.g. the user pressed "No"


def _schtasks(*args, output=False):
    import subprocess
    r = subprocess.run([SCHTASKS, *args], capture_output=True,
                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    if output:
        return r.stdout if r.returncode == 0 else None
    return r.returncode == 0


def task_xml():
    """Logon task with highest privileges: elevated autostart without a UAC prompt.
    Battery and time limits are switched off (schtasks defaults would stop it on laptops)."""
    from xml.sax.saxutils import escape
    exe, args = launch_target()
    user = os.environ.get("USERDOMAIN", "") + "\\" + os.environ.get("USERNAME", "")
    return f"""<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{escape(user)}</UserId>
    <Delay>PT5S</Delay></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{escape(user)}</UserId>
    <LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{escape(exe)}</Command>
    <Arguments>{escape(args)}</Arguments><WorkingDirectory>{escape(APP_DIR)}</WorkingDirectory></Exec></Actions>
</Task>"""


def admin_task_enabled():
    return _schtasks("/Query", "/TN", TASK_NAME)


def admin_task_command():
    """Program the elevated autostart task starts, or None when there is no task."""
    raw = _schtasks("/Query", "/TN", TASK_NAME, "/XML", output=True)
    if raw is None:
        return None
    if raw[:2] in (b"\xff\xfe", b"\xfe\xff"):
        text = raw.decode("utf-16")
    else:
        text = raw.decode("mbcs", "replace")
    m = re.search(r"<Command>(.*?)</Command>", text, re.S)
    if not m:
        return ""
    from xml.sax.saxutils import unescape
    return unescape(m.group(1)).strip().strip('"')


def task_allowed():
    """Only the protected Program Files copy may autostart with admin rights."""
    return running_installed()


def set_admin_task(enable):
    """Needs admin rights (we have them when this is called)."""
    if not enable:
        return _schtasks("/Delete", "/TN", TASK_NAME, "/F") or not admin_task_enabled()
    if not task_allowed():
        return False
    # the XML goes into the protected program folder, not %TEMP%: there another
    # program could swap it between our write and schtasks reading it
    path = os.path.join(INSTALL_DIR, "autostart-task.xml")
    try:
        with open(path, "w", encoding="utf-16") as f:
            f.write(task_xml())
        return _schtasks("/Create", "/TN", TASK_NAME, "/XML", path, "/F")
    finally:
        try:
            os.remove(path)
        except OSError:
            pass


def check_autostart(prefer_task=False):
    """At start: keep the Run entry pointing at this program, drop an elevated task
    that would start something an ordinary program could replace, and (admin mode,
    protected copy) move autostart to the task, so logon shows no UAC prompt."""
    if autostart_enabled():
        set_autostart(True)
    if not is_admin():
        return
    cmd = admin_task_command()
    installed = installed_exe()
    if cmd is not None and not (installed and cmd and _same_path(cmd, installed)):
        log.warning("removing the elevated autostart task for %r (not a protected copy)", cmd)
        if set_admin_task(False):
            set_autostart(True)  # keep starting with Windows, just without silent admin rights
            cmd = None
    if prefer_task and cmd is None and running_installed() and autostart_enabled():
        if set_admin_task(True):
            set_autostart(False)
            log.info("autostart moved to the elevated task of the protected copy")


def autostart_enabled(name=RUN_NAME):
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as key:
            winreg.QueryValueEx(key, name)
            return True
    except OSError:
        return False


def set_autostart(enable, name=RUN_NAME):
    """Per-user Run key: no admin rights needed."""
    import winreg
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0, winreg.KEY_SET_VALUE) as key:
        if enable:
            winreg.SetValueEx(key, name, 0, winreg.REG_SZ, autostart_command())
        else:
            try:
                winreg.DeleteValue(key, name)
            except FileNotFoundError:
                pass


HOTKEY_TEXT = "Ctrl+Alt+W"
MOD_ALT, MOD_CONTROL, MOD_NOREPEAT = 0x1, 0x2, 0x4000
VK_W = 0x57


class HotkeyListener(threading.Thread):
    """Global hotkey via RegisterHotKey; needs its own Win32 message loop."""

    def __init__(self, mods=MOD_CONTROL | MOD_ALT, vk=VK_W):
        super().__init__(daemon=True)
        self.mods, self.vk = mods, vk
        self.pressed = threading.Event()
        self.ok = None  # False if another program already owns the combination

    def run(self):
        try:
            from ctypes import wintypes as wt
            u = ctypes.WinDLL("user32")
            u.RegisterHotKey.argtypes = [wt.HWND, ctypes.c_int, wt.UINT, wt.UINT]
            u.GetMessageW.argtypes = [ctypes.POINTER(wt.MSG), wt.HWND, wt.UINT, wt.UINT]
            if not u.RegisterHotKey(None, 1, self.mods | MOD_NOREPEAT, self.vk):
                self.ok = False
                return
            self.ok = True
            msg = wt.MSG()
            while u.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
                if msg.message == 0x0312:  # WM_HOTKEY
                    self.pressed.set()
        except (AttributeError, OSError):
            self.ok = False


def virtual_screen(root):
    """Bounding box of all monitors: (left, top, width, height)."""
    try:
        gsm = ctypes.windll.user32.GetSystemMetrics
        return gsm(76), gsm(77), gsm(78), gsm(79)
    except (AttributeError, OSError):
        return 0, 0, root.winfo_screenwidth(), root.winfo_screenheight()


def load_font(size, style="Regular"):
    try:
        font = ImageFont.truetype(os.path.join(FONT_DIR, "bahnschrift.ttf"), size)
        try:
            font.set_variation_by_name(style)
        except (OSError, ValueError):
            pass
        return font
    except OSError:
        name = "seguisb.ttf" if "Bold" in style else "segoeui.ttf"
        try:
            return ImageFont.truetype(os.path.join(FONT_DIR, name), size)
        except OSError:
            return ImageFont.load_default()


def load_symbol_font(size):
    try:
        return ImageFont.truetype(os.path.join(FONT_DIR, "seguisym.ttf"), size)
    except OSError:
        return load_font(size)


# ---------- LibreHardwareMonitor ----------

_NUM_RE = re.compile(r"-?\d+(?:[.,]\d+)?")
CPU_PREFS = ("cpu package", "core (tctl/tdie)", "core average", "cpu die", "cpu")
POWER_PREFS = ("package", "cpu package")
GPU_PREFS = ("gpu core", "gpu hot spot", "gpu")


def _parse_num(text):
    m = _NUM_RE.search(text or "")
    return float(m.group().replace(",", ".")) if m else None


SENSOR_GROUPS = {"Temperatures", "Load", "Clocks", "Powers", "Voltages", "Data", "SmallData",
                 "Fans", "Controls", "Throughput", "Levels", "Factors", "Currents", "Energy",
                 "Noise"}
TEMP_SKIP = ("distance", "warning", "critical", "limit")  # thresholds, not readings
STORAGE_PREFS = ("composite temperature", "temperature", "drive temperature")
INTEGRATED_GPU = re.compile(r"radeon\(tm\) graphics|radeon graphics|vega \d|uhd|iris|intel", re.I)


def _classify_hardware(node):
    img = (node.get("ImageURL") or "").lower()
    text = (node.get("Text") or "").lower()
    if "cpu" in img:
        return "cpu"
    if any(k in img for k in ("nvidia", "ati", "amd", "gpu")):
        return "gpu"
    if any(k in img for k in ("hdd", "ssd", "nvme")):
        return "storage"
    if any(k in text for k in ("geforce", "radeon", "graphics", "gpu")):
        return "gpu"
    return None


def _sensor_kind(sensor_id, hw_kind):
    sid = (sensor_id or "").lower()
    if "cpu/" in sid:
        return "cpu"
    if "/gpu" in sid:
        return "gpu"
    if any(k in sid for k in ("/nvme/", "/hdd/", "/ssd/", "/ata/")):
        return "storage"
    return hw_kind


def _pick(sensors, prefs):
    for pref in prefs:
        for name, value in sensors:
            if name.startswith(pref):
                return value
    return max((v for _, v in sensors), default=None)


def read_lhm(tree):
    """Summarise LHM data.json: {"cpu", "gpu", "gpu_shared", "gpu_load", "gpu_name", "ssd"}.
    gpu_shared = integrated graphics without its own sensor (shares the CPU die)."""
    hardware = []

    def walk(node, hw, group):
        kids = node.get("Children") or []
        text = (node.get("Text") or "").strip()
        if not kids:
            value = _parse_num(node.get("Value"))
            if hw is None or group is None or value is None:
                return
            low = text.lower()
            if group == "Temperatures":
                if 0 < value < 150 and not any(w in low for w in TEMP_SKIP):
                    hw["temps"].append((low, value, _sensor_kind(node.get("SensorId"), hw["kind"])))
            elif group == "Load":
                hw["loads"].append((low, value))
            elif group == "Clocks":
                hw["clocks"].append((low, value))
            elif group == "Powers":
                hw["powers"].append((low, value))
            return
        if text in SENSOR_GROUPS:
            for kid in kids:
                walk(kid, hw, text)
            return
        new_hw = {"kind": _classify_hardware(node), "name": text, "temps": [], "loads": [],
                  "clocks": [], "powers": [], "integrated": False}
        hardware.append(new_hw)
        for kid in kids:
            walk(kid, new_hw, None)
            sid = (kid.get("SensorId") or "").lower()
            if "integrated" in sid:
                new_hw["integrated"] = True

    walk(tree, None, None)
    temps = [(name, v, kind) for hw in hardware for name, v, kind in hw["temps"]]
    cpu = _pick([(n, v) for n, v, k in temps if k == "cpu"], CPU_PREFS)

    gpus = [hw for hw in hardware if hw["kind"] == "gpu"]
    primary = next((g for g in gpus if any(k == "gpu" for _, _, k in g["temps"])),
                   gpus[0] if gpus else None)
    gpu = gpu_load = gpu_name = None
    gpu_shared = False
    if primary:
        gpu = _pick([(n, v) for n, v, k in primary["temps"] if k == "gpu"], GPU_PREFS)
        loads = dict(primary["loads"])
        main = [loads[k] for k in ("gpu core", "d3d 3d") if k in loads]
        gpu_load = max(main) if main else max(
            (v for n, v in primary["loads"] if n.startswith("gpu")), default=None)
        gpu_name = re.sub(r"\((tm|r)\)", "", primary["name"], flags=re.I)
        gpu_name = re.sub(r"\s+", " ", gpu_name).strip()
        integrated = primary["integrated"] or bool(INTEGRATED_GPU.search(primary["name"]))
        gpu_shared = gpu is None and integrated and cpu is not None

    ssds = [_pick([(n, v) for n, v, k in hw["temps"] if k == "storage"], STORAGE_PREFS)
            for hw in hardware if any(k == "storage" for _, _, k in hw["temps"])]
    ssd = max((t for t in ssds if t is not None), default=None)

    cpu_clock = cpu_power = None
    cpu_hw = next((hw for hw in hardware if hw["kind"] == "cpu"), None)
    if cpu_hw:
        cores = [v for n, v in cpu_hw["clocks"] if n.startswith("core #") and v > 100]
        cpu_clock = sum(cores) / len(cores) / 1000 if cores else None  # MHz -> GHz
        power = _pick(cpu_hw["powers"], POWER_PREFS)
        cpu_power = power if power and 0 < power < 1000 else None
    return {"cpu": cpu, "gpu": gpu, "gpu_shared": gpu_shared, "gpu_load": gpu_load,
            "gpu_name": gpu_name, "ssd": ssd, "cpu_clock": cpu_clock, "cpu_power": cpu_power}


def extract_temps(tree):
    """(cpu_temp, gpu_temp) — kept for older callers/tests."""
    r = read_lhm(tree)
    return r["cpu"], r["gpu"]


EMPTY_SENSORS = {"cpu": None, "gpu": None, "gpu_shared": False, "gpu_load": None,
                 "gpu_name": None, "ssd": None, "cpu_clock": None, "cpu_power": None}


def is_admin():
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except (AttributeError, OSError):
        return False


def lhm_dir():
    """Folder with LibreHardwareMonitorLib.dll (bundled into the .exe, or ./lhm)."""
    base = getattr(sys, "_MEIPASS", APP_DIR)
    path = os.path.join(base, "lhm")
    return path if os.path.exists(os.path.join(path, "LibreHardwareMonitorLib.dll")) else None


class NativeSensors:
    """Reads sensors in-process with LibreHardwareMonitorLib (via pythonnet).
    Without admin rights it still gives GPU load; temperatures need admin and
    the PawnIO driver. Produces the same tree as LHM's web server, so
    read_lhm() handles both."""

    ICONS = {"Cpu": "cpu", "GpuNvidia": "nvidia", "GpuAmd": "ati", "GpuIntel": "intel",
             "Storage": "hdd"}
    GROUPS = {"Temperature": ("Temperatures", " °C"), "Load": ("Load", " %"),
              "Clock": ("Clocks", " MHz"), "Power": ("Powers", " W")}

    def __init__(self, path):
        from pythonnet import load
        load("netfx")
        import clr
        if path not in sys.path:
            sys.path.append(path)
        clr.AddReference("LibreHardwareMonitorLib")
        from LibreHardwareMonitor.Hardware import Computer
        self.computer = Computer()
        self.computer.IsCpuEnabled = True
        self.computer.IsGpuEnabled = True
        self.computer.IsStorageEnabled = True
        self.computer.Open()

    def _node(self, hw):
        hw.Update()
        groups = {}
        for sensor in hw.Sensors:
            group = self.GROUPS.get(str(sensor.SensorType))
            if group is None or sensor.Value is None:
                continue
            name, unit = group
            groups.setdefault(name, []).append({"Text": sensor.Name,
                                                "Value": f"{float(sensor.Value):.1f}{unit}",
                                                "SensorId": str(sensor.Identifier)})
        children = [{"Text": g, "Children": leaves} for g, leaves in groups.items()]
        children += [self._node(sub) for sub in hw.SubHardware]
        icon = self.ICONS.get(str(hw.HardwareType), "chip")
        return {"Text": hw.Name, "ImageURL": f"images_icon/{icon}.png", "Children": children}

    def tree(self):
        return {"Text": "Sensor", "Children": [
            {"Text": "PC", "ImageURL": "images_icon/computer.png",
             "Children": [self._node(hw) for hw in self.computer.Hardware]}]}


def _fetch_web():
    with urllib.request.urlopen(LHM_URL, timeout=1) as resp:
        return read_lhm(json.loads(resp.read().decode("utf-8")))


class LhmReader(threading.Thread):
    """Background sensor polling, best source first:
    1. built-in LibreHardwareMonitorLib (full data when running as admin),
    2. LibreHardwareMonitor's web server (if the user runs that program),
    so the UI never waits on either."""

    def __init__(self):
        super().__init__(daemon=True)
        self.sensors = dict(EMPTY_SENSORS)
        self.online = False
        self.source = None   # "native", "web" or None
        self.native_ok = False
        self.admin = is_admin()

    @property
    def cpu(self):
        return self.sensors["cpu"]

    @property
    def gpu(self):
        return self.sensors["gpu"]

    def hint(self):
        """What the user can do to get temperatures, or None when they are there."""
        s = self.sensors
        if any(s[k] is not None for k in ("cpu", "gpu", "ssd")):
            return None
        if self.native_ok and not self.admin:
            return "Температуры включаются в меню (ПКМ)"
        if self.native_ok:
            return "Нет доступа к датчикам: нужен PawnIO"
        if self.online:
            return "Запусти LibreHardwareMonitor от админа"
        return "Нужен LibreHardwareMonitor с веб-сервером"

    def run(self):
        native = None
        path = lhm_dir()
        if path:
            try:
                native = NativeSensors(path)
                self.native_ok = True
            except Exception:  # no .NET / pythonnet / broken DLLs: fall back to the web server
                log.exception("built-in sensor library failed to load")
                native = None
        while True:
            found, source = None, None
            if native is not None:
                try:
                    found, source = read_lhm(native.tree()), "native"
                except Exception:
                    log_error("reading built-in sensors")
                    found = None
            if found is None or found["cpu"] is None:
                try:
                    web = _fetch_web()
                    if found is None or web["cpu"] is not None:
                        merged = dict(web)
                        for key, value in (found or {}).items():
                            if merged.get(key) is None:
                                merged[key] = value
                        found, source = merged, "web"
                except (OSError, ValueError):
                    pass
            self.sensors = found or dict(EMPTY_SENSORS)
            self.online = found is not None
            self.source = source
            time.sleep(LHM_POLL_S)


# ---------- ping & processes ----------

PING_HOST = "1.1.1.1"  # Cloudflare DNS: fast and always up
PING_EVERY_S = 2


class PingReader(threading.Thread):
    """Real ICMP ping via Windows IcmpSendEcho (works without admin rights)."""

    def __init__(self, host=PING_HOST):
        super().__init__(daemon=True)
        self.host = host
        self.ms = None      # last round-trip time, None = no reply
        self.ready = False  # at least one attempt finished

    def run(self):
        try:
            icmp = ctypes.WinDLL("iphlpapi")
            icmp.IcmpCreateFile.restype = ctypes.c_void_p
            icmp.IcmpSendEcho.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_void_p,
                                          ctypes.c_ushort, ctypes.c_void_p, ctypes.c_void_p,
                                          ctypes.c_ulong, ctypes.c_ulong]
            icmp.IcmpSendEcho.restype = ctypes.c_ulong
            handle = icmp.IcmpCreateFile()
        except (AttributeError, OSError):
            return
        payload = ctypes.create_string_buffer(b"pc-monitor", 10)
        reply = ctypes.create_string_buffer(256)
        while True:
            try:
                addr = struct.unpack("<I", socket.inet_aton(socket.gethostbyname(self.host)))[0]
                got = icmp.IcmpSendEcho(handle, addr, payload, 10, None, reply, 256, 1000)
                # ICMP_ECHO_REPLY: Address, Status, RoundTripTime (ULONGs)
                _, status, rtt = struct.unpack_from("<III", reply.raw)
                self.ms = rtt if got and status == 0 else None
            except OSError:
                self.ms = None
            self.ready = True
            time.sleep(PING_EVERY_S)


class _UnicodeString(ctypes.Structure):
    _fields_ = [("Length", ctypes.c_ushort), ("MaximumLength", ctypes.c_ushort),
                ("Buffer", ctypes.c_void_p)]


class _ProcInfo(ctypes.Structure):
    """Head of SYSTEM_PROCESS_INFORMATION (ntdll); ctypes handles the padding."""
    _fields_ = [
        ("NextEntryOffset", ctypes.c_ulong),
        ("NumberOfThreads", ctypes.c_ulong),
        ("WorkingSetPrivateSize", ctypes.c_longlong),
        ("HardFaultCount", ctypes.c_ulong),
        ("NumberOfThreadsHighWatermark", ctypes.c_ulong),
        ("CycleTime", ctypes.c_ulonglong),
        ("CreateTime", ctypes.c_longlong),
        ("UserTime", ctypes.c_longlong),
        ("KernelTime", ctypes.c_longlong),
        ("ImageName", _UnicodeString),
        ("BasePriority", ctypes.c_long),
        ("UniqueProcessId", ctypes.c_void_p),
    ]


class ProcessSampler:
    """Per-program CPU and memory, like Task Manager: one NtQuerySystemInformation
    call returns every process at once (~ms), instead of opening each process
    (psutil needs ~0.5 s for that on Windows because of protected processes).
    Memory = private working set, the number Task Manager shows."""

    def __init__(self):
        self.ntdll = ctypes.WinDLL("ntdll")
        self.ntdll.NtQuerySystemInformation.restype = ctypes.c_long
        self.buf = ctypes.create_string_buffer(1 << 20)
        self.prev = {}
        self.prev_t = None
        self.ncpu = psutil.cpu_count() or 1

    def _snapshot(self):
        ret = ctypes.c_ulong()
        while True:
            status = self.ntdll.NtQuerySystemInformation(
                5, self.buf, len(self.buf), ctypes.byref(ret))  # SystemProcessInformation
            if status == 0:
                break
            if status & 0xFFFFFFFF == 0xC0000004:  # STATUS_INFO_LENGTH_MISMATCH
                self.buf = ctypes.create_string_buffer(max(ret.value, len(self.buf)) * 2)
                continue
            raise OSError(f"NtQuerySystemInformation failed: {status & 0xFFFFFFFF:#x}")
        base = ctypes.addressof(self.buf)
        offset = 0
        while True:
            info = _ProcInfo.from_address(base + offset)
            pid = info.UniqueProcessId or 0
            name = (ctypes.wstring_at(info.ImageName.Buffer, info.ImageName.Length // 2)
                    if info.ImageName.Buffer else "")
            yield pid, name, info.UserTime + info.KernelTime, info.WorkingSetPrivateSize, info.CreateTime
            if not info.NextEntryOffset:
                break
            offset += info.NextEntryOffset

    def top(self, limit=5, key="cpu"):
        """[(name, cpu_percent_of_whole_pc, ram_bytes)], same-name processes summed."""
        now = time.monotonic()
        if self.prev_t and now - self.prev_t < 0.8 and getattr(self, "groups", None):
            groups = self.groups  # sampled moments ago: reuse instead of a noisy tiny delta
            idx = 0 if key == "cpu" else 1
            rows = sorted(groups.items(), key=lambda kv: (kv[1][idx], kv[1][1 - idx]), reverse=True)
            return [(name, cpu, mem) for name, (cpu, mem) in rows[:limit]]
        dt = (now - self.prev_t) if self.prev_t else None
        groups, current = {}, {}
        for pid, name, cpu_time, mem, created in self._snapshot():
            if pid == 0:  # idle
                continue
            current[pid] = (created, cpu_time)
            cpu = 0.0
            before = self.prev.get(pid)
            if dt and before and before[0] == created:
                cpu = max(0.0, (cpu_time - before[1]) / 1e7 / dt / self.ncpu * 100)
            g = groups.setdefault(name.removesuffix(".exe") or "?", [0.0, 0])
            g[0] += cpu
            g[1] += mem
        self.prev, self.prev_t, self.groups = current, now, groups
        idx = 0 if key == "cpu" else 1
        rows = sorted(groups.items(), key=lambda kv: (kv[1][idx], kv[1][1 - idx]), reverse=True)
        return [(name, cpu, mem) for name, (cpu, mem) in rows[:limit]]


_sampler = None


def top_processes(limit=5, key="cpu"):
    global _sampler
    if _sampler is None:
        _sampler = ProcessSampler()
    return _sampler.top(limit, key)


# ---------- rendering ----------

class Renderer:
    """Draws the widget into a Pillow image. All sizes are in logical px."""

    PAD = 16
    WIDTH = 264
    RADIUS = 14

    def __init__(self, scale):
        self.s = scale
        self.W = self.px(self.WIDTH)
        self.f_title = load_font(self.px(14), "SemiBold")
        self.f_hero = load_font(self.px(46), "SemiBold Condensed")
        self.f_hero_unit = load_font(self.px(20), "SemiBold Condensed")
        self.f_label = load_font(self.px(12))
        self.f_value = load_font(self.px(12), "SemiBold")
        self.f_small = load_font(self.px(11))
        self.f_sym = load_symbol_font(self.px(13))
        self._card_cache = {}
        self.close_box = (0, 0, 0, 0)
        self.pin_box = (0, 0, 0, 0)
        self.hero_box = (0, 0, 0, 0)

    def px(self, v):
        return int(round(v * self.s))

    # --- static card (cached per height) ---
    def _card(self, H):
        if H in self._card_cache:
            return self._card_cache[H]
        W, S = self.W, SS
        big = Image.new("RGB", (W * S, H * S), KEY)
        d = ImageDraw.Draw(big)
        r = self.px(self.RADIUS) * S
        d.rounded_rectangle((0, 0, W * S - 1, H * S - 1), radius=r, fill=BORDER)
        bw = max(1, self.px(1)) * S
        d.rounded_rectangle((bw, bw, W * S - 1 - bw, H * S - 1 - bw), radius=r - bw, fill=CARD)
        card = big.reduce(S)
        self._card_cache = {H: card}
        return card

    # --- primitives on the supersampled layer (logical coords in) ---
    def _ss(self, *vals):
        return [v * self.s * SS for v in vals]

    def _rrect(self, d, x0, y0, x1, y1, r, fill):
        d.rounded_rectangle(self._ss(x0, y0, x1, y1), radius=r * self.s * SS, fill=fill)

    def _segment_meter(self, d, x0, y, x1, pct, scale, segments=24, h=6, gap=2):
        seg_w = (x1 - x0 - gap * (segments - 1)) / segments
        lit = int(round(pct / 100 * segments))
        for i in range(segments):
            sx = x0 + i * (seg_w + gap)
            color = scale(i / (segments - 1)) if i < lit else TRACK
            self._rrect(d, sx, y, sx + seg_w, y + h, 1.5, color)

    def _histogram(self, d, x0, y0, x1, y1, values, scale):
        pitch = (x1 - x0) / HISTORY_LEN
        bar_w = pitch * 0.62
        offset = HISTORY_LEN - len(values)
        for i in range(HISTORY_LEN):
            bx = x0 + i * pitch
            d.rectangle(self._ss(bx, y0, bx + bar_w, y1), fill=HIST_TRACK)
            j = i - offset
            if j < 0:
                continue
            v = max(0.0, min(100.0, values[j]))
            top = y1 - max(1.5, (y1 - y0) * v / 100)
            d.rectangle(self._ss(bx, top, bx + bar_w, y1), fill=scale(v / 100))

    def _temp_strip(self, d, x0, y, x1, cpu_t, gpu_t, scale, online):
        lo, hi = 30, 100
        steps = 70
        for i in range(steps):
            t = i / steps
            c = scale(t) if online else TRACK
            c = lerp(CARD, c, 0.55)
            sx0 = x0 + (x1 - x0) * i / steps
            sx1 = x0 + (x1 - x0) * (i + 1) / steps + 0.4
            d.rectangle(self._ss(sx0, y, sx1, y + 4), fill=c)
        for deg in range(40, 100, 10):
            tx = x0 + (x1 - x0) * (deg - lo) / (hi - lo)
            d.rectangle(self._ss(tx - 0.5, y + 6, tx + 0.5, y + 9), fill=FAINT)
        for temp, color in ((gpu_t, DIM), (cpu_t, TEXT)):
            if temp is None:
                continue
            mx = x0 + (x1 - x0) * (max(lo, min(hi, temp)) - lo) / (hi - lo)
            self._rrect(d, mx - 2.5, y - 4, mx + 2.5, y + 8, 2.5, CARD)
            self._rrect(d, mx - 1.5, y - 3, mx + 1.5, y + 7, 1.5, color)

    def _battery_icon(self, d, x, y, pct, plugged, scale):
        # x, y = left edge, vertical centre; size 18 x 9
        self._rrect(d, x, y - 4.5, x + 17, y + 4.5, 2, DIM)
        self._rrect(d, x + 1.2, y - 3.3, x + 15.8, y + 3.3, 1.2, CARD)
        self._rrect(d, x + 17.5, y - 2, x + 19, y + 2, 0.7, DIM)
        fill_w = 13.2 * max(0, min(100, pct)) / 100
        if fill_w > 0.5:
            color = scale(1 - pct / 100) if not plugged else scale(0)
            self._rrect(d, x + 2.2, y - 2.3, x + 2.2 + fill_w, y + 2.3, 0.8, color)

    def _pin_icon(self, d, cx, cy, color):
        # upright push pin, ~10 x 14
        self._rrect(d, cx - 4, cy - 7, cx + 4, cy - 4.6, 1.2, color)
        d.polygon(self._ss(cx - 2.4, cy - 4.8, cx + 2.4, cy - 4.8, cx + 3.4, cy + 0.6,
                           cx - 3.4, cy + 0.6), fill=color)
        self._rrect(d, cx - 5, cy + 0.4, cx + 5, cy + 2.2, 0.8, color)
        d.rectangle(self._ss(cx - 0.6, cy + 2, cx + 0.6, cy + 7.5), fill=color)

    def _close_icon(self, d, cx, cy, color):
        a = 4.2
        w = 1.5 * self.s * SS
        d.line(self._ss(cx - a, cy - a, cx + a, cy + a), fill=color, width=round(w))
        d.line(self._ss(cx - a, cy + a, cx + a, cy - a), fill=color, width=round(w))

    # --- text helpers on the 1x image ---
    def _t(self, d, x, y, text, font, fill, anchor="ls"):
        d.text((self.px(x), self.px(y)), text, font=font, fill=fill, anchor=anchor)

    def _rich(self, d, x, y, parts, align):
        total = sum(font.getlength(text) for text, font, _ in parts)
        x = self.px(x) - (total if align == "right" else total / 2 if align == "center" else 0)
        for text, font, color in parts:
            d.text((x, self.px(y)), text, font=font, fill=color, anchor="ls")
            x += font.getlength(text)

    def _rich_right(self, d, x_right, y, parts):
        """Draw [(text, font, colour), ...] right-aligned on baseline y."""
        x = self.px(x_right)
        for text, font, color in reversed(parts):
            x -= font.getlength(text)
            d.text((x, self.px(y)), text, font=font, fill=color, anchor="ls")

    # --- layout ---
    # The body under the CPU block is a list of sections (height, draw function):
    # the same list gives the window height and draws it, so they always agree.

    def _sections(self, data, hidden):
        secs = []
        if data.get("procs") is not None:
            secs.append((120, self._draw_procs))
        if "ram" not in hidden:
            secs.append((38, self._draw_ram))
        if "gpu" not in hidden:
            secs.append((38, self._draw_gpu))
        if "disks" not in hidden:
            for disk in data["disks"]:
                secs.append((38, lambda d, texts, y, data, scale, disk=disk:
                             self._draw_disk(d, texts, y, disk, scale)))
        if "temps" not in hidden:
            secs.append((40 + (20 if self._temps_online(data) else 16), self._draw_temps))
        lower = []
        if "net" not in hidden:
            lower.append((22, self._draw_net))
            if "net_graph" not in hidden:
                lower.append((26, self._draw_net_graph))
        if "ping" not in hidden:
            lower.append((22, self._draw_ping))
        if "disk_io" not in hidden:
            lower.append((22, self._draw_disk_io))
        if lower and secs:
            secs.append((16, self._draw_divider))
        secs += lower
        secs.append((30, self._draw_footer) if "uptime" not in hidden else (10, None))
        return secs

    def layout_height(self, data, compact=False, hidden=()):
        if compact:
            return self.px(178)
        return self.px(HERO_H + sum(h for h, _ in self._sections(data, hidden)))

    def _compact_rows(self, texts, data, scale, y):
        """Two lines of short readings under the CPU block (compact mode)."""
        P, Wl = self.PAD, self.WIDTH

        def chip(name, value, color, approx=False):
            head = [(f"{name} ", self.f_small, DIM)]
            if value is None:
                return head + [("—", self.f_value, FAINT)]
            return head + ([("≈", self.f_sym, color)] if approx else []) + [(value, self.f_value, color)]

        load = data.get("gpu_load")
        disk_pct = data["disks"][0][3] if data["disks"] else None
        row1 = [chip("Память", f"{data['ram_pct']:.0f}%", scale(data["ram_pct"] / 100)),
                chip("Видео", None if load is None else f"{load:.0f}%",
                     scale((load or 0) / 100)),
                chip("Диск", None if disk_pct is None else f"{disk_pct:.0f}%",
                     scale((disk_pct or 0) / 100))]
        cpu_t, gpu_t = data["temp_cpu"], data["temp_gpu"]
        shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
        g = cpu_t if shared else gpu_t
        ping, ready = data.get("ping"), data.get("ping_ready")
        row2 = [chip("CPU", None if cpu_t is None else f"{cpu_t:.0f}°", scale(((cpu_t or 30) - 30) / 70)),
                chip("GPU", None if g is None else f"{g:.0f}°", scale(((g or 30) - 30) / 70), shared),
                chip("Пинг", None if not ready else ("нет" if ping is None else f"{ping} мс"),
                     ping_color(scale, ping))]
        for row, by in ((row1, y + 14), (row2, y + 34)):
            texts.append(("rich_left", P, by, row[0]))
            texts.append(("rich_center", Wl / 2, by, row[1]))
            texts.append(("rich", Wl - P, by, row[2]))

    @staticmethod
    def _temps_online(data):
        return data["lhm_online"] and any(
            data.get(k) is not None for k in ("temp_cpu", "temp_gpu", "temp_ssd"))

    def render(self, data, theme, close_hover=False, pinned=False, pin_hover=False,
               compact=False, close_armed=False, hidden=()):
        scale = Scale(THEMES[theme][1])
        P, Wl = self.PAD, self.WIDTH
        if compact:
            data = dict(data, procs=None)
        H = self.layout_height(data, compact, hidden)
        card = self._card(H)

        layer = Image.new("RGBA", (self.W * SS, H * SS), (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        texts = []  # deferred text: (x, y, text, font, fill, anchor)

        # header; after one click on the cross it asks for a second one
        if close_armed:
            texts.append((P, 28, "Нажми ещё раз — закрыть", self.f_value, CLOSE_HOVER, "ls"))
            self._rrect(d, Wl - P - 13, 14, Wl - P + 5, 32, 5, lerp(CARD, CLOSE_HOVER, 0.25))
        else:
            texts.append((P, 28, "Система", self.f_title, TEXT, "ls"))
        self._close_icon(d, Wl - P - 4, 23, CLOSE_HOVER if close_hover or close_armed else DIM)
        self.close_box = (self.px(Wl - P - 14), self.px(11), self.px(Wl - P + 6), self.px(35))
        pin_x = Wl - P - 28
        pin_color = scale(0.0) if pinned else (TEXT if pin_hover else FAINT)
        if pinned and pin_hover:
            pin_color = lerp(pin_color, TEXT, 0.35)
        self._pin_icon(d, pin_x, 23, pin_color)
        self.pin_box = (self.px(pin_x - 10), self.px(11), self.px(pin_x + 9), self.px(35))
        batt = data["battery"]
        if batt is not None and not close_armed:
            pct, plugged = batt
            bx = Wl - P - 50 - 19
            self._battery_icon(d, bx, 23.5, pct, plugged, scale)
            left = data.get("battery_left")
            texts.append(("rich", bx - 6, 28,
                          ([(f"{fmt_left(left)}   ", self.f_small, FAINT)] if left else [])
                          + [(f"{pct:.0f}%", self.f_small, DIM)]
                          + ([(" ⚡", self.f_sym, DIM)] if plugged else [])))

        # hero: CPU number + histogram (+ clock and power when the sensors give them)
        cpu = data["cpu"]
        hero_base = 92
        num = f"{cpu:.0f}"
        num_w = self.f_hero.getlength(num) / self.s
        texts.append((P - 1, hero_base, num, self.f_hero, scale(cpu / 100), "ls"))
        texts.append((P + num_w + 2, hero_base, "%", self.f_hero_unit, DIM, "ls"))
        procs = data.get("procs")
        texts.append((P, hero_base + 18, "Процессор", self.f_small, DIM, "ls"))
        hx0 = P + 82
        self._histogram(d, hx0, 50, Wl - P, 100, data["cpu_hist"], scale)
        extra = cpu_extra_text(data)
        if extra:
            texts.append(("rich", Wl - P, hero_base + 18, [(extra, self.f_small, DIM)]))
        if compact:
            self.hero_box = (0, 0, 0, 0)  # no process list in compact mode
            self._compact_rows(texts, data, scale, HERO_H)
            return self._compose(card, layer, texts)
        chev_x = P + self.f_small.getlength("Процессор ") / self.s
        texts.append((chev_x, hero_base + 18, "▴" if procs is not None else "▾", self.f_sym, DIM, "ls"))
        self.hero_box = (self.px(P - 4), self.px(42), self.px(Wl - P + 4), self.px(118))

        y = HERO_H
        for h, draw in self._sections(data, hidden):
            if draw is not None:
                draw(d, texts, y, data, scale)
            y += h
        return self._compose(card, layer, texts)

    # --- sections: draw(d, texts, y, data, scale) ---
    def _draw_procs(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        procs = data["procs"]
        y0, y1 = y - 4, y + 108
        self._rrect(d, P - 6, y0, Wl - P + 6, y1, 8, (34, 40, 52))
        col_mem, col_cpu = Wl - P, Wl - P - 58
        texts.append((P, y0 + 17, "Программа", self.f_small, FAINT, "ls"))
        texts.append(("rich", col_cpu, y0 + 17, [("CPU", self.f_small, FAINT)]))
        texts.append(("rich", col_mem, y0 + 17, [("Память", self.f_small, FAINT)]))
        name_w = self.px(col_cpu - 42 - P)
        for i, (name, cpu_p, mem) in enumerate(procs):
            by = y0 + 36 + i * 17
            texts.append((P, by, ellipsize(name, self.f_small, name_w), self.f_small, TEXT, "ls"))
            cpu_txt = f"{cpu_p:.1f}".replace(".", ",") if cpu_p < 10 else f"{cpu_p:.0f}"
            texts.append(("rich", col_cpu, by, [(f"{cpu_txt}%", self.f_value,
                                                 scale(min(1.0, cpu_p / 50)))]))
            texts.append(("rich", col_mem, by, [(fmt_mem(mem), self.f_small, DIM)]))
        if not procs:
            texts.append((P, y0 + 36, "собираю данные…", self.f_small, FAINT, "ls"))

    def _draw_ram(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        texts.append((P, y + 14, "Память", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [
            (f"{fmt_gb(data['ram_used'])} из {fmt_gb(data['ram_total'])} ГБ   ", self.f_small, DIM),
            (f"{data['ram_pct']:.0f}%", self.f_value, TEXT)]))
        self._segment_meter(d, P, y + 22, Wl - P, data["ram_pct"], scale)

    def _draw_gpu(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        load, gname = data.get("gpu_load"), data.get("gpu_name")
        texts.append((P, y + 14, "Видеокарта", self.f_label, TEXT, "ls"))
        if load is not None:
            value = f"{load:.0f}%"
            room = (self.px(Wl - P) - self.px(P) - self.f_label.getlength("Видеокарта")
                    - self.f_value.getlength(value) - self.px(24))
            parts = ([(ellipsize(gname, self.f_small, room) + "   ", self.f_small, DIM)] if gname else [])
            texts.append(("rich", Wl - P, y + 14, parts + [(value, self.f_value, TEXT)]))
        else:
            texts.append(("rich", Wl - P, y + 14, [("нет данных", self.f_small, FAINT)]))
        self._segment_meter(d, P, y + 22, Wl - P, load or 0, scale)

    def _draw_disk(self, d, texts, y, disk, scale):
        P, Wl = self.PAD, self.WIDTH
        name, used, total, pct = disk
        texts.append((P, y + 14, f"Диск {name}", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [
            (f"{fmt_gb(used)} из {fmt_gb(total)} ГБ   ", self.f_small, DIM),
            (f"{pct:.0f}%", self.f_value, TEXT)]))
        self._segment_meter(d, P, y + 22, Wl - P, pct, scale)

    def _draw_temps(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        cpu_t, gpu_t, ssd_t = data["temp_cpu"], data["temp_gpu"], data.get("temp_ssd")
        shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
        online = self._temps_online(data)
        texts.append((P, y + 14, "Температура", self.f_label, TEXT, "ls"))
        if not online:
            texts.append(("rich", Wl - P, y + 14, [("нет данных", self.f_small, FAINT)]))
        self._temp_strip(d, P, y + 26, Wl - P, cpu_t, gpu_t, scale, online)
        y += 40
        if online:
            def chip(name, t, approx=False):
                head = [(f"{name} ", self.f_small, DIM)]
                if t is None:
                    return head + [("—", self.f_value, FAINT)]
                color = scale((t - 30) / 70)
                return (head + ([("≈", self.f_sym, color)] if approx else [])
                        + [(f"{t:.0f}°", self.f_value, color)])
            texts.append(("rich_left", P, y + 8, chip("CPU", cpu_t)))
            texts.append(("rich_center", Wl / 2, y + 8,
                          chip("GPU", cpu_t if shared else gpu_t, approx=shared)))
            texts.append(("rich", Wl - P, y + 8, chip("SSD", ssd_t)))
        else:
            hint = ellipsize(data.get("sensor_hint") or "Нет данных о температуре", self.f_small,
                             self.px(Wl - 2 * P))
            texts.append((P, y + 8, hint, self.f_small, FAINT, "ls"))

    def _draw_divider(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        d.rectangle(self._ss(P, y + 6, Wl - P, y + 6 + 1 / self.s), fill=TRACK)

    def _draw_net(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        down_c, up_c = scale(0.0), scale(0.75)
        texts.append((P, y + 12, "Сеть", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 12, [
            ("↓ ", self.f_sym, down_c), (fmt_speed(data["net_down"]), self.f_small, TEXT),
            ("     ↑ ", self.f_sym, up_c), (fmt_speed(data["net_up"]), self.f_small, TEXT)]))

    def _draw_net_graph(self, d, texts, y, data, scale):
        """Last minute of traffic: download bars up from the line, upload bars down.
        Square-root scale, so a small upload is still visible next to a big download."""
        P, Wl = self.PAD, self.WIDTH
        down_c, up_c = scale(0.0), scale(0.75)
        hist = data.get("net_hist") or []
        top, mid, bottom = y + 1, y + 13, y + 21
        peak = max([max(a, b) for a, b in hist] + [NET_GRAPH_FLOOR])
        pitch = (Wl - 2 * P) / HISTORY_LEN
        bar_w = pitch * 0.62
        offset = HISTORY_LEN - len(hist)
        d.rectangle(self._ss(P, mid - 0.4, Wl - P, mid + 0.4), fill=HIST_TRACK)
        for i, (down, up) in enumerate(hist):
            bx = P + (i + offset) * pitch
            hd = (mid - top) * math.sqrt(max(0.0, down) / peak)
            hu = (bottom - mid) * math.sqrt(max(0.0, up) / peak)
            if hd >= 0.5:
                d.rectangle(self._ss(bx, mid - hd, bx + bar_w, mid - 0.4), fill=down_c)
            if hu >= 0.5:
                d.rectangle(self._ss(bx, mid + 0.4, bx + bar_w, mid + hu), fill=up_c)

    def _draw_ping(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        texts.append((P, y + 12, "Пинг", self.f_label, TEXT, "ls"))
        ping = data.get("ping")
        if not data.get("ping_ready"):
            parts = [("…", self.f_small, FAINT)]
        elif ping is None:
            parts = [("нет ответа", self.f_value, ping_color(scale, None))]
        else:
            parts = [(f"до {PING_HOST}   ", self.f_small, FAINT),
                     (f"{ping} мс", self.f_value, ping_color(scale, ping))]
        texts.append(("rich", Wl - P, y + 12, parts))

    def _draw_disk_io(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        texts.append((P, y + 12, "Диск", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 12, [
            ("чтение ", self.f_small, DIM), (fmt_speed(data["io_read"]), self.f_small, TEXT),
            ("   запись ", self.f_small, DIM), (fmt_speed(data["io_write"]), self.f_small, TEXT)]))

    def _draw_footer(self, d, texts, y, data, scale):
        texts.append((self.PAD, y + 18, f"Без перезагрузки {fmt_uptime(data['uptime'])}",
                      self.f_small, FAINT, "ls"))

    def _compose(self, card, layer, texts):
        img = card.convert("RGBA")
        img.alpha_composite(layer.reduce(SS))
        td = ImageDraw.Draw(img)
        for item in texts:
            if item[0] == "rich":
                self._rich_right(td, item[1], item[2], item[3])
            elif item[0] in ("rich_left", "rich_center"):
                self._rich(td, item[1], item[2], item[3], item[0][5:])
            else:
                x, ty, text, font, fill, anchor = item
                self._t(td, x, ty, text, font, fill, anchor)
        return img.convert("RGB")


# ---------- taskbar mini panel ----------

class TaskbarPanel:
    """Two-line readout on the Windows taskbar, just left of the tray icons."""

    # hidden = parked off-screen; Win32 ShowWindow would desync Tk's geometry
    OFFSCREEN = "1x1+-9000+-9000"  # not -32000: Windows treats that as "minimized"

    def __init__(self, widget):
        self.widget = widget
        px = self.px = widget.renderer.px
        self.gap = px(6)
        self.font = load_font(px(12))
        self.font_val = load_font(px(12), "SemiBold")
        self.font_sym = load_symbol_font(px(12))
        self.light = taskbar_uses_light_theme()
        # background ~ taskbar colour: text edges blend into it, then it is keyed out
        self.key = (232, 232, 234) if self.light else (22, 22, 24)
        key_hex = "#%02x%02x%02x" % self.key

        f, fv, fs = self.font, self.font_val, self.font_sym
        speed_w = max(f.getlength("1023 КБ/с"), f.getlength("99,9 МБ/с"))
        self.col_a = max(f.getlength("CPU "), f.getlength("RAM ")) + fv.getlength("100%")
        self.col_b = fs.getlength("↓ ") + speed_w
        self.col_c = (max(f.getlength("CPU "), f.getlength("GPU ")) + fs.getlength("≈")
                      + fv.getlength("100°"))
        self.col_d = max(f.getlength("пинг"), fv.getlength("999 мс"))
        self.col_gap = px(14)

        self.top = top = tk.Toplevel(widget.root)
        top.overrideredirect(True)
        top.configure(bg=key_hex)
        top.attributes("-transparentcolor", key_hex)
        top.attributes("-topmost", True)
        self.view = tk.Label(top, bg=key_hex, bd=0, highlightthickness=0)
        self.view.pack()
        self.view.bind("<Button-3>", widget._show_menu)
        self.view.bind("<Enter>", self._enter)
        self.view.bind("<Leave>", self._leave)
        self.view.bind("<ButtonRelease-1>", lambda e: widget.toggle_widget())
        self.view.configure(cursor="hand2")
        self.hover = False
        self.hover_job = None
        top.geometry(self.OFFSCREEN)
        top.update_idletasks()
        self.hwnd = int(top.wm_frame(), 16)
        self.shown = False
        self.owner = None
        self.photo = None
        self.size = None
        self.bar_h = None
        self.pos = None
        self.tasks = self.rebar = None

    # --- drawing ---
    def draw(self, data, theme):
        if not self.bar_h:
            return
        img = self.render(data, theme, self.bar_h)
        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        if img.size != self.size:
            self.size = img.size
            self.layout()
        info = getattr(self.widget, "info", None)
        if self.hover and info is not None and info.visible:
            self._show_info()  # keep the details live while hovering

    def render(self, data, theme, h):
        scale = Scale(THEMES[theme][1])
        temps = data["temp_cpu"] is not None or data["temp_gpu"] is not None
        x_b = self.col_a + self.col_gap
        x_d = x_b + self.col_b + self.col_gap
        x_c = x_d + self.col_d + self.col_gap
        width = int((x_c + self.col_c) if temps else (x_d + self.col_d)) + 2
        img = Image.new("RGB", (width, h), self.key)
        d = ImageDraw.Draw(img)
        label_c = (88, 88, 92) if self.light else (158, 163, 172)
        text_c = (24, 24, 24) if self.light else (240, 240, 242)

        def tone(t):
            c = scale(t)
            return lerp(c, (0, 0, 0), 0.4) if self.light else c

        off = min(self.px(8), h * 0.24)
        rows = (h / 2 - off, h / 2 + off)
        f, fv, fs = self.font, self.font_val, self.font_sym

        for y, name, pct in ((rows[0], "CPU", data["cpu"]), (rows[1], "RAM", data["ram_pct"])):
            d.text((0, y), name, font=f, fill=label_c, anchor="lm")
            d.text((self.col_a, y), f"{pct:.0f}%", font=fv, fill=tone(pct / 100), anchor="rm")

        for y, arrow, color, speed in ((rows[0], "↓", tone(0.0), data["net_down"]),
                                       (rows[1], "↑", tone(0.75), data["net_up"])):
            d.text((x_b, y), arrow, font=fs, fill=color, anchor="lm")
            d.text((x_b + self.col_b, y), fmt_speed(speed), font=f, fill=text_c, anchor="rm")

        ping = data.get("ping")
        d.text((x_d + self.col_d, rows[0]), "пинг", font=f, fill=label_c, anchor="rm")
        if not data.get("ping_ready"):
            value, color = "…", label_c
        elif ping is None:
            value, color = "нет", tone(1.0)
        else:
            value, color = f"{ping} мс", tone(0.05 if ping < 50 else 0.55 if ping < 100 else 0.95)
        d.text((x_d + self.col_d, rows[1]), value, font=fv, fill=color, anchor="rm")

        if temps:
            cpu_t, gpu_t = data["temp_cpu"], data["temp_gpu"]
            shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
            for y, name, t, approx in ((rows[0], "CPU", cpu_t, False),
                                       (rows[1], "GPU", cpu_t if shared else gpu_t, shared)):
                d.text((x_c, y), name, font=f, fill=label_c, anchor="lm")
                value, color = ("—", label_c) if t is None else (f"{t:.0f}°", tone((t - 30) / 70))
                right = x_c + self.col_c
                d.text((right, y), value, font=fv, fill=color, anchor="rm")
                if approx:
                    d.text((right - fv.getlength(value), y), "≈", font=fs, fill=color, anchor="rm")

        return img

    # --- hover details ---
    def _enter(self, event):
        self.hover = True
        if self.hover_job is None:
            self.hover_job = self.top.after(350, self._show_info)

    def _leave(self, event):
        self.hover = False
        if self.hover_job is not None:
            self.top.after_cancel(self.hover_job)
            self.hover_job = None
        self.widget.info.hide()

    def _show_info(self):
        self.hover_job = None
        if self.hover and self.shown and self.widget.data:
            self.widget.info.show_above(window_rect(self.hwnd), self.widget.data,
                                        self.widget.cfg["theme"])

    # --- placement ---
    def _hide(self):
        if self.shown:
            self.top.geometry(self.OFFSCREEN)
            self.shown = False
            self.pos = None

    def layout(self):
        if not self.widget.cfg["taskbar"]:
            self._hide()
            return
        bar, notify, tasks, rebar = find_taskbar()
        if not bar or not notify:
            self._hide()
            return
        nl, _, nr, _ = window_rect(notify)
        if nr - nl <= 0:  # tray area without a size (newer Windows 11 taskbar): nowhere to dock
            if not getattr(self, "_warned_tray", False):
                self._warned_tray = True
                log.warning("taskbar tray area has no size; mini panel hidden")
            self._hide()
            return
        bl, bt, br, bb = window_rect(bar)
        h = bb - bt
        screen_h = ctypes.windll.user32.GetSystemMetrics(1)
        visible = min(bb, screen_h) - max(bt, 0)
        if br - bl <= h or visible < h * 0.8 or fullscreen_app_active():
            self._hide()  # vertical, auto-hidden, or a fullscreen game/video
            return
        if h != self.bar_h:
            self.bar_h = h
            if self.widget.data:
                self.draw(self.widget.data, self.widget.cfg["theme"])
        if not self.size:
            return
        if self.owner != bar:
            set_owner(self.hwnd, bar)
            self.owner = bar
        w = self.size[0]
        x = window_rect(notify)[0] - self.gap - w
        if tasks and rebar:
            limit = x - self.gap
            tl, _, tr, _ = window_rect(tasks)
            if tl < limit and abs(tr - limit) > 1:
                set_child_right(tasks, rebar, limit)
            self.tasks, self.rebar = tasks, rebar
        pos = (w, h, x, bt)
        if pos != self.pos:
            self.top.geometry("%dx%d+%d+%d" % pos)
            self.pos = pos
        self.shown = True

    def restore(self):
        """Give the task buttons their full width back."""
        if self.tasks and self.rebar and _user32.IsWindow(self.tasks):
            set_child_right(self.tasks, self.rebar, window_rect(self.rebar)[2])
        self.tasks = self.rebar = None


# ---------- notifications ----------

def work_area():
    """Screen area not covered by the taskbar: (left, top, right, bottom)."""
    r = _wt.RECT()
    ctypes.windll.user32.SystemParametersInfoW(0x30, 0, ctypes.byref(r), 0)  # SPI_GETWORKAREA
    return r.left, r.top, r.right, r.bottom


def card_image(px, W, H, accent=None):
    """Rounded dark card with keyed-out (transparent) corners, supersampled."""
    S = SS
    big = Image.new("RGB", (W * S, H * S), KEY)
    d = ImageDraw.Draw(big)
    r = px(12) * S
    d.rounded_rectangle((0, 0, W * S - 1, H * S - 1), radius=r, fill=BORDER)
    bw = max(1, px(1)) * S
    d.rounded_rectangle((bw, bw, W * S - 1 - bw, H * S - 1 - bw), radius=r - bw, fill=CARD)
    if accent is not None:
        d.rounded_rectangle((px(14) * S, px(16) * S, px(18) * S, H * S - px(16) * S),
                            radius=px(2) * S, fill=accent)
    return big.reduce(S)


class PopupCard:
    """Borderless topmost card. Hidden = parked off-screen, so showing it
    never activates the window or steals focus from the game/app in front."""

    OFFSCREEN = "1x1+-9000+-9000"  # not -32000: Windows treats that as "minimized"

    def __init__(self, widget):
        self.widget = widget
        self.px = widget.renderer.px
        self.top = top = tk.Toplevel(widget.root)
        top.overrideredirect(True)
        top.configure(bg=KEY_HEX)
        top.attributes("-transparentcolor", KEY_HEX)
        top.attributes("-topmost", True)
        self.view = tk.Label(top, bg=KEY_HEX, bd=0, highlightthickness=0, cursor="hand2")
        self.view.pack()
        top.geometry(self.OFFSCREEN)
        top.update_idletasks()  # map it now, parked off-screen
        self.photo = None
        self.visible = False

    def _show_image(self, img, x, y):
        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        geom = f"{img.width}x{img.height}+{x}+{y}"
        self.top.geometry(geom)
        self.top.lift()
        self.visible = True
        # Tk sometimes drops the position of a move made right after the window
        # was mapped (the size still applies) — check once and re-apply
        self.top.after(80, lambda: self._ensure(geom))

    def _ensure(self, geom):
        if self.visible and self.top.wm_geometry() != geom:
            self.top.geometry(geom)

    def hide(self):
        self.top.geometry(self.OFFSCREEN)
        self.visible = False


class Toast(PopupCard):
    """Alert card in the bottom-right corner. Click to dismiss."""

    WIDTH = 300
    SHOW_MS = 9000

    def __init__(self, widget):
        super().__init__(widget)
        self.f_title = load_font(self.px(14), "SemiBold")
        self.f_body = load_font(self.px(12))
        self.view.bind("<ButtonRelease-1>", lambda e: self.hide())
        self.job = None

    def _wrap(self, text, max_w):
        lines, line = [], ""
        for word in text.split():
            trial = f"{line} {word}".strip()
            if line and self.f_body.getlength(trial) > max_w:
                lines.append(line)
                line = word
            else:
                line = trial
        return lines + ([line] if line else [])

    def render(self, title, body, color):
        px = self.px
        W = px(self.WIDTH)
        lines = self._wrap(body, W - px(46))
        img = card_image(px, W, px(46 + 17 * len(lines)), accent=color)
        td = ImageDraw.Draw(img)
        td.text((px(30), px(29)), title, font=self.f_title, fill=TEXT, anchor="ls")
        for i, line in enumerate(lines):
            td.text((px(30), px(48 + 17 * i)), line, font=self.f_body, fill=DIM, anchor="ls")
        return img

    def show(self, title, body, color):
        img = self.render(title, body, color)
        _, _, right, bottom = work_area()
        self._show_image(img, right - img.width - self.px(14), bottom - img.height - self.px(14))
        if self.job:
            self.top.after_cancel(self.job)
        self.job = self.top.after(self.SHOW_MS, self.hide)

    def hide(self):
        self.job = None
        super().hide()


class InfoCard(PopupCard):
    """Details that pop up above the taskbar panel while the mouse is over it."""

    WIDTH = 252

    def __init__(self, widget):
        super().__init__(widget)
        px = self.px
        self.f_title = load_font(px(13), "SemiBold")
        self.f = load_font(px(12))
        self.fv = load_font(px(12), "SemiBold")
        self.fs = load_symbol_font(px(12))
        self.f_small = load_font(px(11))
        self.view.bind("<ButtonRelease-1>", lambda e: widget.show_widget())

    def render(self, data, theme):
        px, f, fv, fs = self.px, self.f, self.fv, self.fs
        scale = Scale(THEMES[theme][1])

        def temp(t, approx=False):
            if t is None:
                return []
            c = scale((t - 30) / 70)
            return [("   ", f, DIM)] + ([("≈", fs, c)] if approx else []) + [(f"{t:.0f}°", fv, c)]

        cpu_t, gpu_t = data["temp_cpu"], data["temp_gpu"]
        shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
        rows = [("Процессор", [(f"{data['cpu']:.0f}%", fv, scale(data["cpu"] / 100))] + temp(cpu_t)),
                ("Память", [(f"{fmt_gb(data['ram_used'])} из {fmt_gb(data['ram_total'])} ГБ   ", f, DIM),
                            (f"{data['ram_pct']:.0f}%", fv, scale(data["ram_pct"] / 100))])]
        if data.get("gpu_load") is not None:
            rows.append(("Видеокарта",
                         [(f"{data['gpu_load']:.0f}%", fv, scale(data["gpu_load"] / 100))]
                         + temp(cpu_t if shared else gpu_t, approx=shared)))
        for name, _, _, pct in data["disks"][:1]:
            rows.append((f"Диск {name}", [(f"{pct:.0f}%", fv, TEXT)] + temp(data.get("temp_ssd"))))
        rows.append(("Сеть", [("↓ ", fs, scale(0.0)), (fmt_speed(data["net_down"]), f, TEXT),
                              ("   ↑ ", fs, scale(0.75)), (fmt_speed(data["net_up"]), f, TEXT)]))
        ping, ready = data.get("ping"), data.get("ping_ready")
        ping_text = "…" if not ready else "нет ответа" if ping is None else f"{ping} мс"
        rows.append(("Пинг", [(ping_text, fv, ping_color(scale, ping) if ready else FAINT)]))
        batt = data.get("battery")
        if batt is not None:
            level, plugged = batt
            left = data.get("battery_left")
            rows.append(("Батарея", ([(f"{fmt_left(left)}   ", f, DIM)] if left else [])
                         + [(f"{level:.0f}%", fv, scale(1 - level / 100) if not plugged else scale(0))]
                         + ([(" ⚡", fs, DIM)] if plugged else [])))
        top = top_processes(1)
        if top:
            name, cpu_p, _ = top[0]
            rows.append(("Больше всех", [(ellipsize(name, f, px(110)) + "  ", f, TEXT),
                                         (f"{cpu_p:.0f}%", fv, scale(min(1.0, cpu_p / 50)))]))

        W = px(self.WIDTH)
        H = px(40 + 20 * len(rows) + 22)
        img = card_image(px, W, H)
        d = ImageDraw.Draw(img)
        d.text((px(16), px(26)), "Система", font=self.f_title, fill=TEXT, anchor="ls")
        d.text((W - px(16), px(26)), f"без перезагрузки {fmt_uptime(data['uptime'])}",
               font=self.f_small, fill=FAINT, anchor="rs")
        for i, (label, parts) in enumerate(rows):
            y = px(50 + 20 * i)
            d.text((px(16), y), label, font=f, fill=DIM, anchor="ls")
            x = W - px(16) - sum(font.getlength(t) for t, font, _ in parts)
            for t, font, color in parts:
                d.text((x, y), t, font=font, fill=color, anchor="ls")
                x += font.getlength(t)
        d.text((px(16), H - px(14)), "Нажми, чтобы открыть виджет", font=self.f_small,
               fill=FAINT, anchor="ls")
        return img

    def show_above(self, rect, data, theme):
        img = self.render(data, theme)
        left, top, right, _ = rect
        screen_w = ctypes.windll.user32.GetSystemMetrics(0)
        x = max(0, min(right - img.width, screen_w - img.width - self.px(8)))
        self._show_image(img, x, top - img.height - self.px(8))


# ---------- window ----------

class Widget:
    def __init__(self):
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(1)
        except (AttributeError, OSError):
            pass

        self.cfg = load_config()
        self.root = root = tk.Tk()
        root.report_callback_exception = lambda *exc: log.error("Tk callback failed", exc_info=exc)
        root.title("PC Monitor")
        root.overrideredirect(True)
        root.configure(bg=KEY_HEX)
        root.attributes("-transparentcolor", KEY_HEX)
        root.attributes("-topmost", bool(self.cfg["topmost"]))
        root.attributes("-alpha", float(self.cfg["alpha"]))

        icon = os.path.join(getattr(sys, "_MEIPASS", APP_DIR), "assets", "app.png")
        if os.path.exists(icon):
            self._icon = tk.PhotoImage(file=icon)
            root.iconphoto(True, self._icon)
        self.renderer = Renderer(root.winfo_fpixels("1i") / 96)
        self.view = tk.Label(root, bg=KEY_HEX, bd=0, highlightthickness=0)
        self.view.pack()
        self.photo = None
        self.height = None
        self.close_hover = False
        self.close_pressed = False
        self.close_armed = False  # first click on the cross only asks for a second one
        self.close_job = None
        self.pin_hover = False
        self.pin_pressed = False
        self._drag = (0, 0)
        self.hwnd = None

        self._build_menu()
        self.view.bind("<ButtonPress-1>", self._press)
        self.view.bind("<B1-Motion>", self._drag_move)
        self.view.bind("<ButtonRelease-1>", self._release)
        self.view.bind("<Motion>", self._hover)
        self.view.bind("<Leave>", self._leave)
        self.view.bind("<Button-3>", self._show_menu)

        # prime counters so the first readings are meaningful
        psutil.cpu_percent(interval=None)
        self.last_net = psutil.net_io_counters()
        self.last_disk = psutil.disk_io_counters()
        self.last_t = time.monotonic()
        self.cpu_hist = []
        self.net_hist = []  # (down, up) bytes/s for the network graph
        self.partitions = scan_partitions()
        self.partitions_t = time.monotonic()
        self.data = None

        self.lhm = LhmReader()
        self.lhm.start()
        self.ping = PingReader()
        self.ping.start()
        self.show_procs = False
        self.alert_state = {}
        self._press_at = (0, 0)
        self.hero_pressed = False

        self.update()
        self._place()
        root.update_idletasks()
        self.hwnd = int(root.wm_frame(), 16)
        self._apply_pin()

        self.taskbar = TaskbarPanel(self) if _user32 is not None else None
        self._taskbar_tick()
        self.toast = Toast(self) if _user32 is not None else None
        self.info = InfoCard(self) if _user32 is not None else None
        self.visible = True

        self.hotkey = HotkeyListener()
        self.hotkey.start()
        self._poll_hotkey()
        try:
            check_autostart(prefer_task=bool(self.cfg["run_as_admin"]))
        except OSError:
            log.exception("checking autostart failed")

    # --- show / hide ---
    def hide_widget(self):
        self.root.withdraw()
        self.visible = False

    def show_widget(self):
        """Bring the widget up, in front of other windows, until something else is clicked."""
        if not self.visible:
            self.root.deiconify()
            self.visible = True
            self.root.update_idletasks()
            if self.cfg["pinned"]:
                set_owner(self.hwnd, find_desktop_view())
        self.root.lift()
        try:
            self.root.focus_force()
        except tk.TclError:
            pass

    def toggle_widget(self):
        in_front = (self.visible and _user32 is not None
                    and _user32.GetForegroundWindow() == self.hwnd)
        if in_front:
            self.hide_widget()
        else:
            self.show_widget()

    def _poll_hotkey(self):
        if self.hotkey.pressed.is_set():
            self.hotkey.pressed.clear()
            self.toggle_widget()
        self.root.after(120, self._poll_hotkey)

    def _toggle_autostart(self):
        """Elevated + wants admin: a scheduled task (no UAC at logon); otherwise the Run key."""
        enable = self.autostart_var.get()
        use_task = enable and is_admin() and self.cfg["run_as_admin"] and task_allowed()
        try:
            if use_task and set_admin_task(True):
                set_autostart(False)
            else:
                set_autostart(enable)
                if is_admin():
                    set_admin_task(False)
        except OSError:
            log.exception("changing autostart failed")
        self.autostart_var.set(autostart_enabled() or admin_task_enabled())

    def _toggle_admin(self):
        self.cfg["run_as_admin"] = self.admin_var.get()
        save_config(self.cfg)
        if self.cfg["run_as_admin"] and (not is_admin() or (is_frozen() and not running_installed())):
            self.quit_for_elevation()
        elif self.autostart_var.get():
            self._toggle_autostart()  # switch autostart between task and Run key

    def quit_for_elevation(self):
        """Restart elevated (from the protected copy); if UAC is refused, keep running as we are."""
        release_instance()
        if is_admin():  # already elevated, but from a folder anyone can write to
            if install_copy() and start_installed():
                self.quit()
            else:
                already_running()
        elif relaunch_as_admin():
            self.quit()
        else:
            already_running()  # take the single-instance lock back
            self.admin_var.set(False)
            self.cfg["run_as_admin"] = False
            save_config(self.cfg)

    def _toggle_compact(self):
        self.cfg["compact"] = self.compact_var.get()
        save_config(self.cfg)
        self._draw()

    # --- taskbar panel ---
    def _taskbar_tick(self):
        if self.taskbar is not None:
            try:
                self.taskbar.layout()
            except Exception:  # explorer restarting etc. — try again next tick
                log_error("taskbar panel")
        self.root.after(500, self._taskbar_tick)

    def _toggle_taskbar(self):
        self.cfg["taskbar"] = self.taskbar_var.get()
        save_config(self.cfg)
        if self.taskbar is not None:
            if not self.cfg["taskbar"]:
                self.taskbar.restore()
            self.taskbar.layout()

    # --- pinning ---
    def _apply_pin(self):
        pinned = bool(self.cfg["pinned"])
        if pinned:
            self.root.attributes("-topmost", False)
            set_owner(self.hwnd, find_desktop_view())
            send_to_back(self.hwnd)
        else:
            set_owner(self.hwnd, None)
            self.root.attributes("-topmost", bool(self.cfg["topmost"]))
        self.menu.entryconfigure(self.topmost_index,
                                 state="disabled" if pinned else "normal")

    def _toggle_pin(self):
        self.cfg["pinned"] = self.pinned_var.get()
        save_config(self.cfg)
        self._apply_pin()
        self._draw()

    def _back_if_pinned(self):
        if self.cfg["pinned"]:
            send_to_back(self.hwnd)

    # --- menu ---
    def _build_menu(self):
        self.pinned_var = tk.BooleanVar(value=bool(self.cfg["pinned"]))
        self.topmost_var = tk.BooleanVar(value=bool(self.cfg["topmost"]))
        self.alpha_var = tk.DoubleVar(value=float(self.cfg["alpha"]))
        self.theme_var = tk.StringVar(value=self.cfg["theme"])
        menu = tk.Menu(self.root, tearoff=0)
        self.taskbar_var = tk.BooleanVar(value=bool(self.cfg["taskbar"]))
        menu.add_checkbutton(label="Закрепить на рабочем столе", variable=self.pinned_var,
                             command=self._toggle_pin)
        menu.add_checkbutton(label="Мини-панель на панели задач", variable=self.taskbar_var,
                             command=self._toggle_taskbar)
        self.procs_var = tk.BooleanVar(value=False)
        menu.add_checkbutton(label="Список процессов", variable=self.procs_var,
                             command=lambda: self._set_procs(self.procs_var.get()))
        self.compact_var = tk.BooleanVar(value=bool(self.cfg["compact"]))
        menu.add_checkbutton(label="Компактный режим", variable=self.compact_var,
                             command=self._toggle_compact)
        self.autostart_var = tk.BooleanVar(value=autostart_enabled() or admin_task_enabled())
        menu.add_checkbutton(label="Запускать вместе с Windows", variable=self.autostart_var,
                             command=self._toggle_autostart)
        self.admin_var = tk.BooleanVar(value=bool(self.cfg["run_as_admin"]))
        if lhm_dir():
            menu.add_checkbutton(label="Права администратора (температуры)",
                                 variable=self.admin_var, command=self._toggle_admin)
        menu.add_separator()
        show_menu = tk.Menu(menu, tearoff=0)
        self.section_vars = {}
        for key, title in SECTIONS.items():
            var = self.section_vars[key] = tk.BooleanVar(value=key not in self.cfg["hidden"])
            show_menu.add_checkbutton(label=title, variable=var,
                                      command=lambda k=key: self._toggle_section(k))
        menu.add_cascade(label="Показывать", menu=show_menu)
        alerts_menu = tk.Menu(menu, tearoff=0)
        self.alerts_var = tk.BooleanVar(value=bool(self.cfg["alerts"]))
        alerts_menu.add_checkbutton(label="Показывать предупреждения", variable=self.alerts_var,
                                    command=self._toggle_alerts)
        alerts_menu.add_separator()
        self.limit_vars = {}
        for key, title, unit in (("alert_temp", "Перегрев CPU от", "°"),
                                 ("alert_ram", "Память занята от", "%"),
                                 ("alert_battery", "Батарея ниже", "%")):
            sub = tk.Menu(alerts_menu, tearoff=0)
            var = self.limit_vars[key] = tk.IntVar(value=self.cfg[key])
            for value in CONFIG_CHOICES[key]:
                sub.add_radiobutton(label=f"{value}{unit}", value=value, variable=var,
                                    command=lambda k=key: self._set_limit(k))
            alerts_menu.add_cascade(label=title, menu=sub)
        menu.add_cascade(label="Предупреждения", menu=alerts_menu)
        theme_menu = tk.Menu(menu, tearoff=0)
        for key, (title, _) in THEMES.items():
            theme_menu.add_radiobutton(label=title, value=key, variable=self.theme_var,
                                       command=self._set_theme)
        menu.add_cascade(label="Цвета", menu=theme_menu)
        alpha_menu = tk.Menu(menu, tearoff=0)
        for pct in (100, 97, 90, 80, 70):
            alpha_menu.add_radiobutton(label=f"{pct}%", value=pct / 100,
                                       variable=self.alpha_var, command=self._set_alpha)
        menu.add_cascade(label="Непрозрачность", menu=alpha_menu)
        menu.add_checkbutton(label="Поверх всех окон", variable=self.topmost_var,
                             command=self._toggle_topmost)
        self.topmost_index = menu.index("end")
        menu.add_command(label="В правый верхний угол", command=self._reset_position)
        menu.add_separator()
        menu.add_command(label=f"Скрыть  ({HOTKEY_TEXT} — показать)", command=self.hide_widget)
        menu.add_command(label="Закрыть", command=self.quit)
        self.menu = menu

    def _show_menu(self, event):
        try:
            self.menu.tk_popup(event.x_root, event.y_root)
        finally:
            self.menu.grab_release()
        self._back_if_pinned()

    def _set_theme(self):
        self.cfg["theme"] = self.theme_var.get()
        save_config(self.cfg)
        self._draw()
        if self.taskbar is not None and self.data:
            self.taskbar.draw(self.data, self.cfg["theme"])

    def _toggle_topmost(self):
        self.cfg["topmost"] = self.topmost_var.get()
        self.root.attributes("-topmost", self.cfg["topmost"])
        save_config(self.cfg)

    def _set_alpha(self):
        self.cfg["alpha"] = self.alpha_var.get()
        self.root.attributes("-alpha", self.cfg["alpha"])
        save_config(self.cfg)

    def quit(self):
        if getattr(self, "taskbar", None) is not None:
            try:
                self.taskbar.restore()
            except Exception:
                log.exception("restoring the taskbar failed")
            self.taskbar = None
        self.root.destroy()

    # --- placement, dragging, close button ---
    def _place(self):
        w, h = self.renderer.W, self.height
        left, top, vw, vh = virtual_screen(self.root)
        x, y = self.cfg.get("x"), self.cfg.get("y")
        inside = (isinstance(x, int) and isinstance(y, int)
                  and left <= x <= left + vw - 40 and top <= y <= top + vh - 40)
        if not inside:
            x = self.root.winfo_screenwidth() - w - self.renderer.px(20)
            y = self.renderer.px(40)
        self.root.geometry(f"{w}x{h}+{x}+{y}")

    def _reset_position(self):
        self.cfg["x"] = self.cfg["y"] = None
        self._place()
        self._save_position()

    def _arm_close(self, armed):
        """The cross closes on a second click within CLOSE_CONFIRM_MS, so a slip
        of the mouse next to the pin does not quit the program."""
        if self.close_job is not None:
            self.root.after_cancel(self.close_job)
            self.close_job = None
        self.close_armed = armed
        if armed:
            self.close_job = self.root.after(CLOSE_CONFIRM_MS, lambda: self._arm_close(False))
        self._draw()

    def _in_close(self, event):
        x0, y0, x1, y1 = self.renderer.close_box
        return x0 <= event.x <= x1 and y0 <= event.y <= y1

    def _in_box(self, box, event):
        x0, y0, x1, y1 = box
        return x0 <= event.x <= x1 and y0 <= event.y <= y1

    def _press(self, event):
        self.close_pressed = self._in_close(event)
        self.pin_pressed = self._in_box(self.renderer.pin_box, event)
        self.hero_pressed = self._in_box(self.renderer.hero_box, event)
        self._press_at = (event.x_root, event.y_root)
        self._drag = (event.x_root - self.root.winfo_x(), event.y_root - self.root.winfo_y())

    def _drag_move(self, event):
        if self.close_pressed or self.pin_pressed or self.cfg["pinned"]:
            return
        dx, dy = self._drag
        self.root.geometry(f"+{event.x_root - dx}+{event.y_root - dy}")

    def _release(self, event):
        if self.close_pressed:
            self.close_pressed = False
            if self._in_close(event):
                if self.close_armed:
                    self.quit()
                else:
                    self._arm_close(True)
            return
        if self.pin_pressed:
            self.pin_pressed = False
            if self._in_box(self.renderer.pin_box, event):
                self.pinned_var.set(not self.cfg["pinned"])
                self._toggle_pin()
            return
        moved = abs(event.x_root - self._press_at[0]) + abs(event.y_root - self._press_at[1])
        if self.hero_pressed and moved < 5 and self._in_box(self.renderer.hero_box, event):
            self._set_procs(not self.show_procs)  # a click (not a drag) on the CPU block
        if self.cfg["pinned"]:
            self._back_if_pinned()
            return
        self._save_position()

    def _hover(self, event):
        self._set_hover(self._in_close(event))
        on_pin = self._in_box(self.renderer.pin_box, event)
        if on_pin != self.pin_hover:
            self.pin_hover = on_pin
            self._draw()
        hand = (self._in_close(event) or on_pin
                or self._in_box(self.renderer.hero_box, event))
        cursor = "hand2" if hand else ""
        if self.view.cget("cursor") != cursor:
            self.view.configure(cursor=cursor)

    # --- processes & alerts ---
    def _set_procs(self, show):
        self.show_procs = show
        self.procs_var.set(show)
        if self.data is not None:
            self.data["procs"] = top_processes() if show else None
            self._draw()

    def _toggle_alerts(self):
        self.cfg["alerts"] = self.alerts_var.get()
        save_config(self.cfg)

    def _set_limit(self, key):
        self.cfg[key] = self.limit_vars[key].get()
        self.alert_state.pop({"alert_temp": "cpu_temp", "alert_ram": "ram",
                              "alert_battery": "battery"}[key], None)  # re-arm
        if key == "alert_temp":
            self.alert_state.pop("gpu_temp", None)
        save_config(self.cfg)

    def _toggle_section(self, key):
        hidden = [k for k in self.cfg["hidden"] if k != key]
        if not self.section_vars[key].get():
            hidden.append(key)
        self.cfg["hidden"] = hidden
        save_config(self.cfg)
        self._draw()

    def _alert(self, key, active, cleared, hold_s, title, body, color):
        """Fire a toast when `active` has held for hold_s; re-arm once `cleared`."""
        now = time.monotonic()
        st = self.alert_state.setdefault(key, {"armed": True, "since": None, "last": -1e9})
        if active:
            st["since"] = st["since"] or now
            if (st["armed"] and now - st["since"] >= hold_s
                    and now - st["last"] >= ALERT_COOLDOWN_S):
                st["armed"], st["last"] = False, now
                self.toast.show(title, body() if callable(body) else body, color)
        else:
            st["since"] = None
            if cleared:
                st["armed"] = True

    def _check_alerts(self, data):
        if not self.cfg["alerts"] or self.toast is None:
            return
        scale = Scale(THEMES[self.cfg["theme"]][1])
        hot = scale(1.0)
        limit = self.cfg["alert_temp"]
        t = data["temp_cpu"]
        if t is not None:
            self._alert("cpu_temp", t >= limit, t < limit - ALERT_HYST, 5,
                        "Процессор перегревается",
                        f"CPU {t:.0f}°. Закрой тяжёлые программы или проверь, не забиты ли решётки.",
                        hot)
        t = data["temp_gpu"]
        if t is not None:
            self._alert("gpu_temp", t >= limit - 3, t < limit - 3 - ALERT_HYST, 5,
                        "Видеокарта перегревается",
                        f"GPU {t:.0f}°. Понизь настройки графики или ограничь FPS.", hot)
        pct = data["ram_pct"]

        def ram_body():
            name, _, mem = (top_processes(1, key="mem") or [("?", 0, 0)])[0]
            return (f"Занято {pct:.0f}% ({fmt_gb(data['ram_used'])} из {fmt_gb(data['ram_total'])} ГБ). "
                    f"Больше всех: {name} — {fmt_mem(mem)}.")

        ram_limit = self.cfg["alert_ram"]
        self._alert("ram", pct >= ram_limit, pct < ram_limit - ALERT_HYST, 10,
                    "Память почти закончилась", ram_body, hot)
        batt = data["battery"]
        if batt is not None:
            level, plugged = batt
            low = self.cfg["alert_battery"]
            left = data.get("battery_left")
            self._alert("battery", not plugged and level <= low,
                        plugged or level > low + ALERT_HYST, 0,
                        "Батарея садится",
                        f"Осталось {level:.0f}%" + (f", примерно на {fmt_left(left)}" if left else "")
                        + ". Подключи зарядку.",
                        scale(0.75))

    def _leave(self, event):
        self.pin_hover = False
        self._set_hover(False)
        self._draw()

    def _set_hover(self, value):
        if value != self.close_hover:
            self.close_hover = value
            self._draw()

    def _save_position(self):
        self.cfg["x"], self.cfg["y"] = self.root.winfo_x(), self.root.winfo_y()
        save_config(self.cfg)

    # --- data refresh ---
    def update(self):
        try:
            self._refresh()
        except Exception:  # a single bad reading must not kill the widget
            log_error("refresh")
        self.root.after(UPDATE_MS, self.update)

    def _collect(self):
        now = time.monotonic()
        dt = max(now - self.last_t, 0.001)
        self.last_t = now

        cpu = psutil.cpu_percent(interval=None)
        self.cpu_hist = (self.cpu_hist + [cpu])[-HISTORY_LEN:]
        mem = psutil.virtual_memory()

        if now - self.partitions_t >= PARTITIONS_RESCAN_S:  # flash drive plugged in / out
            self.partitions_t = now
            try:
                self.partitions = scan_partitions()
            except OSError:
                log_error("listing disks")
        disks = []
        for mount in self.partitions:
            try:
                du = psutil.disk_usage(mount)
            except OSError:
                continue
            disks.append((mount.rstrip("\\"), du.used, du.total, du.percent))

        io_r = io_w = 0.0
        disk = psutil.disk_io_counters()
        if disk and self.last_disk:
            io_r = (disk.read_bytes - self.last_disk.read_bytes) / dt
            io_w = (disk.write_bytes - self.last_disk.write_bytes) / dt
        self.last_disk = disk

        net = psutil.net_io_counters()
        down = max((net.bytes_recv - self.last_net.bytes_recv) / dt, 0.0)
        up = max((net.bytes_sent - self.last_net.bytes_sent) / dt, 0.0)
        self.last_net = net
        self.net_hist = (self.net_hist + [(down, up)])[-HISTORY_LEN:]

        batt = psutil.sensors_battery()
        left = None
        if batt is not None and not batt.power_plugged:
            secs = batt.secsleft  # Windows' estimate; unknown right after unplugging
            if isinstance(secs, int) and 0 < secs < 48 * 3600:
                left = secs
        sensors = self.lhm.sensors  # one consistent snapshot from the reader thread
        return {
            "cpu": cpu,
            "cpu_hist": list(self.cpu_hist),
            "ram_pct": mem.percent,
            "ram_used": mem.used,
            "ram_total": mem.total,
            "disks": disks,
            "io_read": max(io_r, 0.0),
            "io_write": max(io_w, 0.0),
            "net_down": down,
            "net_up": up,
            "net_hist": list(self.net_hist),
            "battery": (batt.percent, bool(batt.power_plugged)) if batt else None,
            "battery_left": left,
            "uptime": time.time() - psutil.boot_time(),
            "temp_cpu": sensors["cpu"],
            "temp_gpu": sensors["gpu"],
            "gpu_shared": sensors["gpu_shared"],
            "gpu_load": sensors["gpu_load"],
            "gpu_name": sensors["gpu_name"],
            "temp_ssd": sensors["ssd"],
            "cpu_clock": sensors.get("cpu_clock"),
            "cpu_power": sensors.get("cpu_power"),
            "lhm_online": self.lhm.online,
            "sensor_hint": self.lhm.hint(),
            "ping": self.ping.ms,
            "ping_ready": self.ping.ready,
            "procs": top_processes() if self.show_procs else None,
        }

    def _refresh(self):
        self.data = self._collect()
        self._draw()
        if getattr(self, "taskbar", None) is not None:
            self.taskbar.draw(self.data, self.cfg["theme"])
        if getattr(self, "toast", None) is not None:
            self._check_alerts(self.data)
        # explorer.exe restarted -> the desktop view changed, re-attach
        if self.cfg["pinned"] and self.hwnd and not owner_alive(self.hwnd):
            self._apply_pin()

    def _draw(self):
        if self.data is None:
            return
        img = self.renderer.render(self.data, self.cfg["theme"], self.close_hover,
                                   bool(self.cfg["pinned"]), self.pin_hover,
                                   compact=bool(self.cfg["compact"]),
                                   close_armed=self.close_armed,
                                   hidden=tuple(self.cfg["hidden"]))
        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        if img.height != self.height:
            self.height = img.height
            self.root.geometry(f"{img.width}x{img.height}")

    def run(self):
        try:
            self.root.mainloop()
        finally:
            if getattr(self, "taskbar", None) is not None:
                self.taskbar.restore()


PARTITIONS_RESCAN_S = 5


def scan_partitions():
    """Mount points of local drives with a file system (USB sticks too, no CD drives)."""
    return [p.mountpoint for p in psutil.disk_partitions(all=False)
            if p.fstype and "cdrom" not in p.opts]


def already_running():
    """Named mutex: a second copy (start.bat clicked twice) exits immediately."""
    try:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateMutexW.restype = ctypes.c_void_p
        kernel32.CreateMutexW.argtypes = [ctypes.c_void_p, ctypes.c_bool, ctypes.c_wchar_p]
        handle = kernel32.CreateMutexW(None, False, r"Local\PCMonitorWidget")
        error = ctypes.get_last_error()
        globals()["_instance_mutex"] = handle
        # 183 = already exists; 5 = exists but belongs to an elevated copy
        return error == 183 or (not handle and error == 5)
    except (AttributeError, OSError):
        return False


def release_instance():
    handle = globals().pop("_instance_mutex", None)
    if handle:
        ctypes.windll.kernel32.CloseHandle(ctypes.c_void_p(handle))


def write_diagnostics():
    """`PC-Monitor.exe --diag`: write what the sensors give into pc-monitor-diag.txt."""
    lines = [f"admin: {is_admin()}", f"frozen: {getattr(sys, 'frozen', False)}",
             f"lhm dir: {lhm_dir()}"]
    try:
        native = NativeSensors(lhm_dir())
        lines.append(f"native: {read_lhm(native.tree())}")
    except Exception as exc:
        lines.append(f"native failed: {exc!r}")
    try:
        lines.append(f"web server: {_fetch_web()}")
    except (OSError, ValueError) as exc:
        lines.append(f"web server: not available ({exc.__class__.__name__})")
    with open(os.path.join(DATA_DIR, "pc-monitor-diag.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    setup_logging()
    if "--diag" in sys.argv:
        write_diagnostics()
        sys.exit(0)
    if not already_running():
        wants_admin = (load_config()["run_as_admin"] and lhm_dir()
                       and "--no-elevate" not in sys.argv)
        if wants_admin and not is_admin():
            release_instance()  # let the elevated copy take over the single-instance lock
            if relaunch_as_admin():
                sys.exit(0)
            if already_running():  # lost a race while UAC was open
                sys.exit(0)
        elif wants_admin and is_frozen() and not running_installed():
            # elevated, but from a folder anyone can write to (e.g. Downloads):
            # copy this build to Program Files and carry on from there
            if install_copy():
                release_instance()
                if start_installed():
                    sys.exit(0)
                already_running()
        log.info("start %s: admin=%s frozen=%s dir=%s", APP_VERSION, is_admin(),
                 getattr(sys, "frozen", False), APP_DIR)
        Widget().run()

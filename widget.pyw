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
import queue
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

APP_VERSION = "1.4.1"

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


# ---------- languages ----------
# Russian is the source language; translations.py holds English, Ukrainian and
# German. tr() looks a text up in the current language; dec() writes a decimal
# comma everywhere but in English.

try:
    from translations import LANGS, TEXTS
except ImportError:  # translations.py missing: Russian only
    LANGS, TEXTS = {"ru": "Русский"}, {}
_LANG_COLUMN = {"en": 0, "uk": 1, "de": 2}
_lang = "ru"


def system_language():
    """Windows display language -> one of LANGS (English when it's another one)."""
    try:
        primary = ctypes.windll.kernel32.GetUserDefaultUILanguage() & 0x3FF
    except (AttributeError, OSError):
        return "en"
    code = {0x19: "ru", 0x22: "uk", 0x07: "de"}.get(primary, "en")
    return code if code in LANGS else "ru"


def set_language(choice):
    """choice: a code from LANGS or "auto" (the Windows language)."""
    global _lang
    _lang = choice if choice in LANGS else system_language()


def tr(text, **fields):
    """The text in the current language; fields fill its {name} places."""
    if _lang != "ru":
        row = TEXTS.get(text)
        if row:
            text = row[_LANG_COLUMN[_lang]]
    return text.format(**fields) if fields else text


def dec(text):
    """Decimal comma (Russian, Ukrainian, German) or point (English)."""
    return text if _lang == "en" else text.replace(".", ",")


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
                  "custom_colors": ["#2ec4b6", "#e8445a"],
                  "pinned": True, "taskbar": not IS_WIN11, "alerts": True, "compact": False,
                  "run_as_admin": False, "hidden": [],
                  "alert_temp": 90, "alert_ram": 90, "alert_battery": 15,
                  "check_updates": True, "update_seen": "", "tray": True,
                  "ping_host": "1.1.1.1", "fps": True, "lang": "auto", "in_games": "hide",
                  "size": 100}

# alerts fire at the limit chosen in the menu and re-arm ALERT_HYST below it
# (above it for the battery), so a value hovering around the limit doesn't spam;
# each alert at most once per ALERT_COOLDOWN_S. GPU limit = CPU limit - 3.
ALERT_HYST = 5
ALERT_COOLDOWN_S = 600
CLOSE_CONFIRM_MS = 3000
DIM_ALPHA = 0.3  # widget opacity while a game or a fullscreen video is in front


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


custom_stops = list(DEFAULT_CONFIG["custom_colors"])  # the "custom" theme: low -> high


def theme_scale(theme):
    """Colour scale of a theme; "custom" is a gradient between the user's two colours."""
    return Scale(custom_stops if theme == "custom" else THEMES[theme][1])


def fmt_gb(n_bytes):
    return dec(f"{n_bytes / 2**30:.1f}")


def fmt_speed(bytes_per_s):
    if bytes_per_s < 1024:
        return tr("{v} Б/с", v=f"{bytes_per_s:.0f}")
    if bytes_per_s < 1024**2:
        return tr("{v} КБ/с", v=f"{bytes_per_s / 1024:.0f}")
    return tr("{v} МБ/с", v=dec(f"{bytes_per_s / 1024**2:.1f}"))


def fmt_mem(n_bytes):
    if n_bytes < 2**30:
        return tr("{v} МБ", v=f"{n_bytes / 2**20:.0f}")
    return tr("{v} ГБ", v=dec(f"{n_bytes / 2**30:.1f}"))


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


def fmt_wh(wh):
    return dec(f"{wh:.1f}")


def fmt_left(seconds):
    """Battery time left: '2 ч 15 мин', '45 мин'."""
    minutes = int(seconds) // 60
    hours, minutes = divmod(minutes, 60)
    return tr("{h} ч {m} мин", h=hours, m=minutes) if hours else tr("{m} мин", m=minutes)


def cpu_extra_text(data):
    """'3,2 ГГц · 12 Вт' from the sensors, or '' when they don't give it."""
    parts = []
    clock, power = data.get("cpu_clock"), data.get("cpu_power")
    if clock:
        parts.append(tr("{v} ГГц", v=dec(f"{clock:.1f}")))
    if power:
        parts.append(tr("{v} Вт", v=f"{power:.0f}" if power >= 10 else dec(f"{power:.1f}")))
    return "  ·  ".join(parts)


def fmt_uptime(seconds):
    seconds = int(seconds)
    days, rest = divmod(seconds, 86400)
    hours, rest = divmod(rest, 3600)
    minutes = rest // 60
    if days:
        return tr("{d} д {h} ч", d=days, h=hours)
    return tr("{h} ч {m} мин", h=hours, m=minutes)


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
            if v in THEMES or v == "custom":
                cfg[key] = v
        elif key == "custom_colors":
            if (isinstance(v, list) and len(v) == 2
                    and all(isinstance(c, str) and re.fullmatch(r"#[0-9a-fA-F]{6}", c) for c in v)):
                cfg[key] = [c.lower() for c in v]
        elif isinstance(default, bool):
            if isinstance(v, bool):
                cfg[key] = v
        elif key in CONFIG_CHOICES:
            if v in CONFIG_CHOICES[key]:
                cfg[key] = v
        elif key == "hidden":
            if isinstance(v, list):
                cfg[key] = [s for s in v if s in SECTIONS]
        elif key == "ping_host":
            if isinstance(v, str) and clean_host(v) == v:
                cfg[key] = v
        elif key == "update_seen":
            if isinstance(v, str) and (v == "" or parse_version(v)):
                cfg[key] = v
    return cfg


# key -> allowed values
CONFIG_CHOICES = {"alert_temp": (80, 85, 90, 95), "alert_ram": (80, 85, 90, 95),
                  "alert_battery": (10, 15, 20, 30), "lang": ("auto", "ru", "en", "uk", "de"),
                  "size": (100, 125, 150), "in_games": ("hide", "dim", "off")}
# sections that can be hidden from the menu: key -> menu title
SECTIONS = {"cores": "Ядра", "ram": "Память", "gpu": "Видеокарта", "disks": "Диски", "temps": "Температура",
            "net": "Сеть", "net_graph": "График сети", "ping": "Пинг",
            "disk_io": "Чтение и запись диска", "battery": "Износ батареи", "fans": "Вентиляторы",
            "uptime": "Время без перезагрузки"}


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


# ---------- FPS (admin mode) ----------
# Frames are counted the way PresentMon does at its simplest: every DXGI
# Present call (Direct3D 10/11/12) raises a Present_Start event in the
# Microsoft-Windows-DXGI ETW provider. A real-time ETW session needs admin
# rights. Vulkan and OpenGL games don't go through DXGI and show no FPS.

from ctypes import wintypes as _w

FPS_SESSION = "PCMonitorFPS"
DXGI_PROVIDER = "{CA11C036-0102-4A2D-A6AD-F03CFED5D3C9}"
DXGI_PRESENT_IDS = (42, 55)  # Present_Start, PresentMultiplaneOverlay_Start
# Every present also reaches the kernel graphics driver, whatever the API — so
# DirectX 9 (Terraria), OpenGL (Minecraft, STALCRAFT) and Vulkan show up here.
# Measured on Vlad's PC: Present_Info comes exactly once per frame (Civ VI: 41.5/s
# against 41.6 DXGI presents; Terraria: 60.0/s, no DXGI events at all).
DXGKRNL_PROVIDER = "{802EC45A-1E99-4B83-9920-87C98277BA9D}"
DXGKRNL_PRESENT_INFO = 184
DXGKRNL_BASE = 0x1  # the keyword that carries Present_Info
FPS_WINDOW_S = 2.0           # frames are averaged over this much of the latest events


class _GUID(ctypes.Structure):
    _fields_ = [("Data1", ctypes.c_ulong), ("Data2", ctypes.c_ushort),
                ("Data3", ctypes.c_ushort), ("Data4", ctypes.c_ubyte * 8)]


class _WNODE_HEADER(ctypes.Structure):
    _fields_ = [("BufferSize", ctypes.c_ulong), ("ProviderId", ctypes.c_ulong),
                ("HistoricalContext", ctypes.c_uint64), ("TimeStamp", ctypes.c_int64),
                ("Guid", _GUID), ("ClientContext", ctypes.c_ulong), ("Flags", ctypes.c_ulong)]


class _EVENT_TRACE_PROPERTIES(ctypes.Structure):
    _fields_ = [("Wnode", _WNODE_HEADER), ("BufferSize", ctypes.c_ulong),
                ("MinimumBuffers", ctypes.c_ulong), ("MaximumBuffers", ctypes.c_ulong),
                ("MaximumFileSize", ctypes.c_ulong), ("LogFileMode", ctypes.c_ulong),
                ("FlushTimer", ctypes.c_ulong), ("EnableFlags", ctypes.c_ulong),
                ("AgeLimit", ctypes.c_long), ("NumberOfBuffers", ctypes.c_ulong),
                ("FreeBuffers", ctypes.c_ulong), ("EventsLost", ctypes.c_ulong),
                ("BuffersWritten", ctypes.c_ulong), ("LogBuffersLost", ctypes.c_ulong),
                ("RealTimeBuffersLost", ctypes.c_ulong), ("LoggerThreadId", _w.HANDLE),
                ("LogFileNameOffset", ctypes.c_ulong), ("LoggerNameOffset", ctypes.c_ulong)]


class _EVENT_TRACE_HEADER(ctypes.Structure):
    _fields_ = [("Size", ctypes.c_ushort), ("FieldTypeFlags", ctypes.c_ushort),
                ("Version", ctypes.c_ulong), ("ThreadId", ctypes.c_ulong),
                ("ProcessId", ctypes.c_ulong), ("TimeStamp", ctypes.c_int64), ("Guid", _GUID),
                ("ProcessorTime", ctypes.c_uint64)]


class _EVENT_TRACE(ctypes.Structure):
    _fields_ = [("Header", _EVENT_TRACE_HEADER), ("InstanceId", ctypes.c_ulong),
                ("ParentInstanceId", ctypes.c_ulong), ("ParentGuid", _GUID),
                ("MofData", ctypes.c_void_p), ("MofLength", ctypes.c_ulong),
                ("ClientContext", ctypes.c_ulong)]


class _SYSTEMTIME(ctypes.Structure):
    _fields_ = [(n, ctypes.c_ushort) for n in ("wYear", "wMonth", "wDayOfWeek", "wDay", "wHour",
                                                "wMinute", "wSecond", "wMilliseconds")]


class _TIME_ZONE_INFORMATION(ctypes.Structure):
    _fields_ = [("Bias", ctypes.c_long), ("StandardName", ctypes.c_wchar * 32),
                ("StandardDate", _SYSTEMTIME), ("StandardBias", ctypes.c_long),
                ("DaylightName", ctypes.c_wchar * 32), ("DaylightDate", _SYSTEMTIME),
                ("DaylightBias", ctypes.c_long)]


class _TRACE_LOGFILE_HEADER(ctypes.Structure):
    _fields_ = [("BufferSize", ctypes.c_ulong), ("Version", ctypes.c_ulong),
                ("ProviderVersion", ctypes.c_ulong), ("NumberOfProcessors", ctypes.c_ulong),
                ("EndTime", ctypes.c_int64), ("TimerResolution", ctypes.c_ulong),
                ("MaximumFileSize", ctypes.c_ulong), ("LogFileMode", ctypes.c_ulong),
                ("BuffersWritten", ctypes.c_ulong), ("LogInstanceGuid", _GUID),
                ("LoggerName", ctypes.c_void_p), ("LogFileName", ctypes.c_void_p),
                ("TimeZone", _TIME_ZONE_INFORMATION), ("BootTime", ctypes.c_int64),
                ("PerfFreq", ctypes.c_int64), ("StartTime", ctypes.c_int64),
                ("ReservedFlags", ctypes.c_ulong), ("BuffersLost", ctypes.c_ulong)]


class _EVENT_DESCRIPTOR(ctypes.Structure):
    _fields_ = [("Id", ctypes.c_ushort), ("Version", ctypes.c_ubyte), ("Channel", ctypes.c_ubyte),
                ("Level", ctypes.c_ubyte), ("Opcode", ctypes.c_ubyte), ("Task", ctypes.c_ushort),
                ("Keyword", ctypes.c_uint64)]


class _EVENT_HEADER(ctypes.Structure):
    """Head of EVENT_RECORD: all the callback needs."""
    _fields_ = [("Size", ctypes.c_ushort), ("HeaderType", ctypes.c_ushort),
                ("Flags", ctypes.c_ushort), ("EventProperty", ctypes.c_ushort),
                ("ThreadId", ctypes.c_ulong), ("ProcessId", ctypes.c_ulong),
                ("TimeStamp", ctypes.c_int64), ("ProviderId", _GUID),
                ("EventDescriptor", _EVENT_DESCRIPTOR), ("ProcessorTime", ctypes.c_uint64),
                ("ActivityId", _GUID)]


_EVENT_RECORD_CALLBACK = ctypes.WINFUNCTYPE(None, ctypes.POINTER(_EVENT_HEADER))


class _EVENT_FILTER_DESCRIPTOR(ctypes.Structure):
    _fields_ = [("Ptr", ctypes.c_uint64), ("Size", ctypes.c_ulong), ("Type", ctypes.c_ulong)]


class _ENABLE_TRACE_PARAMETERS(ctypes.Structure):
    _fields_ = [("Version", ctypes.c_ulong), ("EnableProperty", ctypes.c_ulong),
                ("ControlFlags", ctypes.c_ulong), ("SourceId", _GUID),
                ("EnableFilterDesc", ctypes.POINTER(_EVENT_FILTER_DESCRIPTOR)),
                ("FilterDescCount", ctypes.c_ulong)]


def _event_id_filter(ids):
    """ENABLE_TRACE_PARAMETERS that let only these event ids through, so the
    provider's other (very frequent) events never reach us. Returns the objects
    that must stay alive while EnableTraceEx2 runs."""
    class EVENT_FILTER_EVENT_ID(ctypes.Structure):
        _fields_ = [("FilterIn", ctypes.c_ubyte), ("Reserved", ctypes.c_ubyte),
                    ("Count", ctypes.c_ushort), ("Events", ctypes.c_ushort * len(ids))]
    body = EVENT_FILTER_EVENT_ID(1, 0, len(ids), (ctypes.c_ushort * len(ids))(*ids))
    desc = _EVENT_FILTER_DESCRIPTOR(ctypes.addressof(body), ctypes.sizeof(body),
                                    0x80000200)  # EVENT_FILTER_TYPE_EVENT_ID
    params = _ENABLE_TRACE_PARAMETERS(Version=2, EnableFilterDesc=ctypes.pointer(desc),
                                      FilterDescCount=1)
    return params, (body, desc)


class _EVENT_TRACE_LOGFILEW(ctypes.Structure):
    _fields_ = [("LogFileName", ctypes.c_wchar_p), ("LoggerName", ctypes.c_wchar_p),
                ("CurrentTime", ctypes.c_int64), ("BuffersRead", ctypes.c_ulong),
                ("ProcessTraceMode", ctypes.c_ulong), ("CurrentEvent", _EVENT_TRACE),
                ("LogfileHeader", _TRACE_LOGFILE_HEADER), ("BufferCallback", ctypes.c_void_p),
                ("BufferSize", ctypes.c_ulong), ("Filled", ctypes.c_ulong),
                ("EventsLost", ctypes.c_ulong), ("EventRecordCallback", _EVENT_RECORD_CALLBACK),
                ("IsKernelTrace", ctypes.c_ulong), ("Context", ctypes.c_void_p)]


FRAME_STATS_S = 10.0  # 1% low is taken over this many seconds of frames
FRAME_PAUSE_S = 1.0   # a longer gap is a pause (alt-tab, loading), not a stutter


def best_stats(sources, now=None):
    """frame_stats() of the source (DXGI or kernel) that saw more frames."""
    best = None
    for times in sources:
        st = frame_stats(times, now)
        if st is not None and (best is None or st[0] > best[0]):
            best = st
    return best


def frame_stats(times, now=None):
    """(fps, 1% low fps or None, last frame times in ms) from present timestamps,
    or None when the process isn't drawing. 1% low = 1 / 99th percentile frame time."""
    fps = fps_from_times(times, now)
    if fps is None:
        return None
    last = times[-1]
    recent = [t for t in times if t >= last - FRAME_STATS_S]
    gaps = [b - a for a, b in zip(recent, recent[1:]) if 0 < b - a < FRAME_PAUSE_S]
    if len(gaps) < 20:
        return fps, None, [g * 1000 for g in gaps]
    worst = sorted(gaps)[min(len(gaps) - 1, int(len(gaps) * 0.99))]
    return fps, 1 / worst, [g * 1000 for g in gaps[-48:]]


def fps_from_times(times, now=None, window=FPS_WINDOW_S, stale=2.5):
    """Frames per second from QPC-second timestamps of presents (sorted), or None
    when the process hasn't presented lately."""
    if len(times) < 2:
        return None
    now = time.perf_counter() if now is None else now
    last = times[-1]
    if now - last > stale + window:  # events arrive in batches up to ~1 s late
        return None
    recent = [t for t in times if t >= last - window]
    if len(recent) < 2 or recent[-1] <= recent[0]:
        return None
    return (len(recent) - 1) / (recent[-1] - recent[0])


class FpsReader(threading.Thread):
    """Real-time ETW session on the DXGI and DxgKrnl providers; stats(pid) for any
    process. Each process's presents are kept per source; the one that saw more
    frames counts (one present is seen by both for DirectX 10-12)."""

    def __init__(self):
        super().__init__(daemon=True, name="fps")
        self.frames = {}       # (pid, "dxgi" | "krnl") -> present times (QPC s), last seconds
        self.lock = threading.Lock()
        self.error = None      # why it doesn't work, shown in a toast
        self.needs_admin = False
        self.running = False
        self.session = None
        self._first_logged = False

    # --- session control ---
    def _props(self):
        size = ctypes.sizeof(_EVENT_TRACE_PROPERTIES)
        buf = ctypes.create_string_buffer(size + 2 * (len(FPS_SESSION) + 1) + 16)
        props = _EVENT_TRACE_PROPERTIES.from_buffer(buf)
        props.Wnode.BufferSize = len(buf)
        props.Wnode.Flags = 0x00020000          # WNODE_FLAG_TRACED_GUID
        props.Wnode.ClientContext = 1           # timestamps in QPC units
        props.LogFileMode = 0x00000100          # EVENT_TRACE_REAL_TIME_MODE
        props.FlushTimer = 1                    # deliver at least once a second
        props.BufferSize = 16                   # KB
        props.LoggerNameOffset = size
        return buf, props

    @staticmethod
    def stop_session():
        """Stop our session by name (also one left behind by a crashed run)."""
        buf = FpsReader._props(None)[0]
        ctypes.windll.advapi32.ControlTraceW(ctypes.c_uint64(0), FPS_SESSION, buf, 1)  # STOP

    def stop(self):
        self.running = False
        if self.session is not None:
            self.stop_session()

    # --- per-process frame times ---
    def stats(self, pid):
        with self.lock:
            sources = [list(self.frames.get((pid, src), ())) for src in ("dxgi", "krnl")]
        return best_stats(sources)

    def fps(self, pid):
        st = self.stats(pid)
        return st[0] if st else None

    def run(self):
        try:
            self._run()
        except Exception:
            log.exception("FPS counter failed")
            self.error = self.error or tr("ошибка, см. лог")
        finally:
            self.running = False

    def _run(self):
        adv = ctypes.windll.advapi32
        adv.StartTraceW.argtypes = [ctypes.POINTER(ctypes.c_uint64), ctypes.c_wchar_p, ctypes.c_void_p]
        adv.EnableTraceEx2.argtypes = [ctypes.c_uint64, ctypes.POINTER(_GUID), ctypes.c_ulong,
                                       ctypes.c_ubyte, ctypes.c_uint64, ctypes.c_uint64,
                                       ctypes.c_ulong, ctypes.c_void_p]
        adv.OpenTraceW.argtypes = [ctypes.POINTER(_EVENT_TRACE_LOGFILEW)]
        adv.OpenTraceW.restype = ctypes.c_uint64
        adv.ProcessTrace.argtypes = [ctypes.POINTER(ctypes.c_uint64), ctypes.c_ulong,
                                     ctypes.c_void_p, ctypes.c_void_p]
        adv.CloseTrace.argtypes = [ctypes.c_uint64]

        handle = ctypes.c_uint64()
        buf, _ = self._props()
        rc = adv.StartTraceW(ctypes.byref(handle), FPS_SESSION, buf)
        if rc == 183:  # ERROR_ALREADY_EXISTS: left over from a crash
            self.stop_session()
            buf, _ = self._props()
            rc = adv.StartTraceW(ctypes.byref(handle), FPS_SESSION, buf)
        if rc != 0:
            self.needs_admin = rc == 5
            self.error = tr("нужны права администратора") if rc == 5 else tr("ошибка ETW {rc}", rc=rc)
            log.warning("FPS: StartTrace failed with %d", rc)
            return
        self.session = handle.value
        guid = _GUID()
        ctypes.oledll.ole32.CLSIDFromString(DXGI_PROVIDER, ctypes.byref(guid))
        params, keep_alive = _event_id_filter(DXGI_PRESENT_IDS)
        rc = adv.EnableTraceEx2(handle.value, ctypes.byref(guid), 1, 4,  # ENABLE, INFORMATION
                                0xFFFFFFFFFFFFFFFF, 0, 0, ctypes.byref(params))
        if rc != 0:
            self.error = tr("ошибка ETW {rc}", rc=rc)
            log.warning("FPS: EnableTraceEx2 failed with %d", rc)
            self.stop_session()
            return
        dxgi_guid = (guid.Data1, guid.Data2)
        kguid = _GUID()
        ctypes.oledll.ole32.CLSIDFromString(DXGKRNL_PROVIDER, ctypes.byref(kguid))
        params, keep_alive2 = _event_id_filter((DXGKRNL_PRESENT_INFO,))
        rc = adv.EnableTraceEx2(handle.value, ctypes.byref(kguid), 1, 4,
                                DXGKRNL_BASE, 0, 0, ctypes.byref(params))
        if rc != 0:  # DirectX 10-12 games still work through DXGI
            log.warning("FPS: the kernel graphics provider failed with %d", rc)

        freq = ctypes.c_int64()
        ctypes.windll.kernel32.QueryPerformanceFrequency(ctypes.byref(freq))
        qpc = float(freq.value)
        keep = FRAME_STATS_S + 4

        def on_event(rec):
            h = rec.contents
            eid = h.EventDescriptor.Id
            if (h.ProviderId.Data1, h.ProviderId.Data2) == dxgi_guid:
                if eid not in DXGI_PRESENT_IDS:
                    return
                source = "dxgi"
            elif eid == DXGKRNL_PRESENT_INFO:
                source = "krnl"
            else:
                return
            t = h.TimeStamp / qpc
            pid = h.ProcessId
            with self.lock:
                times = self.frames.setdefault((pid, source), [])
                times.append(t)
                if len(times) > 64 and times[0] < t - keep:
                    del times[:len(times) // 2]
            if not self._first_logged:
                self._first_logged = True
                log.info("FPS: first frame seen (pid %d)", pid)

        self._callback = _EVENT_RECORD_CALLBACK(on_event)  # keep a reference
        logfile = _EVENT_TRACE_LOGFILEW()
        logfile.LoggerName = FPS_SESSION
        logfile.ProcessTraceMode = 0x00000100 | 0x10000000  # REAL_TIME | EVENT_RECORD
        logfile.EventRecordCallback = self._callback
        trace = adv.OpenTraceW(ctypes.byref(logfile))
        if trace == 0xFFFFFFFFFFFFFFFF:
            self.error = tr("ошибка ETW {rc}", rc="OpenTrace")
            log.warning("FPS: OpenTrace failed")
            self.stop_session()
            return
        self.running = True
        log.info("FPS: ETW session started")
        try:
            traces = (ctypes.c_uint64 * 1)(trace)
            adv.ProcessTrace(traces, 1, None, None)  # blocks until the session stops
        finally:
            adv.CloseTrace(trace)
            # forget processes that ended
            with self.lock:
                self.frames.clear()


# processes whose frames are not a game's (browsers, chats, players, shell)
FPS_IGNORE = {"chrome", "msedge", "firefox", "opera", "brave", "vivaldi", "browser", "yandex",
              "msedgewebview2", "telegram", "discord", "steamwebhelper", "explorer",
              "applicationframehost", "searchapp", "searchhost", "startmenuexperiencehost",
              "shellexperiencehost", "textinputhost", "vlc", "mpc-hc64", "mpc-be64",
              "potplayermini64", "spotify", "code", "devenv", "obs64", "pc-monitor", "python",
              "pythonw", "dwm", "winword", "excel", "powerpnt", "outlook", "onenote", "teams",
              "ms-teams", "zoom", "slack", "whatsapp"}


# ---------- tray icon ----------
# A notification-area icon needs a window that receives its messages, so it
# has its own hidden window and message loop in a thread (like the hotkey);
# the Tk thread picks up the clicks from a queue.

def _icon_file():
    path = os.path.join(getattr(sys, "_MEIPASS", APP_DIR), "assets", "app.ico")
    return path if os.path.exists(path) else None


class TrayIcon(threading.Thread):
    WM_TRAY = 0x8000 + 1          # WM_APP + 1: our callback message
    WM_LBUTTONUP, WM_RBUTTONUP = 0x0202, 0x0205
    NIM_ADD, NIM_MODIFY, NIM_DELETE = 0, 1, 2
    NIF_MESSAGE, NIF_ICON, NIF_TIP = 1, 2, 4

    def __init__(self, tip="PC Monitor"):
        super().__init__(daemon=True, name="tray")
        import queue
        self.clicks = queue.SimpleQueue()  # "left" / "right"
        self.tip = tip
        self.hwnd = None
        self.hicon = None
        self.added = False
        self.ready = threading.Event()

    def _data(self, flags):
        from ctypes import wintypes as wt

        class NOTIFYICONDATAW(ctypes.Structure):
            _fields_ = [("cbSize", wt.DWORD), ("hWnd", wt.HWND), ("uID", wt.UINT),
                        ("uFlags", wt.UINT), ("uCallbackMessage", wt.UINT), ("hIcon", wt.HICON),
                        ("szTip", wt.WCHAR * 128), ("dwState", wt.DWORD), ("dwStateMask", wt.DWORD),
                        ("szInfo", wt.WCHAR * 256), ("uVersion", wt.UINT),
                        ("szInfoTitle", wt.WCHAR * 64), ("dwInfoFlags", wt.DWORD),
                        ("guidItem", ctypes.c_byte * 16), ("hBalloonIcon", wt.HICON)]

        nid = NOTIFYICONDATAW()
        nid.cbSize = ctypes.sizeof(NOTIFYICONDATAW)
        nid.hWnd, nid.uID, nid.uFlags = self.hwnd, 1, flags
        nid.uCallbackMessage = self.WM_TRAY
        nid.hIcon = self.hicon
        nid.szTip = self.tip[:127]
        return nid

    def _notify(self, action, flags):
        nid = self._data(flags)
        return bool(ctypes.windll.shell32.Shell_NotifyIconW(action, ctypes.byref(nid)))

    def _add(self):
        self.added = self._notify(self.NIM_ADD, self.NIF_MESSAGE | self.NIF_ICON | self.NIF_TIP)

    def set_tip(self, text):
        """Hover text; callable from the Tk thread."""
        if text != self.tip:
            self.tip = text
            if self.added:
                self._notify(self.NIM_MODIFY, self.NIF_TIP)

    def remove(self):
        if self.added:
            self._notify(self.NIM_DELETE, 0)
            self.added = False

    def stop(self):
        """Remove the icon and close the hidden window (ends the thread)."""
        self.remove()
        if self.hwnd:
            ctypes.windll.user32.PostMessageW(self.hwnd, 0x0010, 0, 0)  # WM_CLOSE

    def run(self):
        try:
            self._run()
        except (AttributeError, OSError):
            log.exception("tray icon failed")
        finally:
            self.ready.set()

    def _run(self):
        from ctypes import wintypes as wt
        u = ctypes.WinDLL("user32", use_last_error=True)
        LRESULT = ctypes.c_ssize_t
        WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM)
        u.DefWindowProcW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
        u.DefWindowProcW.restype = LRESULT
        u.CreateWindowExW.restype = wt.HWND
        u.CreateWindowExW.argtypes = [wt.DWORD, wt.LPCWSTR, wt.LPCWSTR, wt.DWORD, ctypes.c_int,
                                      ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.HWND, wt.HMENU,
                                      wt.HINSTANCE, wt.LPVOID]
        u.LoadImageW.restype = wt.HANDLE
        u.LoadImageW.argtypes = [wt.HINSTANCE, wt.LPCWSTR, wt.UINT, ctypes.c_int, ctypes.c_int, wt.UINT]
        u.LoadIconW.restype = wt.HICON
        u.LoadIconW.argtypes = [wt.HINSTANCE, wt.LPVOID]
        u.GetMessageW.argtypes = [ctypes.POINTER(wt.MSG), wt.HWND, wt.UINT, wt.UINT]
        taskbar_created = u.RegisterWindowMessageW("TaskbarCreated")

        def proc(hwnd, msg, wparam, lparam):
            if msg == self.WM_TRAY:
                event = lparam & 0xFFFF
                if event == self.WM_LBUTTONUP:
                    self.clicks.put("left")
                elif event == self.WM_RBUTTONUP:
                    self.clicks.put("right")
                return 0
            if msg == taskbar_created:  # explorer.exe restarted: the icon is gone, add it again
                self._add()
                return 0
            if msg == 0x0002:  # WM_DESTROY (after stop()): end the message loop
                u.PostQuitMessage(0)
                return 0
            return u.DefWindowProcW(hwnd, msg, wparam, lparam)

        self._proc = WNDPROC(proc)  # keep a reference, or ctypes frees the callback

        class WNDCLASSW(ctypes.Structure):
            _fields_ = [("style", wt.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", ctypes.c_int),
                        ("cbWndExtra", ctypes.c_int), ("hInstance", wt.HINSTANCE),
                        ("hIcon", wt.HICON), ("hCursor", wt.HANDLE), ("hbrBackground", wt.HBRUSH),
                        ("lpszMenuName", wt.LPCWSTR), ("lpszClassName", wt.LPCWSTR)]

        hinst = ctypes.windll.kernel32.GetModuleHandleW(None)
        cls = f"PCMonitorTray{id(self)}"  # own class: a re-enabled icon gets a new window proc
        wc = WNDCLASSW(lpfnWndProc=self._proc, hInstance=hinst, lpszClassName=cls)
        u.RegisterClassW(ctypes.byref(wc))
        # a hidden top-level window (not message-only: those miss the TaskbarCreated broadcast)
        self.hwnd = u.CreateWindowExW(0, cls, "PC Monitor", 0, 0, 0, 0, 0, None, None, hinst, None)
        if not self.hwnd:
            raise OSError("CreateWindowExW failed")
        # when we run elevated, Windows drops messages from explorer.exe (lower
        # integrity) unless we let these two through explicitly
        for msg in (self.WM_TRAY, taskbar_created):
            u.ChangeWindowMessageFilterEx(self.hwnd, msg, 1, None)  # MSGFLT_ALLOW

        size = u.GetSystemMetrics(49)  # SM_CXSMICON
        icon = _icon_file()
        if icon:
            self.hicon = u.LoadImageW(None, icon, 1, size, size, 0x10)  # IMAGE_ICON, LR_LOADFROMFILE
        if not self.hicon:
            self.hicon = u.LoadIconW(None, ctypes.c_void_p(32512))  # IDI_APPLICATION
        self._add()
        self.ready.set()
        msg = wt.MSG()
        while u.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            u.TranslateMessage(ctypes.byref(msg))
            u.DispatchMessageW(ctypes.byref(msg))


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
    if "battery" in img:
        return "battery"
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
            elif group in ("Energy", "Levels"):
                hw["energy"].append((low, value))
            elif group == "Fans":
                hw["fans"].append((low, value))
            return
        if text in SENSOR_GROUPS:
            for kid in kids:
                walk(kid, hw, text)
            return
        new_hw = {"kind": _classify_hardware(node), "name": text, "temps": [], "loads": [],
                  "clocks": [], "powers": [], "energy": [], "fans": [], "integrated": False}
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

    battery = read_battery(next((hw for hw in hardware if hw["kind"] == "battery"), None))
    # spinning fans of any device (0 RPM = a stopped fan or an empty header), fastest first
    fans = sorted((v for hw in hardware for _, v in hw["fans"] if 0 < v < 30000), reverse=True)

    cpu_clock = cpu_power = None
    cpu_hw = next((hw for hw in hardware if hw["kind"] == "cpu"), None)
    if cpu_hw:
        cores = [v for n, v in cpu_hw["clocks"] if n.startswith("core #") and v > 100]
        cpu_clock = sum(cores) / len(cores) / 1000 if cores else None  # MHz -> GHz
        power = _pick(cpu_hw["powers"], POWER_PREFS)
        cpu_power = power if power and 0 < power < 1000 else None
    return {"cpu": cpu, "gpu": gpu, "gpu_shared": gpu_shared, "gpu_load": gpu_load,
            "gpu_name": gpu_name, "ssd": ssd, "cpu_clock": cpu_clock, "cpu_power": cpu_power,
            "fans": fans[:3] or None, **battery}


def read_battery(hw):
    """Battery wear from the sensors (works without admin rights):
    battery_design / battery_full in Wh, battery_wear in %, battery_rate in W."""
    out = {"battery_design": None, "battery_full": None, "battery_wear": None,
           "battery_rate": None}
    if hw is None:
        return out
    values = dict(hw["energy"])
    design, full = values.get("designed capacity"), values.get("fully-charged capacity")
    if design and full and design > 0 and full > 0:
        out["battery_design"], out["battery_full"] = design / 1000, full / 1000  # mWh -> Wh
        out["battery_wear"] = max(0.0, (1 - full / design) * 100)
    rate = dict(hw["powers"]).get("charge/discharge rate")
    if rate:
        out["battery_rate"] = abs(rate)
    return out


def extract_temps(tree):
    """(cpu_temp, gpu_temp) — kept for older callers/tests."""
    r = read_lhm(tree)
    return r["cpu"], r["gpu"]


EMPTY_SENSORS = {"cpu": None, "gpu": None, "gpu_shared": False, "gpu_load": None,
                 "gpu_name": None, "ssd": None, "cpu_clock": None, "cpu_power": None,
                 "battery_design": None, "battery_full": None, "battery_wear": None,
                 "battery_rate": None, "fans": None}


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


def unblock_dlls(folder):
    """Remove the 'downloaded from the internet' mark (the Zone.Identifier stream
    Windows adds to files unpacked from a downloaded zip) from the DLLs under
    `folder`: .NET Framework refuses to load marked assemblies, and then there
    are no sensors. Returns how many files were unmarked."""
    count = 0
    for root, _dirs, files in os.walk(folder):
        for name in files:
            if name.lower().endswith(".dll"):
                try:
                    os.remove(os.path.join(root, name) + ":Zone.Identifier")
                    count += 1
                except OSError:  # not marked (the usual case) or no rights
                    pass
    return count


class NativeSensors:
    """Reads sensors in-process with LibreHardwareMonitorLib (via pythonnet).
    Without admin rights it still gives GPU load; temperatures need admin and
    the PawnIO driver. Produces the same tree as LHM's web server, so
    read_lhm() handles both."""

    ICONS = {"Cpu": "cpu", "GpuNvidia": "nvidia", "GpuAmd": "ati", "GpuIntel": "intel",
             "Storage": "hdd", "Battery": "battery"}
    GROUPS = {"Temperature": ("Temperatures", " °C"), "Load": ("Load", " %"),
              "Clock": ("Clocks", " MHz"), "Power": ("Powers", " W"),
              "Energy": ("Energy", " mWh"), "Level": ("Levels", " %"), "Fan": ("Fans", " RPM")}

    def __init__(self, path):
        # the bundled DLLs (pythonnet, clr_loader, LibreHardwareMonitor) must not
        # carry the download mark, or .NET won't load them
        unmarked = unblock_dlls(getattr(sys, "_MEIPASS", path) if is_frozen() else path)
        if unmarked:
            log.info("removed the download mark from %d DLLs", unmarked)
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
        self.computer.IsBatteryEnabled = True  # wear and power draw, no admin rights needed
        self.computer.IsMotherboardEnabled = True  # fans on the board (with admin rights)
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

    def close(self):
        try:
            self.computer.Close()
        except Exception:
            log.exception("closing the sensor library failed")

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
            return tr("Температуры включаются в меню (ПКМ)")
        if self.native_ok:
            return tr("Нет доступа к датчикам: нужен PawnIO")
        if self.online:
            return tr("Запусти LibreHardwareMonitor от админа")
        return tr("Нужен LibreHardwareMonitor с веб-сервером")

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
        failures = 0
        while True:
            found, source = None, None
            if native is not None:
                try:
                    found, source = read_lhm(native.tree()), "native"
                    failures = 0
                except Exception:
                    log_error("reading built-in sensors")
                    found = None
                    failures += 1
                    if failures >= 3:  # stuck: open the library again
                        failures = 0
                        native.close()
                        try:
                            native = NativeSensors(path)
                            log.info("sensor library reopened")
                        except Exception:
                            log.exception("reopening the sensor library failed")
                            native = None
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


# ---------- updates ----------
# Once in a while the widget asks GitHub for the latest release. It only reads
# that one public page: nothing is downloaded or installed by itself.

REPO = "vladZah403/pc-widget"
RELEASES_URL = f"https://github.com/{REPO}/releases"
LATEST_API = f"https://api.github.com/repos/{REPO}/releases/latest"
UPDATE_FIRST_S = 20        # first check soon after the start
UPDATE_EVERY_S = 6 * 3600


def parse_version(text):
    """'v1.10' or '1.10' -> (1, 10); None if it isn't a plain version number."""
    m = re.fullmatch(r"v?(\d+(?:\.\d+){0,3})", (text or "").strip())
    return tuple(int(x) for x in m.group(1).split(".")) if m else None


def newer_release(info, current=APP_VERSION):
    """(version, page url, zip asset or None) from GitHub's 'latest release' JSON
    if it is newer than us."""
    if not isinstance(info, dict) or info.get("draft") or info.get("prerelease"):
        return None
    tag = info.get("tag_name")
    have, got = parse_version(current), parse_version(tag)
    if not have or not got or got <= have:
        return None
    url = info.get("html_url")
    if not isinstance(url, str) or not url.startswith(RELEASES_URL + "/"):
        url = RELEASES_URL + "/latest"  # only ever open our own release page
    return ".".join(map(str, got)), url, release_asset(info)


def release_asset(info):
    """PC-Monitor.zip of a release: {"url", "sha256", "size"}, or None when it's
    missing or anything about it looks off (then only the page is offered)."""
    for asset in info.get("assets") or []:
        if not isinstance(asset, dict) or asset.get("name") != "PC-Monitor.zip":
            continue
        url, digest, size = asset.get("browser_download_url"), asset.get("digest"), asset.get("size")
        if (isinstance(url, str) and url.startswith(f"https://github.com/{REPO}/releases/download/")
                and isinstance(digest, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", digest)
                and isinstance(size, int) and 0 < size < 200 * 2**20):
            return {"url": url, "sha256": digest[7:], "size": size}
    return None


def update_target():
    """(program folder, staging folder) when this copy can update itself, else None.
    Elevated, only the protected Program Files copy is updated (its staging folder
    is protected too); without admin rights, a copy in an ordinary folder."""
    if not is_frozen() or not os.path.isdir(os.path.join(APP_DIR, "_internal")):
        return None
    if running_installed():
        return (INSTALL_DIR, INSTALL_DIR + ".new") if is_admin() else None
    if is_admin():
        return None  # never run a download from a user-writable folder with admin rights
    return APP_DIR, APP_DIR + ".new"


class Updater(threading.Thread):
    """Downloads PC-Monitor.zip into memory, checks its SHA-256 against GitHub's,
    unpacks it into the staging folder; swap_and_restart() finishes the job."""

    def __init__(self, asset, target):
        super().__init__(daemon=True, name="updater")
        self.asset = asset
        self.app_dir, self.staging = target
        self.progress = 0.0
        self.state = "download"  # download -> install -> ready | error
        self.error = None

    def run(self):
        try:
            data = self._download()
            self.state = "install"
            self._unpack(data)
            self.state = "ready"
            log.info("update %s staged in %s", self.asset["url"], self.staging)
        except Exception as exc:
            log.exception("update failed")
            self.error = str(exc) or exc.__class__.__name__
            self.state = "error"

    def _download(self):
        import hashlib
        req = urllib.request.Request(self.asset["url"], headers={"User-Agent": f"PC-Monitor/{APP_VERSION}"})
        size = self.asset["size"]
        data = bytearray()
        with urllib.request.urlopen(req, timeout=30) as resp:
            # read exactly `size` bytes: GitHub's CDN may keep the connection open after them
            while len(data) < size:
                chunk = resp.read(min(256 * 1024, size - len(data)))
                if not chunk:
                    break
                data += chunk
                self.progress = len(data) / size
        if len(data) != size:
            raise OSError(tr("файл скачался не полностью"))
        if hashlib.sha256(data).hexdigest() != self.asset["sha256"]:
            raise OSError(tr("контрольная сумма не совпала"))
        return bytes(data)

    def _unpack(self, data):
        """Only files under PC-Monitor/ inside the zip, never outside the staging folder."""
        import io
        import shutil
        import zipfile
        shutil.rmtree(self.staging, ignore_errors=True)
        os.makedirs(self.staging)
        root = os.path.realpath(self.staging)
        with zipfile.ZipFile(io.BytesIO(data)) as zf:
            for item in zf.infolist():
                name = item.filename.replace("\\", "/")
                if not name.startswith("PC-Monitor/") or name.endswith("/"):
                    continue
                rel = name[len("PC-Monitor/"):]
                target = os.path.realpath(os.path.join(self.staging, *rel.split("/")))
                if os.path.commonpath([root, target]) != root:
                    raise OSError("bad path in the archive: " + name)
                os.makedirs(os.path.dirname(target), exist_ok=True)
                with zf.open(item) as src, open(target, "wb") as dst:
                    shutil.copyfileobj(src, dst)
        if not (os.path.isfile(os.path.join(self.staging, EXE_NAME))
                and os.path.isdir(os.path.join(self.staging, "_internal"))):
            raise OSError(tr("в архиве нет программы"))


def swap_and_restart(app_dir, staging):
    """After we exit: program folder -> .old, staging -> program folder, start it.
    Done by PowerShell from System32 with the script passed in memory (nothing on
    disk to tamper with); if the swap fails, the old version is started again."""
    import base64
    import subprocess
    q = lambda p: "'" + p.replace("'", "''") + "'"
    old = app_dir + ".old"
    exe = os.path.join(app_dir, EXE_NAME)
    script = f"""
Set-Location -LiteralPath $env:SystemRoot
Wait-Process -Id {os.getpid()} -Timeout 30 -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 300
Remove-Item -LiteralPath {q(old)} -Recurse -Force -ErrorAction SilentlyContinue
try {{
  Rename-Item -LiteralPath {q(app_dir)} -NewName {q(os.path.basename(old))} -ErrorAction Stop
  try {{
    Rename-Item -LiteralPath {q(staging)} -NewName {q(os.path.basename(app_dir))} -ErrorAction Stop
  }} catch {{
    Rename-Item -LiteralPath {q(old)} -NewName {q(os.path.basename(app_dir))} -ErrorAction SilentlyContinue
  }}
}} catch {{}}
Start-Process -FilePath {q(exe)} -WorkingDirectory {q(app_dir)} -ErrorAction SilentlyContinue
Start-Sleep -Seconds 5
Remove-Item -LiteralPath {q(old)} -Recurse -Force -ErrorAction SilentlyContinue
"""
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    ps = os.path.join(_system32(), "WindowsPowerShell", "v1.0", "powershell.exe")
    # cwd outside the program folder: a process sitting in it blocks the rename
    subprocess.Popen([ps, "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden",
                      "-EncodedCommand", encoded],
                     creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0), close_fds=True,
                     cwd=_system32())
    log.info("update: swap scheduled for %s", app_dir)


def open_in_browser(url):
    """Open one of our GitHub pages. When we run elevated, explorer.exe hands the
    link to the user's normal shell, so the browser doesn't get admin rights."""
    if not url.startswith(f"https://github.com/{REPO}"):
        return
    try:
        if is_admin():
            import subprocess
            windir = os.path.dirname(_system32())
            subprocess.Popen([os.path.join(windir, "explorer.exe"), url])
        else:
            os.startfile(url)
    except OSError:
        log.exception("opening %s failed", url)


class UpdateChecker(threading.Thread):
    """Background check of the latest release; `found` = (version, url) or None."""

    def __init__(self, enabled):
        super().__init__(daemon=True)
        self.enabled = enabled            # callable -> bool (the menu switch)
        self.found = None
        self.checks = 0                   # finished attempts, successful or not
        self.failed = False               # the last attempt failed (offline, GitHub down)
        self.wake = threading.Event()     # set() = check right now

    def check_now(self):
        self.wake.set()

    def run(self):
        self.wake.wait(UPDATE_FIRST_S)
        while True:
            self.wake.clear()
            if self.enabled():
                try:
                    req = urllib.request.Request(LATEST_API, headers={
                        "User-Agent": f"PC-Monitor/{APP_VERSION}",
                        "Accept": "application/vnd.github+json"})
                    with urllib.request.urlopen(req, timeout=10) as resp:
                        self.found = newer_release(json.loads(resp.read().decode("utf-8")))
                    self.failed = False
                except (OSError, ValueError) as exc:  # offline is normal for a laptop
                    log.info("update check failed: %s", exc)
                    self.failed = True
                self.checks += 1
            self.wake.wait(UPDATE_EVERY_S)


# ---------- history ----------
# A point every HISTORY_STEP_S seconds for the last day, kept in history.csv in
# %APPDATA%\PC-Monitor, so the charts survive restarts. ~8640 short lines max.

HISTORY_PATH = os.path.join(DATA_DIR, "history.csv")
HISTORY_STEP_S = 10
HISTORY_KEEP_S = 24 * 3600
HISTORY_FLUSH_S = 60


def _num(text):
    try:
        return float(text) if text else None
    except ValueError:
        return None


class History:
    """(time, cpu %, cpu °C or None, gpu °C or None) rows for the last 24 hours."""

    def __init__(self, path=HISTORY_PATH):
        self.path = path
        self.rows = []
        self.pending = []  # added since the last write to the file
        self.last_add = 0.0
        self.last_flush = self.last_trim = time.time()
        self._load()

    def _load(self):
        now = time.time()
        try:
            with open(self.path, encoding="utf-8") as f:
                for line in f:
                    parts = line.strip().split(",")
                    if len(parts) != 4:
                        continue
                    t, cpu = _num(parts[0]), _num(parts[1])
                    if t is None or cpu is None or not now - HISTORY_KEEP_S <= t <= now + 60:
                        continue
                    self.rows.append((t, cpu, _num(parts[2]), _num(parts[3])))
        except FileNotFoundError:
            return
        except OSError:
            log_error("reading history")
        self.rows.sort()
        self._rewrite()  # drops what is older than a day

    @staticmethod
    def _line(row):
        t, cpu, t_cpu, t_gpu = row
        opt = lambda v: "" if v is None else f"{v:.1f}"
        return f"{t:.0f},{cpu:.1f},{opt(t_cpu)},{opt(t_gpu)}\n"

    def _rewrite(self):
        tmp = self.path + ".tmp"
        try:
            with open(tmp, "w", encoding="utf-8") as f:
                f.writelines(self._line(r) for r in self.rows)
            os.replace(tmp, self.path)
        except OSError:
            log_error("writing history")

    def add(self, cpu, t_cpu, t_gpu, now=None):
        now = time.time() if now is None else now
        if now - self.last_add < HISTORY_STEP_S:
            return
        self.last_add = now
        row = (now, cpu, t_cpu, t_gpu)
        self.rows.append(row)
        self.pending.append(row)
        if now - self.last_flush >= HISTORY_FLUSH_S:
            self.flush(now)

    def flush(self, now=None):
        """Append new points to the file; once an hour rewrite it without old ones."""
        now = time.time() if now is None else now
        self.last_flush = now
        cutoff = now - HISTORY_KEEP_S
        if self.rows and self.rows[0][0] < cutoff:
            self.rows = [r for r in self.rows if r[0] >= cutoff]
        if now - self.last_trim >= 3600:
            self.last_trim = now
            self.pending = []
            self._rewrite()
            return
        if self.pending:
            try:
                with open(self.path, "a", encoding="utf-8") as f:
                    f.writelines(self._line(r) for r in self.pending)
            except OSError:
                log_error("writing history")
            self.pending = []

    def window(self, seconds, now=None):
        now = time.time() if now is None else now
        return [r for r in self.rows if r[0] >= now - seconds]


def history_buckets(rows, start, end, n, index):
    """Average and peak of column `index` in n equal time slices (None = no data)."""
    sums, counts, peaks = [0.0] * n, [0] * n, [None] * n
    span = (end - start) / n
    for row in rows:
        value = row[index]
        if value is None or row[0] < start:
            continue
        i = min(n - 1, int((row[0] - start) / span))
        sums[i] += value
        counts[i] += 1
        peaks[i] = value if peaks[i] is None else max(peaks[i], value)
    return [(sums[i] / counts[i], peaks[i]) if counts[i] else None for i in range(n)]


# ---------- ping & processes ----------

PING_HOST = "1.1.1.1"  # Cloudflare DNS: fast and always up (the default)
PING_PRESETS = (("1.1.1.1", "Cloudflare"), ("8.8.8.8", "Google"), ("9.9.9.9", "Quad9"))
_HOST_RE = re.compile(r"(?=.{1,253}$)[A-Za-z0-9](?:[A-Za-z0-9-]{0,62}[A-Za-z0-9])?"
                      r"(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,62}[A-Za-z0-9])?)*")


def clean_host(text):
    """'https://Eu.Battle.net/x' -> 'eu.battle.net'; None if it isn't a host name or IPv4."""
    text = (text or "").strip().lower()
    text = re.sub(r"^[a-z]+://", "", text).split("/")[0].split(":")[0].rstrip(".")
    return text if _HOST_RE.fullmatch(text) else None
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
            host = self.host
            try:
                addr = struct.unpack("<I", socket.inet_aton(socket.gethostbyname(host)))[0]
                got = icmp.IcmpSendEcho(handle, addr, payload, 10, None, reply, 256, 1000)
                # ICMP_ECHO_REPLY: Address, Status, RoundTripTime (ULONGs)
                _, status, rtt = struct.unpack_from("<III", reply.raw)
                ms = rtt if got and status == 0 else None
            except OSError:  # includes "no such host"
                ms = None
            if host == self.host:  # not switched to another address meanwhile
                self.ms, self.ready = ms, True
            time.sleep(PING_EVERY_S)

    def set_host(self, host):
        self.host = host
        self.ms, self.ready = None, False  # shows "…" until the new address answers


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
        self.update_box = (0, 0, 0, 0)
        self.temps_box = (0, 0, 0, 0)

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
        if data.get("update"):
            secs.append((30, self._draw_update))
        if data.get("procs") is not None:
            secs.append((120, self._draw_procs))
        if "cores" not in hidden and data.get("cores"):
            secs.append((38, self._draw_cores))
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
        if "battery" not in hidden and data.get("battery_wear") is not None:
            secs.append((38, self._draw_battery))
        if "fans" not in hidden and data.get("fans"):
            secs.append((24, self._draw_fans))
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
        row1 = [chip(tr("Память"), f"{data['ram_pct']:.0f}%", scale(data["ram_pct"] / 100)),
                chip(tr("Видео"), None if load is None else f"{load:.0f}%",
                     scale((load or 0) / 100)),
                chip(tr("Диск"), None if disk_pct is None else f"{disk_pct:.0f}%",
                     scale((disk_pct or 0) / 100))]
        cpu_t, gpu_t = data["temp_cpu"], data["temp_gpu"]
        shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
        g = cpu_t if shared else gpu_t
        ping, ready = data.get("ping"), data.get("ping_ready")
        row2 = [chip("CPU", None if cpu_t is None else f"{cpu_t:.0f}°", scale(((cpu_t or 30) - 30) / 70)),
                chip("GPU", None if g is None else f"{g:.0f}°", scale(((g or 30) - 30) / 70), shared),
                chip(tr("Пинг"), None if not ready else (tr("нет") if ping is None else tr("{v} мс", v=ping)),
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
        scale = theme_scale(theme)
        P, Wl = self.PAD, self.WIDTH
        self.update_box = self.temps_box = (0, 0, 0, 0)
        if compact:
            data = dict(data, procs=None, update=None)
        H = self.layout_height(data, compact, hidden)
        card = self._card(H)

        layer = Image.new("RGBA", (self.W * SS, H * SS), (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        texts = []  # deferred text: (x, y, text, font, fill, anchor)

        # header; after one click on the cross it asks for a second one
        if close_armed:
            texts.append((P, 28, tr("Нажми ещё раз — закрыть"), self.f_value, CLOSE_HOVER, "ls"))
            self._rrect(d, Wl - P - 13, 14, Wl - P + 5, 32, 5, lerp(CARD, CLOSE_HOVER, 0.25))
        else:
            texts.append((P, 28, tr("Система"), self.f_title, TEXT, "ls"))
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
        texts.append((P, hero_base + 18, tr("Процессор"), self.f_small, DIM, "ls"))
        hx0 = P + 82
        self._histogram(d, hx0, 50, Wl - P, 100, data["cpu_hist"], scale)
        extra = cpu_extra_text(data)
        if extra:
            texts.append(("rich", Wl - P, hero_base + 18, [(extra, self.f_small, DIM)]))
        if compact:
            self.hero_box = (0, 0, 0, 0)  # no process list in compact mode
            self._compact_rows(texts, data, scale, HERO_H)
            return self._compose(card, layer, texts)
        chev_x = P + self.f_small.getlength(tr("Процессор") + " ") / self.s
        texts.append((chev_x, hero_base + 18, "▴" if procs is not None else "▾", self.f_sym, DIM, "ls"))
        self.hero_box = (self.px(P - 4), self.px(42), self.px(Wl - P + 4), self.px(118))

        y = HERO_H
        for h, draw in self._sections(data, hidden):
            if draw is not None:
                draw(d, texts, y, data, scale)
            y += h
        return self._compose(card, layer, texts)

    # --- sections: draw(d, texts, y, data, scale) ---
    def _draw_update(self, d, texts, y, data, scale):
        """'Вышла версия 1.2 — обновить' pill; a click updates (or opens the page)."""
        P, Wl = self.PAD, self.WIDTH
        version = data["update"][0]
        state = data.get("update_state")  # None, ("download", 0..1) or ("install", None)
        accent = scale(0.0)
        self._rrect(d, P - 6, y - 2, Wl - P + 6, y + 22, 8, lerp(CARD, accent, 0.18))
        if state and state[0] == "download":
            done = P - 6 + (Wl - 2 * P + 12) * max(0.04, state[1])
            self._rrect(d, P - 6, y - 2, done, y + 22, 8, lerp(CARD, accent, 0.4))
            right = [(tr("скачиваю {p}%", p=int(state[1] * 100)), self.f_small, TEXT)]
        elif state:
            right = [(tr("устанавливаю…"), self.f_small, TEXT)]
        elif data.get("update_auto"):
            right = [(tr("обновить") + " ", self.f_small, accent), ("↻", self.f_sym, accent)]
        else:
            right = [(tr("скачать") + " ", self.f_small, accent), ("↗", self.f_sym, accent)]
        texts.append((P, y + 14, tr("Вышла версия {v}", v=version), self.f_value, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, right))
        self.update_box = (self.px(P - 6), self.px(y - 2), self.px(Wl - P + 6), self.px(y + 22))

    def _draw_procs(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        procs = data["procs"]
        y0, y1 = y - 4, y + 108
        self._rrect(d, P - 6, y0, Wl - P + 6, y1, 8, (34, 40, 52))
        col_mem, col_cpu = Wl - P, Wl - P - 58
        texts.append((P, y0 + 17, tr("Программа"), self.f_small, FAINT, "ls"))
        texts.append(("rich", col_cpu, y0 + 17, [("CPU", self.f_small, FAINT)]))
        texts.append(("rich", col_mem, y0 + 17, [(tr("Память"), self.f_small, FAINT)]))
        name_w = self.px(col_cpu - 42 - P)
        for i, (name, cpu_p, mem) in enumerate(procs):
            by = y0 + 36 + i * 17
            texts.append((P, by, ellipsize(name, self.f_small, name_w), self.f_small, TEXT, "ls"))
            cpu_txt = dec(f"{cpu_p:.1f}") if cpu_p < 10 else f"{cpu_p:.0f}"
            texts.append(("rich", col_cpu, by, [(f"{cpu_txt}%", self.f_value,
                                                 scale(min(1.0, cpu_p / 50)))]))
            texts.append(("rich", col_mem, by, [(fmt_mem(mem), self.f_small, DIM)]))
        if not procs:
            texts.append((P, y0 + 36, tr("собираю данные…"), self.f_small, FAINT, "ls"))

    def _draw_cores(self, d, texts, y, data, scale):
        """One bar per logical processor: shows a single overloaded core that the
        average hides (a game or a stuck program often loads just one)."""
        P, Wl = self.PAD, self.WIDTH
        cores = data["cores"]
        top = max(cores)
        texts.append((P, y + 14, tr("Ядра"), self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [(tr("макс") + " ", self.f_small, DIM),
                                               (f"{top:.0f}%", self.f_value, scale(top / 100))]))
        n = len(cores)
        gap = 2 if n <= 16 else 1
        bar_w = (Wl - 2 * P - gap * (n - 1)) / n
        y0, y1 = y + 20, y + 32
        for i, load in enumerate(cores):
            bx = P + i * (bar_w + gap)
            self._rrect(d, bx, y0, bx + bar_w, y1, 1.2, TRACK)
            h = (y1 - y0) * max(0.0, min(100.0, load)) / 100
            if h >= 0.6:
                self._rrect(d, bx, y1 - h, bx + bar_w, y1, 1.2, scale(load / 100))

    def _draw_ram(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        texts.append((P, y + 14, tr("Память"), self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [
            (tr("{used} из {total} ГБ", used=fmt_gb(data['ram_used']),
                total=fmt_gb(data['ram_total'])) + "   ", self.f_small, DIM),
            (f"{data['ram_pct']:.0f}%", self.f_value, TEXT)]))
        self._segment_meter(d, P, y + 22, Wl - P, data["ram_pct"], scale)

    def _draw_gpu(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        load, gname = data.get("gpu_load"), data.get("gpu_name")
        texts.append((P, y + 14, tr("Видеокарта"), self.f_label, TEXT, "ls"))
        if load is not None:
            value = f"{load:.0f}%"
            room = (self.px(Wl - P) - self.px(P) - self.f_label.getlength(tr("Видеокарта"))
                    - self.f_value.getlength(value) - self.px(24))
            parts = ([(ellipsize(gname, self.f_small, room) + "   ", self.f_small, DIM)] if gname else [])
            texts.append(("rich", Wl - P, y + 14, parts + [(value, self.f_value, TEXT)]))
        else:
            texts.append(("rich", Wl - P, y + 14, [(tr("нет данных"), self.f_small, FAINT)]))
        self._segment_meter(d, P, y + 22, Wl - P, load or 0, scale)

    def _draw_disk(self, d, texts, y, disk, scale):
        P, Wl = self.PAD, self.WIDTH
        name, used, total, pct = disk
        texts.append((P, y + 14, tr("Диск {name}", name=name), self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [
            (tr("{used} из {total} ГБ", used=fmt_gb(used), total=fmt_gb(total)) + "   ",
             self.f_small, DIM),
            (f"{pct:.0f}%", self.f_value, TEXT)]))
        self._segment_meter(d, P, y + 22, Wl - P, pct, scale)

    def _draw_temps(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        cpu_t, gpu_t, ssd_t = data["temp_cpu"], data["temp_gpu"], data.get("temp_ssd")
        shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
        online = self._temps_online(data)
        self.temps_box = (self.px(P - 4), self.px(y), self.px(Wl - P + 4), self.px(y + 58))
        texts.append((P, y + 14, tr("Температура"), self.f_label, TEXT, "ls"))
        if not online:
            texts.append(("rich", Wl - P, y + 14, [(tr("нет данных"), self.f_small, FAINT)]))
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
            hint = ellipsize(data.get("sensor_hint") or tr("Нет данных о температуре"), self.f_small,
                             self.px(Wl - 2 * P))
            texts.append((P, y + 8, hint, self.f_small, FAINT, "ls"))

    def _draw_battery(self, d, texts, y, data, scale):
        """Wear (how much capacity is lost since new) and, on battery, the power draw."""
        P, Wl = self.PAD, self.WIDTH
        wear = data["battery_wear"]
        batt = data.get("battery")
        rate = data.get("battery_rate")
        if batt is not None and not batt[1] and rate:
            info = tr("расход {v} Вт", v=dec(f"{rate:.1f}")) + "   "
        else:
            info = tr("{full} из {design} Вт·ч", full=fmt_wh(data['battery_full']),
                      design=fmt_wh(data['battery_design'])) + "   "
        texts.append((P, y + 14, tr("Батарея"), self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [
            (info, self.f_small, DIM), (tr("износ") + " ", self.f_small, DIM),
            (f"{wear:.0f}%", self.f_value, scale(min(1.0, wear / 40)))]))
        self._segment_meter(d, P, y + 22, Wl - P, wear, scale)

    def _draw_fans(self, d, texts, y, data, scale):
        """Fan speeds (only when the sensors report any), coloured by how fast they spin."""
        P, Wl = self.PAD, self.WIDTH
        texts.append((P, y + 14, tr("Вентиляторы"), self.f_label, TEXT, "ls"))
        parts = []
        for i, rpm in enumerate(data["fans"]):
            if i:
                parts.append(("  ·  ", self.f_small, FAINT))
            parts.append((f"{rpm:.0f}", self.f_value, scale(min(1.0, rpm / 5000))))
        parts.append((" " + tr("об/мин"), self.f_small, DIM))
        texts.append(("rich", Wl - P, y + 14, parts))

    def _draw_divider(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        d.rectangle(self._ss(P, y + 6, Wl - P, y + 6 + 1 / self.s), fill=TRACK)

    def _draw_net(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        down_c, up_c = scale(0.0), scale(0.75)
        texts.append((P, y + 12, tr("Сеть"), self.f_label, TEXT, "ls"))
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
        texts.append((P, y + 12, tr("Пинг"), self.f_label, TEXT, "ls"))
        ping = data.get("ping")
        if not data.get("ping_ready"):
            parts = [("…", self.f_small, FAINT)]
        elif ping is None:
            parts = [(tr("нет ответа"), self.f_value, ping_color(scale, None))]
        else:
            host = data.get("ping_host") or PING_HOST
            parts = [(tr("до {host}", host=ellipsize(host, self.f_small, self.px(120))) + "   ",
                      self.f_small, FAINT),
                     (tr("{v} мс", v=ping), self.f_value, ping_color(scale, ping))]
        texts.append(("rich", Wl - P, y + 12, parts))

    def _draw_disk_io(self, d, texts, y, data, scale):
        P, Wl = self.PAD, self.WIDTH
        texts.append((P, y + 12, tr("Диск"), self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 12, [
            (tr("чтение") + " ", self.f_small, DIM), (fmt_speed(data["io_read"]), self.f_small, TEXT),
            ("   " + tr("запись") + " ", self.f_small, DIM),
            (fmt_speed(data["io_write"]), self.f_small, TEXT)]))

    def _draw_footer(self, d, texts, y, data, scale):
        texts.append((self.PAD, y + 18, tr("Без перезагрузки {t}", t=fmt_uptime(data['uptime'])),
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
        px = self.px = getattr(widget, "ui_renderer", widget.renderer).px
        self.gap = px(6)
        self.font = load_font(px(12))
        self.font_val = load_font(px(12), "SemiBold")
        self.font_sym = load_symbol_font(px(12))
        self.light = taskbar_uses_light_theme()
        # background ~ taskbar colour: text edges blend into it, then it is keyed out
        self.key = (232, 232, 234) if self.light else (22, 22, 24)
        key_hex = "#%02x%02x%02x" % self.key

        self.col_gap = px(14)
        self.measure()

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

    def measure(self):
        """Column widths for the widest values (again after a language change)."""
        f, fv, fs = self.font, self.font_val, self.font_sym
        speed_w = max(f.getlength(fmt_speed(1023 * 1024)), f.getlength(fmt_speed(99.9 * 1024**2)))
        self.col_a = max(f.getlength("CPU "), f.getlength("RAM ")) + fv.getlength("100%")
        self.col_b = fs.getlength("↓ ") + speed_w
        self.col_c = (max(f.getlength("CPU "), f.getlength("GPU ")) + fs.getlength("≈")
                      + fv.getlength("100°"))
        self.col_d = max(f.getlength(tr("пинг")), fv.getlength(tr("{v} мс", v=999)))

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
        scale = theme_scale(theme)
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
        d.text((x_d + self.col_d, rows[0]), tr("пинг"), font=f, fill=label_c, anchor="rm")
        if not data.get("ping_ready"):
            value, color = "…", label_c
        elif ping is None:
            value, color = tr("нет"), tone(1.0)
        else:
            value, color = tr("{v} мс", v=ping), tone(0.05 if ping < 50 else 0.55 if ping < 100 else 0.95)
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
        self.px = getattr(widget, "ui_renderer", widget.renderer).px
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
        self.view.bind("<ButtonRelease-1>", self._clicked)
        self.job = None
        self.on_click = None

    def _clicked(self, event):
        action, self.on_click = self.on_click, None
        self.hide()
        if action is not None:
            action()

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

    def show(self, title, body, color, on_click=None):
        """on_click: what a click on the card does (besides closing it)."""
        self.on_click = on_click
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
        scale = theme_scale(theme)

        def temp(t, approx=False):
            if t is None:
                return []
            c = scale((t - 30) / 70)
            return [("   ", f, DIM)] + ([("≈", fs, c)] if approx else []) + [(f"{t:.0f}°", fv, c)]

        cpu_t, gpu_t = data["temp_cpu"], data["temp_gpu"]
        shared = bool(data.get("gpu_shared")) and gpu_t is None and cpu_t is not None
        rows = [(tr("Процессор"), [(f"{data['cpu']:.0f}%", fv, scale(data["cpu"] / 100))] + temp(cpu_t)),
                (tr("Память"), [(tr("{used} из {total} ГБ", used=fmt_gb(data['ram_used']),
                                    total=fmt_gb(data['ram_total'])) + "   ", f, DIM),
                            (f"{data['ram_pct']:.0f}%", fv, scale(data["ram_pct"] / 100))])]
        if data.get("gpu_load") is not None:
            rows.append((tr("Видеокарта"),
                         [(f"{data['gpu_load']:.0f}%", fv, scale(data["gpu_load"] / 100))]
                         + temp(cpu_t if shared else gpu_t, approx=shared)))
        for name, _, _, pct in data["disks"][:1]:
            rows.append((tr("Диск {name}", name=name), [(f"{pct:.0f}%", fv, TEXT)] + temp(data.get("temp_ssd"))))
        rows.append((tr("Сеть"), [("↓ ", fs, scale(0.0)), (fmt_speed(data["net_down"]), f, TEXT),
                              ("   ↑ ", fs, scale(0.75)), (fmt_speed(data["net_up"]), f, TEXT)]))
        ping, ready = data.get("ping"), data.get("ping_ready")
        ping_text = "…" if not ready else tr("нет ответа") if ping is None else tr("{v} мс", v=ping)
        rows.append((tr("Пинг"), [(ping_text, fv, ping_color(scale, ping) if ready else FAINT)]))
        batt = data.get("battery")
        if batt is not None:
            level, plugged = batt
            left = data.get("battery_left")
            rows.append((tr("Батарея"), ([(f"{fmt_left(left)}   ", f, DIM)] if left else [])
                         + [(f"{level:.0f}%", fv, scale(1 - level / 100) if not plugged else scale(0))]
                         + ([(" ⚡", fs, DIM)] if plugged else [])))
        top = top_processes(1)
        if top:
            name, cpu_p, _ = top[0]
            rows.append((tr("Больше всех"), [(ellipsize(name, f, px(110)) + "  ", f, TEXT),
                                         (f"{cpu_p:.0f}%", fv, scale(min(1.0, cpu_p / 50)))]))

        W = px(self.WIDTH)
        H = px(40 + 20 * len(rows) + 22)
        img = card_image(px, W, H)
        d = ImageDraw.Draw(img)
        d.text((px(16), px(26)), tr("Система"), font=self.f_title, fill=TEXT, anchor="ls")
        room = W - px(42) - self.f_title.getlength(tr("Система"))
        d.text((W - px(16), px(26)), ellipsize(tr("без перезагрузки {t}", t=fmt_uptime(data['uptime'])),
                                               self.f_small, room),
               font=self.f_small, fill=FAINT, anchor="rs")
        for i, (label, parts) in enumerate(rows):
            y = px(50 + 20 * i)
            d.text((px(16), y), label, font=f, fill=DIM, anchor="ls")
            x = W - px(16) - sum(font.getlength(t) for t, font, _ in parts)
            for t, font, color in parts:
                d.text((x, y), t, font=font, fill=color, anchor="ls")
                x += font.getlength(t)
        d.text((px(16), H - px(14)), tr("Нажми, чтобы открыть виджет"), font=self.f_small,
               fill=FAINT, anchor="ls")
        return img

    def show_above(self, rect, data, theme):
        img = self.render(data, theme)
        left, top, right, _ = rect
        screen_w = ctypes.windll.user32.GetSystemMetrics(0)
        x = max(0, min(right - img.width, screen_w - img.width - self.px(8)))
        self._show_image(img, x, top - img.height - self.px(8))


class HistoryCard(PopupCard):
    """CPU temperature and load for the last hour or day. The 'час / сутки'
    switch changes the period, a click anywhere else closes the card."""

    WIDTH = 380
    HEIGHT = 262
    PERIODS = (("час", 3600), ("сутки", 24 * 3600))
    COLUMNS = 120

    def __init__(self, widget):
        super().__init__(widget)
        px = self.px
        self.f_title = load_font(px(14), "SemiBold")
        self.f = load_font(px(12))
        self.fv = load_font(px(12), "SemiBold")
        self.f_small = load_font(px(11))
        self.period = 0
        self.tab_boxes = []
        self.pos = (0, 0)
        self.view.bind("<ButtonRelease-1>", self._click)

    def _click(self, event):
        for i, (x0, y0, x1, y1) in enumerate(self.tab_boxes):
            if x0 <= event.x <= x1 and y0 <= event.y <= y1:
                self.period = i
                self.refresh()
                return
        self.hide()

    def _chart(self, d, x0, y0, x1, y1, cols, lo, hi, scale):
        """Bars of the average per time slice, coloured on the theme scale."""
        px = self.px
        pitch = (x1 - x0) / len(cols)
        for i, col in enumerate(cols):
            # whole device pixels, every bar the same width with a 1 px gap
            bx0, bx1 = px(x0 + i * pitch), px(x0 + (i + 1) * pitch) - 2
            d.rectangle((bx0, px(y0), bx1, px(y1)), fill=HIST_TRACK)
            if col is None:
                continue
            t = max(0.0, min(1.0, (col[0] - lo) / (hi - lo)))
            top = y1 - max(1.0, (y1 - y0) * t)
            d.rectangle((bx0, px(top), bx1, px(y1)), fill=scale(t))

    def _summary(self, rows, index, unit, lo, hi, scale):
        values = [r[index] for r in rows if r[index] is not None]
        if not values:
            return None
        peak, avg = max(values), sum(values) / len(values)
        color = lambda v: scale(max(0.0, min(1.0, (v - lo) / (hi - lo))))
        return [(tr("макс") + " ", self.f_small, DIM), (f"{peak:.0f}{unit}", self.fv, color(peak)),
                ("   " + tr("средн.") + " ", self.f_small, DIM),
                (f"{avg:.0f}{unit}", self.fv, color(avg))]

    def render(self, rows, theme, now):
        px = self.px
        scale = theme_scale(theme)
        name, seconds = self.PERIODS[self.period]
        W, H = px(self.WIDTH), px(self.HEIGHT)
        img = card_image(px, W, H)
        d = ImageDraw.Draw(img)
        L, R = 16, self.WIDTH - 16
        rows = [r for r in rows if r[0] >= now - seconds]

        d.text((px(L), px(28)), tr("История"), font=self.f_title, fill=TEXT, anchor="ls")
        # period switch in the top-right corner; tab_boxes[i] = click area of PERIODS[i]
        self.tab_boxes = [None] * len(self.PERIODS)
        x = R
        for i in reversed(range(len(self.PERIODS))):
            label = tr(self.PERIODS[i][0])
            w = self.f.getlength(label) / self.px(1) + 16
            tx0, tx1 = x - w, x
            x -= w + 4
            active = i == self.period
            if active:
                d.rounded_rectangle((px(tx0), px(12), px(tx1), px(33)), radius=px(7),
                                    fill=lerp(CARD, scale(0.0), 0.25))
            d.text((px((tx0 + tx1) / 2), px(27)), label, font=self.f,
                   fill=TEXT if active else DIM, anchor="ms")
            self.tab_boxes[i] = (px(tx0), px(8), px(tx1), px(37))

        start = now - seconds
        for label, index, unit, lo, hi, cy0, cy1, empty in (
                ("Температура CPU", 2, "°", 30, 100, 60, 116,
                 "нет данных: нужны права администратора"),
                ("Загрузка CPU", 1, "%", 0, 100, 152, 208, "нет данных")):
            d.text((px(L), px(cy0 - 8)), tr(label), font=self.f, fill=TEXT, anchor="ls")
            summary = self._summary(rows, index, unit, lo, hi, scale)
            if summary:
                xr = px(R)
                for text, font, color in reversed(summary):
                    xr -= font.getlength(text)
                    d.text((xr, px(cy0 - 8)), text, font=font, fill=color, anchor="ls")
            cols = history_buckets(rows, start, now, self.COLUMNS, index)
            self._chart(d, L, cy0, R, cy1, cols, lo, hi, scale)
            if not summary:
                d.text((px((L + R) / 2), px((cy0 + cy1) / 2 + 4)), tr(empty), font=self.f_small,
                       fill=FAINT, anchor="ms")

        fmt = (lambda t: time.strftime("%H:%M", time.localtime(t)))
        d.text((px(L), px(226)), fmt(start), font=self.f_small, fill=FAINT, anchor="ls")
        d.text((px((L + R) / 2), px(226)), fmt(start + seconds / 2), font=self.f_small,
               fill=FAINT, anchor="ms")
        d.text((px(R), px(226)), tr("сейчас"), font=self.f_small, fill=FAINT, anchor="rs")
        d.text((px(L), px(250)), tr("Нажми, чтобы закрыть"), font=self.f_small, fill=FAINT, anchor="ls")
        return img

    def show_near(self, rect):
        """Beside the widget: on the left if it fits, else on the right."""
        img = self.render(self.widget.history.rows, self.widget.cfg["theme"], time.time())
        wl, wt, wr, wb = work_area()
        left, top, right, _ = rect
        gap = self.px(10)
        x = left - img.width - gap if left - img.width - gap >= wl else right + gap
        x = max(wl, min(x, wr - img.width))
        y = max(wt, min(top, wb - img.height))
        self.pos = (x, y)
        self._show_image(img, x, y)

    def refresh(self):
        if self.visible:
            img = self.render(self.widget.history.rows, self.widget.cfg["theme"], time.time())
            self._show_image(img, *self.pos)


class FpsOverlay:
    """'FPS 60' in the top-left corner of the game window: topmost, clicks go
    through it to the game, and it never takes the focus."""

    OFFSCREEN = "1x1+-9000+-9000"

    def __init__(self, widget):
        self.px = px = getattr(widget, "ui_renderer", widget.renderer).px
        self.f_label = load_font(px(11), "SemiBold")
        self.f_value = load_font(px(17), "SemiBold Condensed")
        self.top = top = tk.Toplevel(widget.root)
        top.overrideredirect(True)
        top.configure(bg=KEY_HEX)
        top.attributes("-transparentcolor", KEY_HEX)
        top.attributes("-topmost", True)
        self.view = tk.Label(top, bg=KEY_HEX, bd=0, highlightthickness=0)
        self.view.pack()
        top.geometry(self.OFFSCREEN)
        top.update_idletasks()
        hwnd = int(top.wm_frame(), 16)
        u = ctypes.windll.user32
        u.GetWindowLongPtrW.restype = ctypes.c_ssize_t
        u.GetWindowLongPtrW.argtypes = [_w.HWND, ctypes.c_int]
        ex = u.GetWindowLongPtrW(hwnd, -20)  # GWL_EXSTYLE
        # TRANSPARENT: clicks pass through; TOOLWINDOW: no taskbar button; NOACTIVATE: no focus
        _user32.SetWindowLongPtrW(hwnd, -20, ex | 0x20 | 0x80000 | 0x80 | 0x08000000)
        self.hwnd = hwnd
        self.photo = None
        self.visible = False
        self.shown = None  # (text, colour, x, y) on screen now

    GRAPH_W = 52  # frame-time graph, logical px

    @staticmethod
    def fps_color(scale, fps):
        return scale(0.05 if fps >= 55 else 0.55 if fps >= 30 else 0.95)

    def render(self, stats, scale, ping=None):
        """'FPS 60  low 48', a graph of the last frame times (spikes = stutter) and the ping."""
        px = self.px
        fps, low, times = stats
        parts = [("FPS ", self.f_label, DIM), (f"{fps:.0f}", self.f_value, self.fps_color(scale, fps))]
        if low is not None:
            parts += [("   low ", self.f_label, DIM), (f"{low:.0f}", self.f_value, self.fps_color(scale, low))]
        tail = []
        if ping is not None:
            tail = [(tr("пинг") + " ", self.f_label, DIM), (str(ping), self.f_value, ping_color(scale, ping)),
                    (" " + tr("{v} мс", v="").strip(), self.f_label, DIM)]
        tail_w = sum(font.getlength(t) for t, font, _ in tail)
        text_w = sum(font.getlength(t) for t, font, _ in parts)
        graph = bool(times)
        w = int(text_w + px(22) + (px(self.GRAPH_W + 10) if graph else 0)
                + (tail_w + px(12) if tail else 0))
        h = px(30)
        img = card_image(px, w, h)
        d = ImageDraw.Draw(img)
        x = px(11)
        for t, font, color in parts:
            d.text((x, px(21) if font is self.f_value else px(20)), t, font=font, fill=color, anchor="ls")
            x += font.getlength(t)
        if graph:
            # bars of frame time: the median is a third of the height, a spike reaches the top
            med = sorted(times)[len(times) // 2]
            top_ms = max(med * 3, 1.0)
            gx0, gy0, gy1 = x + px(10), px(8), px(22)
            bar = px(self.GRAPH_W) / 48
            for i, ms in enumerate(times[-48:]):
                hgt = max(1, (gy1 - gy0) * min(1.0, ms / top_ms))
                spike = ms > med * 1.8
                d.rectangle((gx0 + i * bar, gy1 - hgt, gx0 + i * bar + max(1, bar - 1), gy1),
                            fill=scale(0.95) if spike else scale(0.05))
            x = gx0 + px(self.GRAPH_W)
        x += px(12)
        for t, font, color in tail:
            d.text((x, px(21) if font is self.f_value else px(20)), t, font=font, fill=color, anchor="ls")
            x += font.getlength(t)
        return img

    def show(self, stats, rect, scale, ping=None):
        x, y = rect[0] + self.px(12), rect[1] + self.px(12)
        fps, low, times = stats
        key = (round(fps), None if low is None else round(low), tuple(round(t) for t in times[-48:]),
               ping, x, y)
        if self.shown == key:
            return
        self.shown = key
        img = self.render(stats, scale, ping)
        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        self.top.geometry(f"{img.width}x{img.height}+{x}+{y}")
        self.top.lift()
        self.visible = True

    def hide(self):
        if self.visible:
            self.top.geometry(self.OFFSCREEN)
            self.visible = False
            self.shown = None


def window_pid(hwnd):
    pid = _w.DWORD()
    ctypes.windll.user32.GetWindowThreadProcessId(_w.HWND(hwnd), ctypes.byref(pid))
    return pid.value


_names = {}


def process_name(pid):
    """'civilizationvi_dx12' for a pid (cached; '' if it can't be read)."""
    if pid not in _names:
        try:
            _names[pid] = psutil.Process(pid).name().lower().removesuffix(".exe")
        except (psutil.Error, OSError):
            _names[pid] = ""
        if len(_names) > 500:
            _names.clear()
    return _names[pid]


class SettingsWindow:
    """All settings in one window with tabs. The controls use the widget's own
    Tk variables and handlers, so the menu, the window and the config agree."""

    TABS = ("Общие", "Вид", "Показывать", "Предупреждения", "Сеть и игры")

    def __init__(self, widget, tab=0):
        from tkinter import ttk
        self.w = w = widget
        self.top = top = tk.Toplevel(w.root)
        top.title(tr("Настройки") + " — PC Monitor")
        top.resizable(False, False)
        top.protocol("WM_DELETE_WINDOW", self.close)
        top.bind("<Escape>", lambda e: self.close())
        self.book = book = ttk.Notebook(top)
        book.pack(fill="both", expand=True, padx=10, pady=(10, 0))
        self.pages = []
        for title in self.TABS:
            frame = ttk.Frame(book, padding=12)
            book.add(frame, text=tr(title))
            self.pages.append(frame)
        self._general(self.pages[0])
        self._look(self.pages[1])
        self._sections(self.pages[2])
        self._alerts(self.pages[3])
        self._network(self.pages[4])
        bottom = ttk.Frame(top, padding=(10, 8))
        bottom.pack(fill="x")
        ttk.Label(bottom, text=f"PC Monitor {APP_VERSION}", foreground="#888").pack(side="left")
        ttk.Button(bottom, text=tr("Закрыть"), command=self.close).pack(side="right")
        book.select(tab)
        self.sync()
        top.update_idletasks()
        # next to the widget's screen corner, fully on screen
        wl, wt, wr, wb = work_area()
        x = min(max(wl, w.root.winfo_rootx() - top.winfo_width() - 12), wr - top.winfo_width())
        y = min(max(wt, w.root.winfo_rooty()), wb - top.winfo_height())
        top.geometry(f"+{x}+{y}")
        top.lift()
        top.focus_force()

    # --- small builders ---
    @staticmethod
    def _check(parent, text, var, command):
        from tkinter import ttk
        b = ttk.Checkbutton(parent, text=text, variable=var, command=command)
        b.pack(anchor="w", pady=2)
        return b

    @staticmethod
    def _radios(parent, title, var, items, command):
        from tkinter import ttk
        box = ttk.LabelFrame(parent, text=title, padding=(10, 4))
        box.pack(fill="x", pady=(8, 0))
        for value, label in items:
            ttk.Radiobutton(box, text=label, value=value, variable=var,
                            command=command).pack(side="left", padx=(0, 10))
        return box

    # --- tabs ---
    def _general(self, p):
        from tkinter import ttk
        w = self.w
        self._check(p, tr("Закрепить на рабочем столе"), w.pinned_var, w._toggle_pin)
        self.topmost_box = self._check(p, tr("Поверх всех окон"), w.topmost_var, w._toggle_topmost)
        self._check(p, tr("Мини-панель на панели задач"), w.taskbar_var, w._toggle_taskbar)
        self._check(p, tr("Значок в трее"), w.tray_var, w._toggle_tray)
        self._check(p, tr("Запускать вместе с Windows"), w.autostart_var, w._toggle_autostart)
        if lhm_dir():
            self._check(p, tr("Права администратора (температуры)"), w.admin_var, w._toggle_admin)
        self._check(p, tr("Проверять обновления"), w.updates_var, w._toggle_updates)
        langs = [("auto", tr("Как в Windows"))] + list(LANGS.items())
        box = ttk.LabelFrame(p, text="Язык / Language", padding=(10, 4))
        box.pack(fill="x", pady=(8, 0))
        for i, (value, label) in enumerate(langs):
            ttk.Radiobutton(box, text=label, value=value, variable=w.lang_var,
                            command=w._set_language).grid(row=i // 3, column=i % 3, sticky="w",
                                                          padx=(0, 12))
        ttk.Button(p, text=tr("В правый верхний угол"), command=w._reset_position).pack(
            anchor="w", pady=(10, 0))

    def _look(self, p):
        from tkinter import ttk
        w = self.w
        themes = [(key, tr(title)) for key, (title, _) in THEMES.items()] + [("custom", tr("Свои"))]
        box = self._radios(p, tr("Цвета"), w.theme_var, themes, w._set_theme)
        ttk.Button(box, text=tr("Выбрать свои цвета…"), command=w._pick_colors).pack(side="left")
        self._radios(p, tr("Непрозрачность"), w.alpha_var,
                     [(pct / 100, f"{pct}%") for pct in (100, 97, 90, 80, 70)], w._set_alpha)
        self._radios(p, tr("Размер"), w.size_var,
                     [(pct, f"{pct}%") for pct in CONFIG_CHOICES["size"]], w._set_size)
        ttk.Frame(p, height=6).pack()
        self._check(p, tr("Компактный режим"), w.compact_var, w._toggle_compact)
        self._radios(p, tr("В играх и видео на весь экран"), w.in_games_var,
                     [("hide", tr("прятать")), ("dim", tr("тускнеть")), ("off", tr("ничего"))],
                     w._set_in_games)

    def _sections(self, p):
        for key, title in SECTIONS.items():
            self._check(p, tr(title), self.w.section_vars[key],
                        lambda k=key: self.w._toggle_section(k))

    def _alerts(self, p):
        w = self.w
        self._check(p, tr("Показывать предупреждения"), w.alerts_var, w._toggle_alerts)
        for key, title, unit in (("alert_temp", "Перегрев CPU от", "°"),
                                 ("alert_ram", "Память занята от", "%"),
                                 ("alert_battery", "Батарея ниже", "%")):
            self._radios(p, tr(title), w.limit_vars[key],
                         [(v, f"{v}{unit}") for v in CONFIG_CHOICES[key]],
                         lambda k=key: w._set_limit(k))

    def _network(self, p):
        from tkinter import ttk
        w = self.w
        box = ttk.LabelFrame(p, text=tr("Пинг до"), padding=(10, 4))
        box.pack(fill="x")
        for host, name in PING_PRESETS:
            ttk.Radiobutton(box, text=f"{host}  ({name})", value=host, variable=w.ping_var,
                            command=lambda h=host: w._set_ping_host(h)).pack(anchor="w")
        row = ttk.Frame(box)
        row.pack(fill="x", pady=(4, 4))
        ttk.Label(row, text=tr("Свой адрес…").rstrip("…") + ":").pack(side="left")
        custom = w.cfg["ping_host"] if w.cfg["ping_host"] not in dict(PING_PRESETS) else ""
        self.host_entry = ttk.Entry(row, width=24)
        self.host_entry.insert(0, custom)
        self.host_entry.pack(side="left", padx=6)
        self.host_entry.bind("<Return>", lambda e: self._apply_host())
        ttk.Button(row, text="OK", width=4, command=self._apply_host).pack(side="left")
        ttk.Frame(p, height=8).pack()
        self._check(p, tr("FPS в играх"), w.fps_var, w._toggle_fps)

    def _apply_host(self):
        from tkinter import messagebox
        text = self.host_entry.get()
        host = clean_host(text)
        if host is None:
            messagebox.showwarning(tr("Пинг до"), tr("«{text}» не похоже на адрес.", text=text),
                                   parent=self.top)
            return
        self.w._set_ping_host(host)

    # --- state ---
    def sync(self):
        """'Always on top' is meaningless while pinned to the desktop."""
        if self.alive():
            self.topmost_box.state(["disabled"] if self.w.cfg["pinned"] else ["!disabled"])

    def alive(self):
        try:
            return bool(self.top.winfo_exists())
        except tk.TclError:
            return False

    def current_tab(self):
        try:
            return self.book.index(self.book.select())
        except tk.TclError:
            return 0

    def close(self):
        if self.alive():
            self.top.destroy()


# ---------- window ----------

class Widget:
    def __init__(self):
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(1)
        except (AttributeError, OSError):
            pass

        self.cfg = load_config()
        set_language(self.cfg["lang"])
        custom_stops[:] = self.cfg["custom_colors"]
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
        self.dpi_scale = root.winfo_fpixels("1i") / 96
        self.ui_renderer = Renderer(self.dpi_scale)  # cards, panel, overlay: Windows' scale
        self.renderer = Renderer(self.dpi_scale * self.cfg["size"] / 100)  # the widget: + its size
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
        psutil.cpu_percent(interval=None, percpu=True)
        self.last_net = psutil.net_io_counters()
        self.last_disk = psutil.disk_io_counters()
        self.last_t = time.monotonic()
        self.cpu_hist = []
        self.net_hist = []  # (down, up) bytes/s for the network graph
        self.partitions = scan_partitions()
        self.partitions_t = time.monotonic()
        self.data = None

        self.history = History()
        self.lhm = LhmReader()
        self.lhm.start()
        self.ping = PingReader(self.cfg["ping_host"])
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
        self.updates = UpdateChecker(lambda: bool(self.cfg["check_updates"]))
        self.updates.start()
        self.manual_check = None  # checks count when "Проверить сейчас" was clicked
        self.updater = None       # a running Updater
        self.info = InfoCard(self) if _user32 is not None else None
        self.history_card = HistoryCard(self) if _user32 is not None else None
        self.visible = True

        self.tray = None
        if _user32 is not None and self.cfg["tray"]:
            self._start_tray()
        self.fps = None
        self.fps_streak = (0, 0)  # (pid, seconds in a row with game-like FPS)
        self.fps_overlay = FpsOverlay(self) if _user32 is not None else None
        if self.cfg["fps"]:
            self._start_fps()
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
        if getattr(self, "game_hidden_at", None) is not None and _user32 is not None:
            # shown on purpose during a game: stay until another window comes to the front
            self.game_override = _user32.GetForegroundWindow()
            self.root.geometry("+%d+%d" % self.game_hidden_at)
            self.game_hidden_at = None
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
        while self.tray is not None:
            try:
                click = self.tray.clicks.get_nowait()
            except queue.Empty:
                break
            if click == "left":
                self.hide_widget() if self.visible else self.show_widget()
            else:
                self._tray_menu()
        self.root.after(120, self._poll_hotkey)

    # --- FPS ---
    def _start_fps(self):
        """ETW sessions usually need admin rights (some PCs allow them without);
        if Windows refuses, fps.error says why."""
        if self.fps is None:
            self.fps = FpsReader()
            self.fps.start()

    def _toggle_fps(self):
        self.cfg["fps"] = self.fps_var.get()
        save_config(self.cfg)
        if self.cfg["fps"]:
            self._start_fps()
            self.root.after(1500, self._fps_problem)
        elif self.fps is not None:
            self.fps.stop()
            self.fps = None
            self.fps_overlay.hide()

    def _fps_problem(self):
        """Tell why the counter can't work (after switching it on)."""
        if self.fps is not None and self.fps.error and self.toast is not None:
            scale = theme_scale(self.cfg["theme"])
            hint = (" " + tr("Включи в меню «Права администратора (температуры)».")
                    if self.fps.needs_admin else "")
            self.toast.show(tr("FPS недоступен"), tr("Windows отказала: {error}.", error=self.fps.error) + hint,
                            scale(0.75))

    def _apply_dim(self):
        """While a game (FPS shown) or a fullscreen window is in front: hide the widget
        (parked off-screen, so nothing gets activated) or fade it, as chosen in the
        settings; everything comes back when that window goes. Ctrl+Alt+W during
        the game shows the widget anyway, until another window comes to the front."""
        if _user32 is None:
            return
        fg = _user32.GetForegroundWindow()
        overlay = getattr(self, "fps_overlay", None)
        busy = (fg != self.hwnd and fg != getattr(self, "game_override", None)
                and ((overlay is not None and overlay.visible) or fullscreen_app_active()))
        mode = self.cfg["in_games"]
        hide = busy and mode == "hide" and self.visible
        hidden_at = getattr(self, "game_hidden_at", None)
        if hide and hidden_at is None:
            self.game_hidden_at = (self.root.winfo_x(), self.root.winfo_y())
            self.root.geometry("+-9000+-9000")
        elif not hide and hidden_at is not None:
            self.game_hidden_at = None
            self.root.geometry("+%d+%d" % hidden_at)
        alpha = float(self.cfg["alpha"])
        if busy and mode == "dim":
            alpha = min(alpha, DIM_ALPHA)
        if alpha != getattr(self, "_alpha_now", None):
            self._alpha_now = alpha
            self.root.attributes("-alpha", alpha)
        if getattr(self, "game_override", None) not in (None, fg):
            self.game_override = None  # the game lost the front: the override ends

    def _set_in_games(self):
        self.cfg["in_games"] = self.in_games_var.get()
        save_config(self.cfg)
        self._apply_dim()
    def _set_size(self):
        """Scale the widget (fonts and all) without restarting."""
        self.cfg["size"] = self.size_var.get()
        save_config(self.cfg)
        self.renderer = Renderer(self.dpi_scale * self.cfg["size"] / 100)
        self.height = None  # force the new window size
        self._draw()
        self.root.update_idletasks()
        self._place()  # keep it on screen when it grew

    def _update_fps_overlay(self):
        """Show the counter over the window in front if it is a game: not ignored, and
        fullscreen or drawing 25+ frames a second for 3 seconds in a row."""
        shown = None
        fg = _user32.GetForegroundWindow() if self.fps is not None else None
        own = {self.hwnd, getattr(self.taskbar, "hwnd", None)}
        if fg and fg not in own and self.fps.running:
            pid = window_pid(fg)
            stats = self.fps.stats(pid) if pid != os.getpid() else None
            fps = stats[0] if stats else None
            if fps is not None and process_name(pid) not in FPS_IGNORE:
                last_pid, streak = self.fps_streak
                streak = streak + 1 if (pid == last_pid and fps >= 25) else int(fps >= 25)
                self.fps_streak = (pid, streak)
                if fullscreen_app_active() or streak >= 3:
                    shown = (stats, window_rect(fg))
        if shown:
            ping = self.ping.ms if getattr(self, "ping", None) and self.ping.ready else None
            self.fps_overlay.show(shown[0], shown[1], theme_scale(self.cfg["theme"]), ping)
        else:
            self.fps_overlay.hide()

    # --- tray icon ---
    def _start_tray(self):
        self.tray = TrayIcon()
        self.tray.start()

    def _toggle_tray(self):
        self.cfg["tray"] = self.tray_var.get()
        save_config(self.cfg)
        if self.cfg["tray"] and self.tray is None:
            self._start_tray()
        elif not self.cfg["tray"] and self.tray is not None:
            self.tray.stop()
            self.tray = None

    def _tray_menu(self):
        x, y = self.root.winfo_pointerxy()
        try:  # a popup menu closes on an outside click only if its owner is in front
            _user32.SetForegroundWindow(self.hwnd)
        except (AttributeError, OSError):
            pass
        try:
            self.menu.tk_popup(x, y)
        finally:
            self.menu.grab_release()
        self._back_if_pinned()

    def _tray_tip(self, data):
        text = f"PC Monitor\nCPU {data['cpu']:.0f}%  ·  RAM {data['ram_pct']:.0f}%"
        if data["temp_cpu"] is not None:
            text += f"  ·  {data['temp_cpu']:.0f}°"
        self.tray.set_tip(text)

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
        if getattr(self, "settings", None) is not None:
            self.settings.sync()

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
        """Tk variables for every setting (shared by the menu and the settings
        window) and the short right-click menu; the rest is in "Настройки…"."""
        cfg = self.cfg
        self.pinned_var = tk.BooleanVar(value=bool(cfg["pinned"]))
        self.topmost_var = tk.BooleanVar(value=bool(cfg["topmost"]))
        self.alpha_var = tk.DoubleVar(value=float(cfg["alpha"]))
        self.theme_var = tk.StringVar(value=cfg["theme"])
        self.taskbar_var = tk.BooleanVar(value=bool(cfg["taskbar"]))
        self.tray_var = tk.BooleanVar(value=bool(cfg["tray"]))
        self.fps_var = tk.BooleanVar(value=bool(cfg["fps"]))
        self.procs_var = tk.BooleanVar(value=self.show_procs if hasattr(self, "show_procs") else False)
        self.compact_var = tk.BooleanVar(value=bool(cfg["compact"]))
        self.autostart_var = tk.BooleanVar(value=autostart_enabled() or admin_task_enabled())
        self.admin_var = tk.BooleanVar(value=bool(cfg["run_as_admin"]))
        self.section_vars = {key: tk.BooleanVar(value=key not in cfg["hidden"]) for key in SECTIONS}
        self.alerts_var = tk.BooleanVar(value=bool(cfg["alerts"]))
        self.limit_vars = {key: tk.IntVar(value=cfg[key])
                           for key in ("alert_temp", "alert_ram", "alert_battery")}
        self.size_var = tk.IntVar(value=cfg["size"])
        self.in_games_var = tk.StringVar(value=cfg["in_games"])
        self.ping_var = tk.StringVar(value=cfg["ping_host"])
        self.updates_var = tk.BooleanVar(value=bool(cfg["check_updates"]))
        self.lang_var = tk.StringVar(value=cfg["lang"])

        menu = tk.Menu(self.root, tearoff=0)
        menu.add_command(label=tr("Настройки…"), command=self._show_settings)
        menu.add_command(label=tr("История за час и сутки"), command=self._show_history)
        menu.add_checkbutton(label=tr("Список процессов"), variable=self.procs_var,
                             command=lambda: self._set_procs(self.procs_var.get()))
        menu.add_checkbutton(label=tr("Компактный режим"), variable=self.compact_var,
                             command=self._toggle_compact)
        menu.add_checkbutton(label=tr("Закрепить на рабочем столе"), variable=self.pinned_var,
                             command=self._toggle_pin)
        menu.add_separator()
        about = tk.Menu(menu, tearoff=0)
        about.add_command(label=f"PC Monitor {APP_VERSION}", state="disabled")
        about.add_command(label=tr("Проверить сейчас"), command=self._check_updates_now)
        about.add_command(label=tr("Страница программы"), command=lambda: open_in_browser(RELEASES_URL))
        menu.add_cascade(label=tr("О программе"), menu=about)
        menu.add_separator()
        menu.add_command(label=tr("Скрыть  ({hotkey} — показать)", hotkey=HOTKEY_TEXT),
                         command=self.hide_widget)
        menu.add_command(label=tr("Закрыть"), command=self.quit)
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

    def _pick_colors(self):
        """Two colour dialogs: for low and for high load; then switch to "custom"."""
        from tkinter import colorchooser
        picked = []
        for title, initial in ((tr("Цвет: низкая нагрузка"), self.cfg["custom_colors"][0]),
                               (tr("Цвет: высокая нагрузка"), self.cfg["custom_colors"][1])):
            _, hex_color = colorchooser.askcolor(color=initial, title=title, parent=self.root)
            if not hex_color:
                return  # cancelled: keep everything as it was
            picked.append(hex_color.lower())
        self.cfg["custom_colors"] = picked
        custom_stops[:] = picked
        self.theme_var.set("custom")
        self._set_theme()

    def _toggle_topmost(self):
        self.cfg["topmost"] = self.topmost_var.get()
        self.root.attributes("-topmost", self.cfg["topmost"])
        save_config(self.cfg)

    def _set_alpha(self):
        self.cfg["alpha"] = self.alpha_var.get()
        self.root.attributes("-alpha", self.cfg["alpha"])
        self._alpha_now = self.cfg["alpha"]
        save_config(self.cfg)

    def quit(self):
        if getattr(self, "fps", None) is not None:
            self.fps.stop()  # an ETW session outlives the process unless stopped
            self.fps = None
        if getattr(self, "history", None) is not None:
            self.history.flush()
        if getattr(self, "tray", None) is not None:
            self.tray.stop()  # or a dead icon stays in the tray until hovered
            self.tray = None
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
        if (moved < 5 and self.data and self.data.get("update")
                and self._in_box(self.renderer.update_box, event)):
            self._update_clicked()
        if moved < 5 and self._in_box(self.renderer.temps_box, event):
            self._show_history()
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
                or self._in_box(self.renderer.hero_box, event)
                or self._in_box(self.renderer.update_box, event)
                or self._in_box(self.renderer.temps_box, event))
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

    # --- language ---
    def _set_language(self):
        self.cfg["lang"] = self.lang_var.get()
        save_config(self.cfg)
        set_language(self.cfg["lang"])
        old = self.menu
        self._build_menu()  # new texts, new variables
        old.destroy()
        settings = getattr(self, "settings", None)
        if settings is not None and settings.alive():  # reopen in the new language
            tab = settings.current_tab()
            settings.close()
            self.settings = None
            self.root.after(0, lambda: self._show_settings(tab))
        if self.taskbar is not None:
            self.taskbar.measure()
        self.data = self._collect() if self.data is not None else None
        self._draw()
        if self.taskbar is not None and self.data:
            self.taskbar.draw(self.data, self.cfg["theme"])

    # --- ping target ---
    def _set_ping_host(self, host):
        self.cfg["ping_host"] = host
        self.ping_var.set(host)
        save_config(self.cfg)
        self.ping.set_host(host)

    def _show_settings(self, tab=0):
        if getattr(self, "settings", None) is not None and self.settings.alive():
            self.settings.top.lift()
            self.settings.top.focus_force()
            return
        self.settings = SettingsWindow(self, tab)

    def _show_history(self):
        if self.history_card is None:
            return
        if self.history_card.visible:
            self.history_card.hide()
            return
        if self.visible:
            rect = (self.root.winfo_rootx(), self.root.winfo_rooty(),
                    self.root.winfo_rootx() + self.root.winfo_width(),
                    self.root.winfo_rooty() + self.root.winfo_height())
        else:  # widget hidden: put the card in the middle of the screen
            wl, wt, wr, wb = work_area()
            mid_x, mid_y = (wl + wr) // 2, (wt + wb) // 2
            rect = (mid_x + self.history_card.px(HistoryCard.WIDTH) // 2, mid_y - 150, mid_x, mid_y)
        self.history_card.show_near(rect)

    # --- updates ---
    def _toggle_updates(self):
        self.cfg["check_updates"] = self.updates_var.get()
        save_config(self.cfg)
        if self.cfg["check_updates"]:
            self.updates.check_now()

    def _check_updates_now(self):
        self.manual_check = self.updates.checks
        if not self.cfg["check_updates"]:
            self.updates_var.set(True)
            self._toggle_updates()
        else:
            self.updates.check_now()

    def _can_auto_update(self):
        found = getattr(self, "updates", None) and self.updates.found
        return bool(found and found[2] and update_target())

    def _update_state(self):
        up = getattr(self, "updater", None)
        if up is None or up.state == "error":
            return None
        return (up.state, up.progress) if up.state == "download" else (up.state, None)

    def _update_clicked(self):
        """Update in place when this copy can, otherwise open the release page."""
        found = self.updates.found
        if not found or getattr(self, "updater", None) is not None:
            return
        target = update_target()
        if found[2] and target:
            self.updater = Updater(found[2], target)
            self.updater.start()
            self._draw()
        else:
            open_in_browser(found[1])

    def _poll_updater(self):
        up = getattr(self, "updater", None)
        if up is None:
            return
        if up.state == "ready":
            self.updater = None
            swap_and_restart(up.app_dir, up.staging)
            self.quit()
        elif up.state == "error":
            self.updater = None
            url = self.updates.found[1] if self.updates.found else RELEASES_URL
            scale = theme_scale(self.cfg["theme"])
            self.toast.show(tr("Не удалось обновить"),
                            tr("{error}. Нажми, чтобы открыть страницу загрузки.", error=up.error),
                            scale(0.75), on_click=lambda: open_in_browser(url))

    def _update_notice(self):
        """Toast once per new version, and the answer to "Проверить сейчас"."""
        found = self.updates.found if self.cfg["check_updates"] else None
        manual = self.manual_check is not None and self.updates.checks > self.manual_check
        if manual:
            self.manual_check = None
        scale = theme_scale(self.cfg["theme"])
        if found and (manual or found[0] != self.cfg["update_seen"]):
            version = found[0]
            self.cfg["update_seen"] = version
            save_config(self.cfg)
            body = (tr("У тебя {v}. Нажми, чтобы обновить.", v=APP_VERSION) if self._can_auto_update()
                    else tr("У тебя {v}. Нажми, чтобы открыть страницу загрузки.", v=APP_VERSION))
            self.toast.show(tr("Вышла версия {v}", v=version), body, scale(0.0),
                            on_click=self._update_clicked)
        elif manual:
            if self.updates.failed:
                self.toast.show(tr("Не удалось проверить"), tr("Нет связи с GitHub. Попробуй позже."),
                                scale(0.75))
            else:
                self.toast.show(tr("Обновлений нет"), tr("У тебя последняя версия, {v}.", v=APP_VERSION),
                                scale(0.0))

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
        scale = theme_scale(self.cfg["theme"])
        hot = scale(1.0)
        limit = self.cfg["alert_temp"]
        t = data["temp_cpu"]
        if t is not None:
            self._alert("cpu_temp", t >= limit, t < limit - ALERT_HYST, 5,
                        tr("Процессор перегревается"),
                        tr("CPU {t}°. Закрой тяжёлые программы или проверь, не забиты ли решётки.",
                           t=f"{t:.0f}"),
                        hot)
        t = data["temp_gpu"]
        if t is not None:
            self._alert("gpu_temp", t >= limit - 3, t < limit - 3 - ALERT_HYST, 5,
                        tr("Видеокарта перегревается"),
                        tr("GPU {t}°. Понизь настройки графики или ограничь FPS.", t=f"{t:.0f}"), hot)
        pct = data["ram_pct"]

        def ram_body():
            name, _, mem = (top_processes(1, key="mem") or [("?", 0, 0)])[0]
            return (tr("Занято {pct}% ({used} из {total} ГБ).", pct=f"{pct:.0f}",
                       used=fmt_gb(data['ram_used']), total=fmt_gb(data['ram_total'])) + " "
                    + tr("Больше всех: {name} — {mem}.", name=name, mem=fmt_mem(mem)))

        ram_limit = self.cfg["alert_ram"]
        self._alert("ram", pct >= ram_limit, pct < ram_limit - ALERT_HYST, 10,
                    tr("Память почти закончилась"), ram_body, hot)
        batt = data["battery"]
        if batt is not None:
            level, plugged = batt
            low = self.cfg["alert_battery"]
            left = data.get("battery_left")
            self._alert("battery", not plugged and level <= low,
                        plugged or level > low + ALERT_HYST, 0,
                        tr("Батарея садится"),
                        tr("Осталось {level}%", level=f"{level:.0f}")
                        + (tr(", примерно на {left}", left=fmt_left(left)) if left else "")
                        + ". " + tr("Подключи зарядку."),
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
        if getattr(self, "game_hidden_at", None) is not None:
            return  # parked off-screen for a game: keep the real position
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
        cores = psutil.cpu_percent(interval=None, percpu=True)
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

        if getattr(self, "history", None) is not None:
            recent = self.cpu_hist[-HISTORY_STEP_S:]
            self.history.add(sum(recent) / len(recent), self.lhm.sensors["cpu"],
                             self.lhm.sensors["gpu"])

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
            "cores": cores,
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
            "battery_wear": sensors.get("battery_wear"),
            "battery_design": sensors.get("battery_design"),
            "battery_full": sensors.get("battery_full"),
            "battery_rate": sensors.get("battery_rate"),
            "fans": sensors.get("fans"),
            "lhm_online": self.lhm.online,
            "sensor_hint": self.lhm.hint(),
            "ping": self.ping.ms,
            "ping_ready": self.ping.ready,
            "ping_host": self.ping.host,
            "procs": top_processes() if self.show_procs else None,
            "update": (getattr(self, "updates", None) and self.cfg["check_updates"]
                       and self.updates.found) or None,
            "update_auto": self._can_auto_update(),
            "update_state": self._update_state(),
        }

    def _refresh(self):
        self.data = self._collect()
        self._draw()
        if getattr(self, "taskbar", None) is not None:
            self.taskbar.draw(self.data, self.cfg["theme"])
        if getattr(self, "toast", None) is not None:
            self._check_alerts(self.data)
            self._update_notice()
            self._poll_updater()
        card = getattr(self, "history_card", None)
        if card is not None and card.visible and int(time.time()) % 5 == 0:
            card.refresh()
        if getattr(self, "tray", None) is not None:
            self._tray_tip(self.data)
        if getattr(self, "fps_overlay", None) is not None:
            self._update_fps_overlay()
        self._apply_dim()
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


def leave_program_folder():
    """Work from the data folder in %APPDATA%, not the program folder: a process whose
    current folder is inside it (and children that inherit that) would block
    an update from renaming the folder. Also drop a staging folder that a failed
    update left behind."""
    try:
        os.chdir(DATA_DIR)
    except OSError:
        pass
    leftover = APP_DIR + ".new"
    if is_frozen() and os.path.isdir(leftover):
        import shutil
        log.warning("removing an unfinished update in %s", leftover)
        shutil.rmtree(leftover, ignore_errors=True)


if __name__ == "__main__":
    setup_logging()
    leave_program_folder()
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

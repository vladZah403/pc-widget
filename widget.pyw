"""PC Monitor — desktop widget for Windows.

Shows CPU load (number + 60-second histogram), RAM, disk space and I/O,
network speed, battery, uptime and CPU/GPU temperatures. Temperatures come
from LibreHardwareMonitor's Remote Web Server (http://localhost:8085),
because Windows does not expose real sensor temperatures to normal programs.

The whole widget is drawn with Pillow (supersampled for smooth edges) and
shown in a borderless tkinter window with transparent rounded corners.
One colour scale (cool -> hot) colours every value, so colour always means
"how loaded / how hot".

Controls: drag with the left mouse button, right click for the menu.
"""

import ctypes
import json
import os
import re
import socket
import struct
import threading
import time
import tkinter as tk
import urllib.request

import psutil
from PIL import Image, ImageDraw, ImageFont, ImageTk

APP_DIR = os.path.dirname(os.path.abspath(__file__))
CONFIG_PATH = os.path.join(APP_DIR, "widget_config.json")
FONT_DIR = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts")

LHM_URL = "http://localhost:8085/data.json"
LHM_POLL_S = 3
UPDATE_MS = 1000
HISTORY_LEN = 60
SS = 3  # supersampling factor for shapes

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

DEFAULT_CONFIG = {"x": None, "y": None, "topmost": True, "alpha": 1.0, "theme": "heat",
                  "pinned": True, "taskbar": True, "alerts": True}

# alerts: (switch on at, switch off below) — hysteresis so a value hovering
# around the limit doesn't spam; each alert at most once per ALERT_COOLDOWN_S
ALERT_CPU_TEMP = (90, 85)
ALERT_GPU_TEMP = (87, 82)
ALERT_RAM = (90, 85)
ALERT_BATTERY = (15, 20)
ALERT_COOLDOWN_S = 600


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


def fmt_uptime(seconds):
    seconds = int(seconds)
    days, rest = divmod(seconds, 86400)
    hours, rest = divmod(rest, 3600)
    minutes = rest // 60
    if days:
        return f"{days} д {hours} ч"
    return f"{hours} ч {minutes} мин"


def load_config():
    cfg = dict(DEFAULT_CONFIG)
    try:
        with open(CONFIG_PATH, encoding="utf-8") as f:
            cfg.update(json.load(f))
    except (OSError, ValueError):
        pass
    if cfg.get("theme") not in THEMES:
        cfg["theme"] = DEFAULT_CONFIG["theme"]
    return cfg


def save_config(cfg):
    try:
        with open(CONFIG_PATH, "w", encoding="utf-8") as f:
            json.dump(cfg, f, indent=2)
    except OSError:
        pass


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
GPU_PREFS = ("gpu core", "gpu hot spot", "gpu")


def _parse_num(text):
    m = _NUM_RE.search(text or "")
    return float(m.group().replace(",", ".")) if m else None


def _classify_hardware(node):
    img = (node.get("ImageURL") or "").lower()
    text = (node.get("Text") or "").lower()
    if "cpu" in img:
        return "cpu"
    if any(k in img for k in ("nvidia", "ati", "amd", "gpu")):
        return "gpu"
    if any(k in text for k in ("geforce", "radeon", "graphics", "gpu")):
        return "gpu"
    return None


def _sensor_kind(sensor_id, hw_kind):
    sid = (sensor_id or "").lower()
    if "cpu/" in sid:
        return "cpu"
    if "/gpu" in sid:
        return "gpu"
    return hw_kind


def _pick(sensors, prefs):
    for pref in prefs:
        for name, value in sensors:
            if name.startswith(pref):
                return value
    return max((v for _, v in sensors), default=None)


def extract_temps(tree):
    """Return (cpu_temp, gpu_temp) in °C from LHM data.json, None if missing."""
    found = {"cpu": [], "gpu": []}

    def walk(node, hw_kind, in_temps):
        children = node.get("Children") or []
        text = (node.get("Text") or "").strip()
        if not children:
            if not in_temps or "distance" in text.lower():
                return
            value = _parse_num(node.get("Value"))
            kind = _sensor_kind(node.get("SensorId"), hw_kind)
            if kind in found and value is not None and 0 < value < 150:
                found[kind].append((text.lower(), value))
            return
        is_group = text.lower() == "temperatures"
        kind = hw_kind if is_group else (_classify_hardware(node) or hw_kind)
        for child in children:
            walk(child, kind, in_temps or is_group)

    walk(tree, None, False)
    return _pick(found["cpu"], CPU_PREFS), _pick(found["gpu"], GPU_PREFS)


class LhmReader(threading.Thread):
    """Polls LibreHardwareMonitor in the background so the UI never freezes."""

    def __init__(self):
        super().__init__(daemon=True)
        self.cpu = None
        self.gpu = None
        self.online = False

    def run(self):
        while True:
            try:
                with urllib.request.urlopen(LHM_URL, timeout=1) as resp:
                    tree = json.loads(resp.read().decode("utf-8"))
                self.cpu, self.gpu = extract_temps(tree)
                self.online = True
            except (OSError, ValueError):
                self.cpu = self.gpu = None
                self.online = False
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
        self.prev, self.prev_t = current, now
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

    def _rich_right(self, d, x_right, y, parts):
        """Draw [(text, font, colour), ...] right-aligned on baseline y."""
        x = self.px(x_right)
        for text, font, color in reversed(parts):
            x -= font.getlength(text)
            d.text((x, self.px(y)), text, font=font, fill=color, anchor="ls")

    # --- layout ---
    def layout_height(self, data):
        h = 128  # header + hero
        if data.get("procs") is not None:
            h += 120
        h += 38 * (1 + len(data["disks"]))  # memory + disks
        h += 40  # temperature
        if not data["lhm_online"] or (data["temp_cpu"] is None and data["temp_gpu"] is None):
            h += 16
        h += 16 + 22 + 22 + 22 + 30  # divider, net, ping, disk io, footer
        return self.px(h)

    def render(self, data, theme, close_hover=False, pinned=False, pin_hover=False):
        scale = Scale(THEMES[theme][1])
        P, Wl = self.PAD, self.WIDTH
        H = self.layout_height(data)
        card = self._card(H)

        layer = Image.new("RGBA", (self.W * SS, H * SS), (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        texts = []  # deferred text: (x, y, text, font, fill, anchor)

        # header
        texts.append((P, 28, "Система", self.f_title, TEXT, "ls"))
        self._close_icon(d, Wl - P - 4, 23, CLOSE_HOVER if close_hover else DIM)
        self.close_box = (self.px(Wl - P - 14), self.px(11), self.px(Wl - P + 6), self.px(35))
        pin_x = Wl - P - 28
        pin_color = scale(0.0) if pinned else (TEXT if pin_hover else FAINT)
        if pinned and pin_hover:
            pin_color = lerp(pin_color, TEXT, 0.35)
        self._pin_icon(d, pin_x, 23, pin_color)
        self.pin_box = (self.px(pin_x - 10), self.px(11), self.px(pin_x + 9), self.px(35))
        batt = data["battery"]
        if batt is not None:
            pct, plugged = batt
            bx = Wl - P - 50 - 19
            self._battery_icon(d, bx, 23.5, pct, plugged, scale)
            texts.append(("rich", bx - 6, 28, [(f"{pct:.0f}%", self.f_small, DIM)]
                          + ([(" ⚡", self.f_sym, DIM)] if plugged else [])))

        # hero: CPU number + histogram
        cpu = data["cpu"]
        hero_base = 92
        num = f"{cpu:.0f}"
        num_w = self.f_hero.getlength(num) / self.s
        texts.append((P - 1, hero_base, num, self.f_hero, scale(cpu / 100), "ls"))
        texts.append((P + num_w + 2, hero_base, "%", self.f_hero_unit, DIM, "ls"))
        procs = data.get("procs")
        texts.append((P, hero_base + 18, "Процессор", self.f_small, DIM, "ls"))
        chev_x = P + self.f_small.getlength("Процессор ") / self.s
        texts.append((chev_x, hero_base + 18, "▴" if procs is not None else "▾", self.f_sym, DIM, "ls"))
        hx0 = P + 82
        self._histogram(d, hx0, 50, Wl - P, 108, data["cpu_hist"], scale)
        self.hero_box = (self.px(P - 4), self.px(42), self.px(Wl - P + 4), self.px(118))

        y = 128
        if procs is not None:
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
            y += 120
        # memory
        texts.append((P, y + 14, "Память", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 14, [
            (f"{fmt_gb(data['ram_used'])} из {fmt_gb(data['ram_total'])} ГБ   ", self.f_small, DIM),
            (f"{data['ram_pct']:.0f}%", self.f_value, TEXT)]))
        self._segment_meter(d, P, y + 22, Wl - P, data["ram_pct"], scale)
        y += 38

        # disks
        for name, used, total, pct in data["disks"]:
            texts.append((P, y + 14, f"Диск {name}", self.f_label, TEXT, "ls"))
            texts.append(("rich", Wl - P, y + 14, [
                (f"{fmt_gb(used)} из {fmt_gb(total)} ГБ   ", self.f_small, DIM),
                (f"{pct:.0f}%", self.f_value, TEXT)]))
            self._segment_meter(d, P, y + 22, Wl - P, pct, scale)
            y += 38

        # temperature
        cpu_t, gpu_t = data["temp_cpu"], data["temp_gpu"]
        online = data["lhm_online"] and (cpu_t is not None or gpu_t is not None)
        texts.append((P, y + 14, "Температура", self.f_label, TEXT, "ls"))
        if online:
            parts = []
            if gpu_t is not None:
                parts += [("GPU ", self.f_small, DIM),
                          (f"{gpu_t:.0f}°", self.f_value, scale((gpu_t - 30) / 70))]
            if cpu_t is not None:
                if parts:
                    parts.append(("    ", self.f_small, DIM))
                parts += [("CPU ", self.f_small, DIM),
                          (f"{cpu_t:.0f}°", self.f_value, scale((cpu_t - 30) / 70))]
            texts.append(("rich", Wl - P, y + 14, parts))
        else:
            texts.append(("rich", Wl - P, y + 14, [("нет данных", self.f_small, FAINT)]))
        self._temp_strip(d, P, y + 26, Wl - P, cpu_t, gpu_t, scale, online)
        y += 40
        if not online:
            hint = ("Запусти LibreHardwareMonitor с веб-сервером" if not data["lhm_online"]
                    else "Запусти LibreHardwareMonitor от админа")
            texts.append((P, y + 8, hint, self.f_small, FAINT, "ls"))
            y += 16

        # divider
        d.rectangle(self._ss(P, y + 6, Wl - P, y + 6 + 1 / self.s), fill=TRACK)
        y += 16

        # network & disk activity
        down_c, up_c = scale(0.0), scale(0.75)
        texts.append((P, y + 12, "Сеть", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 12, [
            ("↓ ", self.f_sym, down_c), (fmt_speed(data["net_down"]), self.f_small, TEXT),
            ("     ↑ ", self.f_sym, up_c), (fmt_speed(data["net_up"]), self.f_small, TEXT)]))
        y += 22
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
        y += 22
        texts.append((P, y + 12, "Диск", self.f_label, TEXT, "ls"))
        texts.append(("rich", Wl - P, y + 12, [
            ("чтение ", self.f_small, DIM), (fmt_speed(data["io_read"]), self.f_small, TEXT),
            ("   запись ", self.f_small, DIM), (fmt_speed(data["io_write"]), self.f_small, TEXT)]))
        y += 22

        # footer
        texts.append((P, y + 18, f"Без перезагрузки {fmt_uptime(data['uptime'])}",
                      self.f_small, FAINT, "ls"))

        img = card.convert("RGBA")
        img.alpha_composite(layer.reduce(SS))
        td = ImageDraw.Draw(img)
        for item in texts:
            if item[0] == "rich":
                self._rich_right(td, item[1], item[2], item[3])
            else:
                x, ty, text, font, fill, anchor = item
                self._t(td, x, ty, text, font, fill, anchor)
        return img.convert("RGB")


# ---------- taskbar mini panel ----------

class TaskbarPanel:
    """Two-line readout on the Windows taskbar, just left of the tray icons."""

    # hidden = parked off-screen; Win32 ShowWindow would desync Tk's geometry
    OFFSCREEN = "1x1+-32000+-32000"

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
        self.col_c = max(f.getlength("CPU "), f.getlength("GPU ")) + fv.getlength("100°")
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
        scale = Scale(THEMES[theme][1])
        h = self.bar_h
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
            for y, name, t in ((rows[0], "CPU", data["temp_cpu"]), (rows[1], "GPU", data["temp_gpu"])):
                d.text((x_c, y), name, font=f, fill=label_c, anchor="lm")
                value, color = ("—", label_c) if t is None else (f"{t:.0f}°", tone((t - 30) / 70))
                d.text((x_c + self.col_c, y), value, font=fv, fill=color, anchor="rm")

        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        if img.size != self.size:
            self.size = img.size
            self.layout()

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


class Toast:
    """Our own notification card in the bottom-right corner, in the widget's style.
    Click to dismiss; disappears by itself after a few seconds."""

    OFFSCREEN = "1x1+-32000+-32000"
    WIDTH = 300
    SHOW_MS = 9000

    def __init__(self, widget):
        self.widget = widget
        self.px = widget.renderer.px
        self.s = widget.renderer.s
        self.f_title = load_font(self.px(14), "SemiBold")
        self.f_body = load_font(self.px(12))
        self.top = top = tk.Toplevel(widget.root)
        top.overrideredirect(True)
        top.configure(bg=KEY_HEX)
        top.attributes("-transparentcolor", KEY_HEX)
        top.attributes("-topmost", True)
        self.view = tk.Label(top, bg=KEY_HEX, bd=0, highlightthickness=0, cursor="hand2")
        self.view.pack()
        self.view.bind("<ButtonRelease-1>", lambda e: self.hide())
        top.geometry(self.OFFSCREEN)
        self.photo = None
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
        px, S = self.px, SS
        W = px(self.WIDTH)
        lines = self._wrap(body, W - px(46))
        H = px(46 + 17 * len(lines))
        big = Image.new("RGB", (W * S, H * S), KEY)
        d = ImageDraw.Draw(big)
        r = px(12) * S
        d.rounded_rectangle((0, 0, W * S - 1, H * S - 1), radius=r, fill=BORDER)
        bw = max(1, px(1)) * S
        d.rounded_rectangle((bw, bw, W * S - 1 - bw, H * S - 1 - bw), radius=r - bw, fill=CARD)
        d.rounded_rectangle((px(14) * S, px(16) * S, px(18) * S, H * S - px(16) * S),
                            radius=px(2) * S, fill=color)
        img = big.reduce(S)
        td = ImageDraw.Draw(img)
        td.text((px(30), px(29)), title, font=self.f_title, fill=TEXT, anchor="ls")
        for i, line in enumerate(lines):
            td.text((px(30), px(48 + 17 * i)), line, font=self.f_body, fill=DIM, anchor="ls")
        return img

    def show(self, title, body, color):
        img = self.render(title, body, color)
        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        _, _, right, bottom = work_area()
        x = right - img.width - self.px(14)
        y = bottom - img.height - self.px(14)
        self.top.geometry(f"{img.width}x{img.height}+{x}+{y}")
        self.top.lift()
        if self.job:
            self.top.after_cancel(self.job)
        self.job = self.top.after(self.SHOW_MS, self.hide)

    def hide(self):
        self.job = None
        self.top.geometry(self.OFFSCREEN)


# ---------- window ----------

class Widget:
    def __init__(self):
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(1)
        except (AttributeError, OSError):
            pass

        self.cfg = load_config()
        self.root = root = tk.Tk()
        root.title("PC Monitor")
        root.overrideredirect(True)
        root.configure(bg=KEY_HEX)
        root.attributes("-transparentcolor", KEY_HEX)
        root.attributes("-topmost", bool(self.cfg["topmost"]))
        root.attributes("-alpha", float(self.cfg["alpha"]))

        self.renderer = Renderer(root.winfo_fpixels("1i") / 96)
        self.view = tk.Label(root, bg=KEY_HEX, bd=0, highlightthickness=0)
        self.view.pack()
        self.photo = None
        self.height = None
        self.close_hover = False
        self.close_pressed = False
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
        self.partitions = [p.mountpoint for p in psutil.disk_partitions(all=False)
                           if p.fstype and "cdrom" not in p.opts]
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

    # --- taskbar panel ---
    def _taskbar_tick(self):
        if self.taskbar is not None:
            try:
                self.taskbar.layout()
            except Exception:  # explorer restarting etc. — try again next tick
                pass
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
        self.alerts_var = tk.BooleanVar(value=bool(self.cfg["alerts"]))
        menu.add_checkbutton(label="Предупреждения", variable=self.alerts_var,
                             command=self._toggle_alerts)
        menu.add_separator()
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
                pass
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
                self.quit()
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
        t = data["temp_cpu"]
        if t is not None:
            self._alert("cpu_temp", t >= ALERT_CPU_TEMP[0], t < ALERT_CPU_TEMP[1], 5,
                        "Процессор перегревается",
                        f"CPU {t:.0f}°. Закрой тяжёлые программы или проверь, не забиты ли решётки.",
                        hot)
        t = data["temp_gpu"]
        if t is not None:
            self._alert("gpu_temp", t >= ALERT_GPU_TEMP[0], t < ALERT_GPU_TEMP[1], 5,
                        "Видеокарта перегревается",
                        f"GPU {t:.0f}°. Понизь настройки графики или ограничь FPS.", hot)
        pct = data["ram_pct"]

        def ram_body():
            name, _, mem = (top_processes(1, key="mem") or [("?", 0, 0)])[0]
            return (f"Занято {pct:.0f}% ({fmt_gb(data['ram_used'])} из {fmt_gb(data['ram_total'])} ГБ). "
                    f"Больше всех: {name} — {fmt_mem(mem)}.")

        self._alert("ram", pct >= ALERT_RAM[0], pct < ALERT_RAM[1], 10,
                    "Память почти закончилась", ram_body, hot)
        batt = data["battery"]
        if batt is not None:
            level, plugged = batt
            self._alert("battery", not plugged and level <= ALERT_BATTERY[0],
                        plugged or level > ALERT_BATTERY[1], 0,
                        "Батарея садится", f"Осталось {level:.0f}%. Подключи зарядку.",
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
            pass
        self.root.after(UPDATE_MS, self.update)

    def _collect(self):
        now = time.monotonic()
        dt = max(now - self.last_t, 0.001)
        self.last_t = now

        cpu = psutil.cpu_percent(interval=None)
        self.cpu_hist = (self.cpu_hist + [cpu])[-HISTORY_LEN:]
        mem = psutil.virtual_memory()

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
        down = (net.bytes_recv - self.last_net.bytes_recv) / dt
        up = (net.bytes_sent - self.last_net.bytes_sent) / dt
        self.last_net = net

        batt = psutil.sensors_battery()
        return {
            "cpu": cpu,
            "cpu_hist": list(self.cpu_hist),
            "ram_pct": mem.percent,
            "ram_used": mem.used,
            "ram_total": mem.total,
            "disks": disks,
            "io_read": max(io_r, 0.0),
            "io_write": max(io_w, 0.0),
            "net_down": max(down, 0.0),
            "net_up": max(up, 0.0),
            "battery": (batt.percent, bool(batt.power_plugged)) if batt else None,
            "uptime": time.time() - psutil.boot_time(),
            "temp_cpu": self.lhm.cpu,
            "temp_gpu": self.lhm.gpu,
            "lhm_online": self.lhm.online,
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
                                   bool(self.cfg["pinned"]), self.pin_hover)
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


def already_running():
    """Named mutex: a second copy (start.bat clicked twice) exits immediately."""
    try:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateMutexW.restype = ctypes.c_void_p
        kernel32.CreateMutexW.argtypes = [ctypes.c_void_p, ctypes.c_bool, ctypes.c_wchar_p]
        globals()["_instance_mutex"] = kernel32.CreateMutexW(None, False, r"Local\PCMonitorWidget")
        return ctypes.get_last_error() == 183  # ERROR_ALREADY_EXISTS
    except (AttributeError, OSError):
        return False


if __name__ == "__main__":
    if not already_running():
        Widget().run()

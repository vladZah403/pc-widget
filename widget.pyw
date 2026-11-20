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
                  "pinned": True}


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
        h += 38 * (1 + len(data["disks"]))  # memory + disks
        h += 40  # temperature
        if not data["lhm_online"] or (data["temp_cpu"] is None and data["temp_gpu"] is None):
            h += 16
        h += 16 + 22 + 22 + 30  # divider, net, disk io, footer
        return self.px(h)

    def render(self, data, theme, close_hover=False):
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
        batt = data["battery"]
        if batt is not None:
            pct, plugged = batt
            bx = Wl - P - 26 - 19
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
        texts.append((P, hero_base + 18, "Процессор", self.f_small, DIM, "ls"))
        hx0 = P + 82
        self._histogram(d, hx0, 50, Wl - P, 108, data["cpu_hist"], scale)

        y = 128
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
        self._drag = (0, 0)
        self.hwnd = None

        self._build_menu()
        self.view.bind("<ButtonPress-1>", self._press)
        self.view.bind("<B1-Motion>", self._drag_move)
        self.view.bind("<ButtonRelease-1>", self._release)
        self.view.bind("<Motion>", self._hover)
        self.view.bind("<Leave>", lambda e: self._set_hover(False))
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

        self.update()
        self._place()
        root.update_idletasks()
        self.hwnd = int(root.wm_frame(), 16)
        self._apply_pin()

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
        menu.add_checkbutton(label="Закрепить на рабочем столе", variable=self.pinned_var,
                             command=self._toggle_pin)
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

    def _toggle_topmost(self):
        self.cfg["topmost"] = self.topmost_var.get()
        self.root.attributes("-topmost", self.cfg["topmost"])
        save_config(self.cfg)

    def _set_alpha(self):
        self.cfg["alpha"] = self.alpha_var.get()
        self.root.attributes("-alpha", self.cfg["alpha"])
        save_config(self.cfg)

    def quit(self):
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

    def _press(self, event):
        self.close_pressed = self._in_close(event)
        self._drag = (event.x_root - self.root.winfo_x(), event.y_root - self.root.winfo_y())

    def _drag_move(self, event):
        if self.close_pressed or self.cfg["pinned"]:
            return
        dx, dy = self._drag
        self.root.geometry(f"+{event.x_root - dx}+{event.y_root - dy}")

    def _release(self, event):
        if self.close_pressed:
            self.close_pressed = False
            if self._in_close(event):
                self.quit()
            return
        if self.cfg["pinned"]:
            self._back_if_pinned()
            return
        self._save_position()

    def _hover(self, event):
        self._set_hover(self._in_close(event))

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
        }

    def _refresh(self):
        self.data = self._collect()
        self._draw()
        # explorer.exe restarted -> the desktop view changed, re-attach
        if self.cfg["pinned"] and self.hwnd and not owner_alive(self.hwnd):
            self._apply_pin()

    def _draw(self):
        if self.data is None:
            return
        img = self.renderer.render(self.data, self.cfg["theme"], self.close_hover)
        self.photo = ImageTk.PhotoImage(img)
        self.view.configure(image=self.photo)
        if img.height != self.height:
            self.height = img.height
            self.root.geometry(f"{img.width}x{img.height}")

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    Widget().run()

"""PC Monitor — desktop widget for Windows.

Shows CPU (with history graph), RAM, disk space and I/O, network speed,
battery and CPU/GPU temperatures. Temperatures come from
LibreHardwareMonitor's Remote Web Server (http://localhost:8085), because
Windows does not expose real sensor temperatures to normal programs.

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

APP_DIR = os.path.dirname(os.path.abspath(__file__))
CONFIG_PATH = os.path.join(APP_DIR, "widget_config.json")

LHM_URL = "http://localhost:8085/data.json"
LHM_POLL_S = 3
UPDATE_MS = 1000
HISTORY_LEN = 60

BG = "#0f1216"
FG = "#e6e9ef"
DIM = "#8a93a3"
TRACK = "#262b33"
GREEN = "#3ddc84"
YELLOW = "#ffc94a"
RED = "#ff5c5c"
BLUE = "#4aa3ff"
GRAPH_FILL = "#16324d"

DEFAULT_CONFIG = {"x": None, "y": None, "topmost": True, "alpha": 0.95}


# ---------- helpers ----------

def load_color(percent):
    if percent < 60:
        return GREEN
    if percent < 85:
        return YELLOW
    return RED


def temp_color(celsius):
    if celsius < 65:
        return GREEN
    if celsius < 85:
        return YELLOW
    return RED


def fmt_gb(n_bytes):
    return f"{n_bytes / 2**30:.1f}"


def fmt_speed(bytes_per_s):
    if bytes_per_s < 1024:
        return f"{bytes_per_s:.0f} Б/с"
    if bytes_per_s < 1024**2:
        return f"{bytes_per_s / 1024:.0f} КБ/с"
    return f"{bytes_per_s / 1024**2:.1f} МБ/с"


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
    return cfg


def save_config(cfg):
    try:
        with open(CONFIG_PATH, "w", encoding="utf-8") as f:
            json.dump(cfg, f, indent=2)
    except OSError:
        pass


def virtual_screen(root):
    """Bounding box of all monitors: (left, top, width, height)."""
    try:
        gsm = ctypes.windll.user32.GetSystemMetrics
        return gsm(76), gsm(77), gsm(78), gsm(79)
    except (AttributeError, OSError):
        return 0, 0, root.winfo_screenwidth(), root.winfo_screenheight()


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


# ---------- UI ----------

class Bar:
    def __init__(self, parent, width, height):
        self.width = width
        self.canvas = tk.Canvas(parent, width=width, height=height, bg=BG,
                                highlightthickness=0, bd=0)
        self.canvas.create_rectangle(0, 0, width, height, fill=TRACK, width=0)
        self.fill = self.canvas.create_rectangle(0, 0, 0, height, fill=GREEN, width=0)
        self.height = height

    def set(self, percent, color=None):
        percent = max(0.0, min(100.0, percent))
        self.canvas.coords(self.fill, 0, 0, self.width * percent / 100, self.height)
        self.canvas.itemconfigure(self.fill, fill=color or load_color(percent))


class Graph:
    def __init__(self, parent, width, height):
        self.width, self.height = width, height
        self.canvas = tk.Canvas(parent, width=width, height=height, bg=BG,
                                highlightthickness=0, bd=0)
        self.canvas.create_rectangle(0, 0, width, height, fill="#141920", width=0)
        self.area = self.canvas.create_polygon(0, 0, 0, 0, 0, 0, fill=GRAPH_FILL, width=0)
        self.line = self.canvas.create_line(0, 0, 0, 0, fill=BLUE, width=1.5)

    def set(self, values):
        if len(values) < 2:
            return
        step = self.width / (HISTORY_LEN - 1)
        offset = HISTORY_LEN - len(values)
        pts = []
        for i, v in enumerate(values):
            pts += [(offset + i) * step, self.height - 1 - (self.height - 2) * v / 100]
        self.canvas.coords(self.line, *pts)
        self.canvas.coords(self.area, pts[0], self.height, *pts, pts[-2], self.height)


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
        root.configure(bg=BG)
        root.attributes("-topmost", bool(self.cfg["topmost"]))
        root.attributes("-alpha", float(self.cfg["alpha"]))

        s = self.scale = root.winfo_fpixels("1i") / 96
        self.width = int(250 * s)
        pad = int(12 * s)
        self.font = ("Segoe UI", 9)
        self.font_bold = ("Segoe UI Semibold", 9)
        self.font_title = ("Segoe UI Semibold", 10)
        self.font_small = ("Segoe UI", 8)

        body = tk.Frame(root, bg=BG, padx=pad, pady=int(10 * s))
        body.pack(fill="both", expand=True)
        self.body = body
        inner_w = self.width - 2 * pad
        bar_h = max(3, int(5 * s))

        # header
        head = tk.Frame(body, bg=BG)
        head.pack(fill="x")
        tk.Label(head, text="PC Monitor", font=self.font_title, bg=BG, fg=FG).pack(side="left")
        close = tk.Label(head, text="✕", font=self.font, bg=BG, fg=DIM, cursor="hand2")
        close.pack(side="right")
        close.bind("<ButtonRelease-1>", lambda e: self.quit())
        self.uptime = tk.Label(head, text="", font=self.font_small, bg=BG, fg=DIM)
        self.uptime.pack(side="right", padx=int(8 * s))

        # CPU
        self.cpu_val = self._row(body, "CPU")
        self.cpu_bar = self._bar(body, inner_w, bar_h)
        self.cpu_graph = Graph(body, inner_w, int(30 * s))
        self.cpu_graph.canvas.pack(pady=(int(4 * s), 0))
        self.cpu_hist = []

        # RAM
        self.ram_val = self._row(body, "RAM")
        self.ram_bar = self._bar(body, inner_w, bar_h)

        # disks
        self.disks = []
        for part in psutil.disk_partitions(all=False):
            if not part.fstype or "cdrom" in part.opts:
                continue
            label = self._row(body, f"Диск {part.mountpoint.rstrip(chr(92))}")
            bar = self._bar(body, inner_w, bar_h)
            self.disks.append((part.mountpoint, label, bar))
        self.disk_io_val = self._row(body, "Диск I/O")

        # network
        self.net_val = self._row(body, "Сеть")

        # temperatures
        self.temp_val = self._row(body, "Темп.")
        self.temp_hint = tk.Label(body, text="", font=self.font_small, bg=BG, fg=DIM,
                                  anchor="w", justify="left", wraplength=inner_w)
        self.temp_hint.pack(fill="x")

        # battery (laptops only)
        self.batt_val = self.batt_bar = None
        if psutil.sensors_battery() is not None:
            self.batt_val = self._row(body, "Батарея")
            self.batt_bar = self._bar(body, inner_w, bar_h)

        self._build_menu()
        self._drag = (0, 0)
        root.bind_all("<ButtonPress-1>", self._drag_start)
        root.bind_all("<B1-Motion>", self._drag_move)
        root.bind_all("<ButtonRelease-1>", self._drag_end)
        root.bind_all("<Button-3>", self._show_menu)

        # prime counters so the first readings are meaningful
        psutil.cpu_percent(interval=None)
        self.last_net = psutil.net_io_counters()
        self.last_disk = psutil.disk_io_counters()
        self.last_t = time.monotonic()

        self.lhm = LhmReader()
        self.lhm.start()

        self.update()
        root.update_idletasks()
        self._place()

    # --- building blocks ---
    def _row(self, parent, name):
        row = tk.Frame(parent, bg=BG)
        row.pack(fill="x", pady=(int(7 * self.scale), int(2 * self.scale)))
        tk.Label(row, text=name, font=self.font_bold, bg=BG, fg=DIM).pack(side="left")
        value = tk.Label(row, text="…", font=self.font, bg=BG, fg=FG)
        value.pack(side="right")
        return value

    def _bar(self, parent, width, height):
        bar = Bar(parent, width, height)
        bar.canvas.pack()
        return bar

    def _build_menu(self):
        self.topmost_var = tk.BooleanVar(value=bool(self.cfg["topmost"]))
        self.alpha_var = tk.DoubleVar(value=float(self.cfg["alpha"]))
        menu = tk.Menu(self.root, tearoff=0)
        menu.add_checkbutton(label="Поверх всех окон", variable=self.topmost_var,
                             command=self._toggle_topmost)
        alpha_menu = tk.Menu(menu, tearoff=0)
        for pct in (100, 95, 90, 80, 70, 60):
            alpha_menu.add_radiobutton(label=f"{pct}%", value=pct / 100,
                                       variable=self.alpha_var, command=self._set_alpha)
        menu.add_cascade(label="Прозрачность", menu=alpha_menu)
        menu.add_command(label="В правый верхний угол", command=self._reset_position)
        menu.add_separator()
        menu.add_command(label="Закрыть", command=self.quit)
        self.menu = menu

    # --- placement & dragging ---
    def _place(self):
        h = self.root.winfo_reqheight()
        left, top, vw, vh = virtual_screen(self.root)
        x, y = self.cfg.get("x"), self.cfg.get("y")
        inside = (isinstance(x, int) and isinstance(y, int)
                  and left <= x <= left + vw - 40 and top <= y <= top + vh - 40)
        if not inside:
            x = self.root.winfo_screenwidth() - self.width - int(20 * self.scale)
            y = int(40 * self.scale)
        self.root.geometry(f"{self.width}x{h}+{x}+{y}")

    def _reset_position(self):
        self.cfg["x"] = self.cfg["y"] = None
        self._place()
        self._save_position()

    def _drag_start(self, event):
        self._drag = (event.x_root - self.root.winfo_x(), event.y_root - self.root.winfo_y())

    def _drag_move(self, event):
        dx, dy = self._drag
        self.root.geometry(f"+{event.x_root - dx}+{event.y_root - dy}")

    def _drag_end(self, event):
        self._save_position()

    def _save_position(self):
        self.cfg["x"], self.cfg["y"] = self.root.winfo_x(), self.root.winfo_y()
        save_config(self.cfg)

    # --- menu actions ---
    def _show_menu(self, event):
        try:
            self.menu.tk_popup(event.x_root, event.y_root)
        finally:
            self.menu.grab_release()

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

    # --- data refresh ---
    def update(self):
        try:
            self._refresh()
        except Exception:  # a single bad reading must not kill the widget
            pass
        self.root.after(UPDATE_MS, self.update)

    def _refresh(self):
        now = time.monotonic()
        dt = max(now - self.last_t, 0.001)
        self.last_t = now

        self.uptime.config(text=fmt_uptime(time.time() - psutil.boot_time()))

        cpu = psutil.cpu_percent(interval=None)
        self.cpu_val.config(text=f"{cpu:.0f}%  ·  {psutil.cpu_count()} потоков")
        self.cpu_bar.set(cpu)
        self.cpu_hist = (self.cpu_hist + [cpu])[-HISTORY_LEN:]
        self.cpu_graph.set(self.cpu_hist)

        mem = psutil.virtual_memory()
        self.ram_val.config(text=f"{fmt_gb(mem.used)} / {fmt_gb(mem.total)} ГБ  ·  {mem.percent:.0f}%")
        self.ram_bar.set(mem.percent)

        for mount, label, bar in self.disks:
            try:
                du = psutil.disk_usage(mount)
            except OSError:
                continue
            label.config(text=f"{fmt_gb(du.used)} / {fmt_gb(du.total)} ГБ  ·  {du.percent:.0f}%")
            bar.set(du.percent)

        disk = psutil.disk_io_counters()
        if disk and self.last_disk:
            r = (disk.read_bytes - self.last_disk.read_bytes) / dt
            w = (disk.write_bytes - self.last_disk.write_bytes) / dt
            self.disk_io_val.config(text=f"чт {fmt_speed(r)}  ·  зап {fmt_speed(w)}")
        self.last_disk = disk

        net = psutil.net_io_counters()
        down = (net.bytes_recv - self.last_net.bytes_recv) / dt
        up = (net.bytes_sent - self.last_net.bytes_sent) / dt
        self.net_val.config(text=f"↓ {fmt_speed(down)}   ↑ {fmt_speed(up)}")
        self.last_net = net

        self._refresh_temps()

        if self.batt_val is not None:
            batt = psutil.sensors_battery()
            if batt is not None:
                plug = " ⚡" if batt.power_plugged else ""
                self.batt_val.config(text=f"{batt.percent:.0f}%{plug}")
                color = GREEN if batt.percent > 30 else YELLOW if batt.percent > 15 else RED
                self.batt_bar.set(batt.percent, color)

    def _refresh_temps(self):
        cpu_t, gpu_t = self.lhm.cpu, self.lhm.gpu
        parts = []
        if cpu_t is not None:
            parts.append(f"CPU {cpu_t:.0f}°")
        if gpu_t is not None:
            parts.append(f"GPU {gpu_t:.0f}°")
        if parts:
            hottest = max(t for t in (cpu_t, gpu_t) if t is not None)
            self.temp_val.config(text="   ".join(parts), fg=temp_color(hottest))
            hint = ""
        else:
            self.temp_val.config(text="—", fg=DIM)
            hint = ("Нужен LibreHardwareMonitor:\nOptions → Remote Web Server → Run"
                    if not self.lhm.online else "LibreHardwareMonitor не видит датчики (запусти от админа)")
        if self.temp_hint.cget("text") != hint:
            self.temp_hint.config(text=hint)
            self.temp_hint.pack_configure(pady=(int(2 * self.scale), 0) if hint else 0)
            self.root.update_idletasks()
            self.root.geometry(f"{self.width}x{self.root.winfo_reqheight()}")

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    Widget().run()

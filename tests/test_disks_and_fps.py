"""SSD health, low disk space, closing programs from the list, the FPS panel's
corner, size and hotkey, video memory and GPU clock from the sensors."""
import struct

from conftest import fake_data
from test_ui import FakeToast, widget  # noqa: F401  (the fixture)


def health_log(temp_c=32, spare=100, spare_min=10, used=3, written_units=75907000, hours=2101,
               critical=0):
    log = bytearray(512)
    log[0] = critical
    struct.pack_into("<H", log, 1, temp_c + 273)
    log[3], log[4], log[5] = spare, spare_min, used
    log[48:64] = written_units.to_bytes(16, "little")
    log[128:144] = hours.to_bytes(16, "little")
    return bytes(log)


def test_nvme_health_log(w):
    h = w.parse_nvme_health(health_log())
    assert h == {"critical": 0, "temp": 32, "spare": 100, "spare_min": 10, "wear": 3,
                 "written": 75907000 * 512_000, "hours": 2101}
    assert w.parse_nvme_health(health_log(used=180))["wear"] == 100  # can go past 100 %
    assert w.parse_nvme_health(health_log(temp_c=-273))["temp"] is None  # 0 K = no sensor
    assert w.parse_nvme_health(b"\0" * 100) is None
    assert w.fmt_written(38.86e12) == "38,9 ТБ" and w.fmt_written(512e9) == "512 ГБ"


def test_ssd_row_drops_what_does_not_fit(w):
    r = w.Renderer(1.0)
    drive = fake_data()["drives"][0]
    data = fake_data()
    assert r.render(data, "heat").height == r.layout_height(data)
    assert r.layout_height(data) - r.layout_height(data, hidden=("ssd",)) == r.px(38)
    assert r.layout_height(fake_data(drives=[drive, dict(drive, drive=1, letters=["D:"])])) == \
        r.layout_height(data) + r.px(38)
    for lang in ("ru", "en", "uk", "de"):  # long words: the wear always stays on the row
        w.set_language(lang)
        texts = []
        r._draw_ssd(r_draw(r), texts, 0, dict(drive, critical=1), w.theme_scale("heat"))
        right = [t for kind, *rest in texts if kind == "rich" for t, _, _ in rest[2]]
        assert "3%" in right and w.tr("неисправен") + "   " in right


def r_draw(r):
    from PIL import Image, ImageDraw
    return ImageDraw.Draw(Image.new("RGBA", (r.px(300) * w_ss(), r.px(60) * w_ss())))


def w_ss():
    import conftest
    return conftest.W.SS


def test_read_lhm_gpu_clock_and_video_memory(w):
    tree = {"Text": "Sensor", "Children": [{"Text": "PC", "Children": [
        {"Text": "AMD Radeon(TM) Graphics", "ImageURL": "images_icon/ati.png", "Children": [
            {"Text": "Clocks", "Children": [{"Text": "GPU Core", "Value": "1800.0 MHz"}]},
            {"Text": "SmallData", "Children": [
                {"Text": "D3D Dedicated Memory Used", "Value": "273.9 MB"},
                {"Text": "D3D Dedicated Memory Total", "Value": "2031.6 MB"}]},
            {"Text": "Load", "Children": [{"Text": "GPU Core", "Value": "12.0 %"}]}]}]}]}
    r = w.read_lhm(tree)
    assert r["gpu_clock"] == 1800.0 and r["vram_used"] == 273.9 and r["vram_total"] == 2031.6
    # the driver's own numbers win over Windows' view; GB values come in MB
    tree["Children"][0]["Children"][0]["Children"].append({"Text": "Data", "Children": [
        {"Text": "GPU Memory Used", "Value": "1.5 GB"}, {"Text": "GPU Memory Total", "Value": "8 GB"}]})
    r = w.read_lhm(tree)
    assert r["vram_used"] == 1536.0 and r["vram_total"] == 8192.0


def test_disk_space_and_ssd_alerts(w, widget, monkeypatch):
    monkeypatch.setenv("SystemDrive", "C:")
    widget.toast = FakeToast()
    widget.cfg.update(alerts=True, alert_disk=10, quiet_games=False)
    total = 476 * 2**30
    full = fake_data(disks=[("C:", total * 0.95, total, 95.0), ("D:", 0, total, 99.0)], battery=None)
    widget._check_alerts(full)
    assert widget.toast.shown == []  # must last 10 s
    widget.alert_state["disk_space"]["since"] -= 11
    widget._check_alerts(full)
    titles = [t for t, _, _ in widget.toast.shown]
    assert titles == ["Мало места на диске C:"] and "23,8 ГБ (5%)" in widget.toast.shown[0][1]
    worn = dict(fake_data()["drives"][0], wear=93)
    widget._check_alerts(fake_data(drives=[worn], battery=None))
    assert widget.toast.shown[-1][0] == "SSD C: изношен" and "93%" in widget.toast.shown[-1][1]
    failing = dict(worn, wear=5, spare=4, spare_min=10, drive=1, letters=["D:"])
    widget._check_alerts(fake_data(drives=[failing], battery=None))
    assert widget.toast.shown[-1][0] == "SSD D: сообщает о неисправности"


def test_windows_itself_is_never_offered_for_closing(w, monkeypatch):
    assert not w.can_close("svchost") and not w.can_close("CSRSS") and not w.can_close("explorer")
    assert w.can_close("chrome")
    ended = []

    class Proc:
        def __init__(self, pid):
            self.pid = pid

        def name(self):
            return {11: "chrome.exe", 12: "chrome.exe", 13: "notepad.exe", 14: "chrome.exe"}[self.pid]

        def username(self):
            return "PC\\other" if self.pid == 14 else "PC\\vlad"

        def terminate(self):
            ended.append(self.pid)

    class Me(Proc):
        def __init__(self, pid=None):
            super().__init__(99)

    class Sampler:
        pids = {"chrome": [11, 12, 13, 14]}

    monkeypatch.setattr(w, "_sampler", Sampler())
    monkeypatch.setattr(w.psutil, "Process", lambda pid=None: Me() if pid is None else Proc(pid))
    # 13 is another program by now, 14 belongs to another user
    assert w.close_program("chrome") == 2 and ended == [11, 12]
    assert w.close_program("svchost") == 0


def test_closing_a_program_takes_two_clicks(w, widget, monkeypatch):
    closed = []
    monkeypatch.setattr(w, "close_program", lambda name: closed.append(name) or 1)
    widget.renderer, widget.data = w.Renderer(1.0), fake_data(procs=[("chrome", 12.0, 2**30)])
    widget.close_hover = widget.pin_hover = widget.close_armed = False
    widget.view, widget.photo, widget.height = w.tk.Label(widget.root), None, None
    widget.toast, widget.proc_armed = FakeToast(), None
    widget._draw = lambda: None
    widget._close_program_clicked("chrome")
    assert closed == [] and widget.proc_armed == "chrome"
    widget._disarm_program("chrome")  # 3 s later
    assert widget.proc_armed is None
    widget._close_program_clicked("chrome")
    widget._close_program_clicked("chrome")
    assert closed == ["chrome"] and widget.proc_armed is None
    # drawn: the armed row asks, and only closable programs get a ×
    r = w.Renderer(1.0)
    r.render(fake_data(procs=[("chrome", 12.0, 2**30), ("svchost", 3.0, 2**28)], procs_armed="chrome"),
             "heat")
    assert [name for name, _ in r.proc_boxes] == ["chrome"]
    r.render(fake_data(), "heat")
    assert r.proc_boxes == []  # list closed: no stale buttons


def test_fps_panel_corner_and_size(w, widget):
    widget.dpi_scale = 1.0
    ov = w.FpsOverlay(widget)
    rect = (100, 50, 1700, 950)
    for corner, spot in (("tl", (112, 62)), ("tr", (1700 - 200 - 12, 62)),
                         ("bl", (112, 950 - 80 - 12)), ("br", (1700 - 200 - 12, 950 - 80 - 12))):
        widget.cfg["fps_corner"] = corner
        assert ov.place(rect, 200, 80) == spot
    small = ov.render((60.0, 50.0, [16.7] * 30), w.theme_scale("heat"), {})
    widget.cfg["fps_size"] = 150
    ov.rescale()
    big = ov.render((60.0, 50.0, [16.7] * 30), w.theme_scale("heat"), {})
    assert big.height > small.height * 1.4


def test_fps_hotkey_hides_only_the_panel(w, widget, monkeypatch):
    shown = []

    class Overlay:
        visible = False

        def show(self, *a):
            shown.append(True)

        def hide(self):
            shown.append(False)

    class Fps:
        running = True

        def stats(self, pid):
            return (60.0, 50.0, [16.7] * 30)

    monkeypatch.setattr(w, "_user32", type("U", (), {"GetForegroundWindow": staticmethod(lambda: 5)}))
    monkeypatch.setattr(w, "window_pid", lambda hwnd: 4242)
    monkeypatch.setattr(w, "process_name", lambda pid: "stalcraft")
    monkeypatch.setattr(w, "fullscreen_app_active", lambda: True)
    monkeypatch.setattr(w, "window_rect", lambda hwnd: (0, 0, 1600, 900))
    widget.fps, widget.fps_overlay, widget.fps_streak = Fps(), Overlay(), (None, 0)
    widget.data, widget.ping = fake_data(), None
    widget._update_fps_overlay()
    assert shown == [True] and widget.game_fps
    widget.fps_panel_var.set(False)
    widget._toggle_fps_panel()
    assert shown == [True, False] and widget.game_fps and widget.cfg["fps_panel"] is False
    assert widget._game_in_front()  # the widget still knows a game is running

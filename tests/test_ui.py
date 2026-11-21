"""Menu, settings window, alerts and update notices on a Widget built without its
__init__ (no threads, no taskbar changes, no sensors)."""
import tkinter as tk

import pytest


@pytest.fixture
def widget(w, monkeypatch):
    try:
        root = tk.Tk()
    except tk.TclError:
        pytest.skip("no display")
    root.withdraw()
    monkeypatch.setattr(w, "save_config", lambda cfg: None)
    monkeypatch.setattr(w, "autostart_enabled", lambda name=None: False)
    monkeypatch.setattr(w, "admin_task_enabled", lambda: False)
    wid = w.Widget.__new__(w.Widget)
    wid.cfg = w.sanitize_config({})
    wid.root, wid.data, wid.alert_state, wid.show_procs = root, None, {}, False
    wid.hwnd, wid.taskbar, wid.settings = 1, None, None
    wid._build_menu()
    yield wid
    root.destroy()


class FakeToast:
    def __init__(self):
        self.shown = []

    def show(self, title, body, color, on_click=None):
        self.shown.append((title, body, on_click))


def test_short_menu(widget):
    m = widget.menu
    labels = [m.entrycget(i, "label") for i in range(m.index("end") + 1) if m.type(i) != "separator"]
    assert labels[0] == "Настройки…" and labels[-1] == "Закрыть"


def test_settings_window_builds_in_every_language(w, widget, monkeypatch):
    monkeypatch.setattr(w, "work_area", lambda: (0, 0, 1600, 860))
    for lang in ("ru", "en", "uk", "de"):
        w.set_language(lang)
        win = w.SettingsWindow(widget, tab=4)
        assert win.alive() and win.current_tab() == 4
        win.close()
        assert not win.alive()


def test_sections_and_limits(widget):
    widget.section_vars["disks"].set(False)
    widget._toggle_section("disks")
    assert widget.cfg["hidden"] == ["disks"]
    widget.limit_vars["alert_temp"].set(80)
    widget._set_limit("alert_temp")
    assert widget.cfg["alert_temp"] == 80


def test_alerts_use_the_chosen_limits(widget):
    from conftest import fake_data
    widget.toast = FakeToast()
    widget.cfg.update(alerts=True, alert_temp=80, alert_battery=20)
    widget._check_alerts(fake_data(temp_cpu=82.0, battery=(18.0, False), battery_left=50 * 60))
    assert any("примерно на 50 мин" in body for _, body, _ in widget.toast.shown)
    widget.alert_state["cpu_temp"]["since"] -= 10
    widget._check_alerts(fake_data(temp_cpu=82.0))
    assert "Процессор перегревается" in [t for t, _, _ in widget.toast.shown]


def test_update_notice_once_per_version(w, widget, monkeypatch):
    class Checker:
        found, checks, failed = None, 0, False

    widget.toast, widget.updates, widget.manual_check, widget.updater = FakeToast(), Checker(), None, None
    opened = []
    monkeypatch.setattr(w, "open_in_browser", opened.append)
    monkeypatch.setattr(w, "update_target", lambda: None)
    widget._update_notice()
    assert widget.toast.shown == []
    widget.updates.found = ("1.9", w.RELEASES_URL + "/tag/v1.9", None)
    widget._update_notice()
    widget._update_notice()
    assert [t for t, _, _ in widget.toast.shown] == ["Вышла версия 1.9"]
    widget.toast.shown[0][2]()  # no asset / can't update here -> opens the page
    assert opened == [w.RELEASES_URL + "/tag/v1.9"]


def test_in_games_hide_dim_and_override(w, widget, monkeypatch):
    state = {"fg": 2, "full": True}

    class U:
        GetForegroundWindow = staticmethod(lambda: state["fg"])

    monkeypatch.setattr(w, "_user32", U)
    monkeypatch.setattr(w, "fullscreen_app_active", lambda: state["full"])
    widget.fps_overlay, widget.visible = None, True
    widget.root.deiconify()  # a withdrawn window doesn't move
    widget.root.geometry("100x50+300+200")
    widget.root.update()
    alpha = lambda: round(float(widget.root.attributes("-alpha")), 2)
    widget.cfg.update(in_games="hide", alpha=0.9)
    widget._apply_dim()  # a fullscreen game in front: parked off-screen
    widget.root.update()
    assert widget.game_hidden_at == (300, 200) and widget.root.winfo_x() < -1000
    widget._save_position()
    assert widget.cfg["x"] != widget.root.winfo_x()  # the parked spot is never saved
    state["full"] = False
    widget._apply_dim()  # the game is gone: back in place
    widget.root.update()
    assert widget.game_hidden_at is None and widget.root.winfo_x() == 300
    state["full"] = True
    widget.cfg["in_games"] = "dim"
    widget._apply_dim()
    assert alpha() == 0.3 and widget.root.winfo_x() == 300
    widget.cfg["in_games"] = "off"
    widget._apply_dim()
    assert alpha() == 0.9
    widget.cfg["in_games"] = "hide"
    widget._apply_dim()
    widget.show_widget()  # Ctrl+Alt+W during the game: shown and stays
    widget._apply_dim()
    widget.root.update()
    assert widget.root.winfo_x() == 300 and widget.game_override == 2
    state["fg"] = 3  # another window in front: the override ends
    widget._apply_dim()
    assert widget.game_override is None


def test_fps_pill_second_row(w, widget):
    from conftest import fake_data

    class Ping:
        ms, ready = 24, True

    widget.data, widget.ping = fake_data(cpu=45.0, temp_cpu=66.0, temp_gpu=None, gpu_shared=True,
                                         gpu_load=23.0), Ping()
    info = widget._overlay_info()
    assert info == {"cpu": 45.0, "cpu_t": 66.0, "gpu": 23.0, "gpu_t": 66.0, "gpu_approx": True, "ping": 24}
    widget.renderer = w.Renderer(1.0)  # the stub has no __init__
    ov = w.FpsOverlay(widget)
    one_row = ov.render((60.0, 50.0, [16.7] * 30), w.theme_scale("heat"), {})
    two_rows = ov.render((60.0, 50.0, [16.7] * 30), w.theme_scale("heat"), info)
    assert two_rows.height > one_row.height
    texts = [t for t, _, _ in ov.system_parts(info, w.theme_scale("heat"))]
    assert "≈" in texts and "24" in texts and "45%" in texts


@pytest.fixture
def live(w, widget, monkeypatch):
    """The stub widget plus what _apply_view needs."""
    widget.dpi_scale, widget.taskbar, widget.height = 1.0, None, None
    widget.close_hover = widget.pin_hover = widget.close_armed = False
    widget.renderer = w.Renderer(1.0)
    widget.view = tk.Label(widget.root)
    widget.photo = None
    monkeypatch.setattr(widget, "_apply_pin", lambda: None)
    return widget


def test_profiles_keep_their_own_look(w, live):
    from conftest import fake_data
    live.data = fake_data()
    live.cfg.update(compact=False, size=100, theme="heat")
    live.switch_profile("game")  # first time: the game look is the compact widget
    assert live.cfg["profile"] == "game" and live.cfg["compact"] is True
    live.cfg["theme"] = "neon"  # change the game look
    live.switch_profile("work")
    assert live.cfg["compact"] is False and live.cfg["theme"] == "heat"
    live.switch_profile("game")
    assert live.cfg["compact"] is True and live.cfg["theme"] == "neon"
    assert live.compact_var.get() is True and live.profile_var.get() == "game"
    assert w.sanitize_config({"profiles": {"game": {"compact": True, "evil": 1}, "x": {}}})["profiles"] == \
        {"game": {"compact": True}}


def test_game_switches_profile_on_and_back(w, live, monkeypatch):
    from conftest import fake_data
    live.data = fake_data()
    clock = {"t": 1000.0}
    monkeypatch.setattr(w.time, "monotonic", lambda: clock["t"])
    state = {"game": False}
    monkeypatch.setattr(live, "_game_in_front", lambda: state["game"])
    state["game"] = True
    live._auto_profile()
    assert live.cfg["profile"] == "work"  # not yet: a game must stay in front a moment
    clock["t"] += w.GAME_ON_S
    live._auto_profile()
    assert live.cfg["profile"] == "game"
    state["game"] = False
    live._auto_profile()
    clock["t"] += w.GAME_OFF_S
    live._auto_profile()
    assert live.cfg["profile"] == "work"


def test_quiet_in_games(w, widget, monkeypatch):
    from conftest import fake_data
    shown = []

    class Toast:
        def show(self, *a, **k):
            shown.append(a)

    widget.toast = Toast()
    state = {"game": True}
    monkeypatch.setattr(widget, "_game_in_front", lambda: state["game"])
    widget.cfg.update(alerts=True, quiet_games=True)
    low = fake_data(battery=(10.0, False), battery_left=None)
    widget._check_alerts(low)
    widget._check_alerts(low)
    assert not shown  # no card over the game
    state["game"] = False
    widget._check_alerts(low)
    assert len(shown) == 1 and "Батарея" in shown[0][0]  # shown once the game is closed
    widget._check_alerts(low)
    assert len(shown) == 1

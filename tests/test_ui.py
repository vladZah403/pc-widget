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

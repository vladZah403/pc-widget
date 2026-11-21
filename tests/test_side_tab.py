"""The side tab: the widget slides behind the nearer screen edge and comes back."""
import time

import pytest

from test_resize_and_ux import Ev, ui  # noqa: F401  (fixtures)
from test_ui import live, widget  # noqa: F401  (fixtures)

TW, TH = 14, 60  # the tab at Windows' scale 100%


@pytest.fixture
def tab(w, ui, monkeypatch):
    """`ui` (a roomy 3000 x 3000 screen) with a side tab."""
    monkeypatch.setattr(w, "other_monitor_at", lambda *a: False)
    ui.ui_renderer = w.Renderer(1.0)
    ui.side_tab = w.SideTab(ui)
    ui.tucked = ui.sliding = False
    ui.game_hidden_at, ui._handle_job = None, None
    return ui.side_tab


def at(ui, x, y):
    ui.root.geometry(f"+{x}+{y}")
    ui.root.update()


def settle(ui):
    """Let a slide finish."""
    end = time.monotonic() + 3
    while ui.sliding and time.monotonic() < end:
        ui.root.update()
        time.sleep(0.005)
    ui.root.update()
    assert not ui.sliding


def click(tab, dy=0):
    tab._press(Ev(5, 5, (tab.x, tab.y)))
    if dy:
        tab._drag(Ev(5, 5 + dy, (tab.x, tab.y)))
    tab._release(Ev(5, 5 + dy, (tab.x, tab.y)))


def tick(ui):
    """One check of the mouse by the side tab's watcher."""
    if ui._handle_job is not None:
        ui.root.after_cancel(ui._handle_job)
        ui._handle_job = None
    ui._watch_handle()


def test_tab_is_flat_on_the_edge_side(w, tab):
    tab.edge, tab.docked = "right", False
    img = tab.render()
    assert img.size == (TW, TH)
    assert img.getpixel((0, 0)) == w.KEY and img.getpixel((TW - 1, 0)) != w.KEY  # rounded left
    tab.edge = "left"
    img = tab.render()
    assert img.getpixel((TW - 1, 0)) == w.KEY and img.getpixel((0, 0)) != w.KEY  # rounded right
    attached = img.tobytes()
    tab.docked, tab.cpu = True, 90.0
    busy = tab.render().tobytes()
    tab.cpu = 5.0
    idle = tab.render().tobytes()
    assert len({attached, busy, idle}) == 3  # the arrow turns; docked it shows the CPU load


def test_tuck_behind_the_right_edge_and_back(w, ui, tab):
    at(ui, 2500, 400)
    ui._enter(Ev(10, 10, (2500, 400)))  # the mouse comes: the tab shows on the inner side
    H = ui.root.winfo_height()
    assert tab.visible and not tab.docked and tab.edge == "right"
    assert (tab.x, tab.y) == (2500 - TW, 400 + (H - TH) // 2)
    click(tab)
    settle(ui)
    assert ui.tucked and ui.cfg["tucked"] and not ui.visible and ui.root.state() == "withdrawn"
    assert tab.visible and tab.docked and (tab.x, tab.y) == (3000 - TW, 400 + (H - TH) // 2)
    assert {k: ui.saved[-1][k] for k in ("x", "y", "tucked")} == {"x": 2500, "y": 400, "tucked": True}
    click(tab)  # back
    settle(ui)
    assert not ui.tucked and not ui.cfg["tucked"] and ui.visible and ui.root.state() == "normal"
    assert (ui.root.winfo_x(), ui.root.winfo_y()) == (2500, 400)
    assert not tab.docked and tab.x == 2500 - TW and ui.saved[-1]["tucked"] is False


def test_left_half_goes_left_and_the_hotkey_brings_it_back(w, ui, tab):
    at(ui, 100, 300)
    ui._show_handle()
    assert tab.edge == "left" and tab.x == 100 + ui.root.winfo_width()
    ui.tuck(animate=False)
    assert tab.docked and tab.x == 0
    ui.show_widget()  # Ctrl+Alt+W or the tray icon
    settle(ui)
    assert not ui.tucked and ui.visible and (ui.root.winfo_x(), ui.root.winfo_y()) == (100, 300)


def test_dragging_the_tab_moves_where_the_widget_comes_back(w, ui, tab):
    at(ui, 2500, 400)
    ui.tuck(animate=False)
    y0 = tab.y
    click(tab, dy=300)  # a drag, not a click: it stays tucked
    assert ui.tucked and tab.y == y0 + 300
    click(tab)
    settle(ui)
    assert (ui.root.winfo_x(), ui.root.winfo_y()) == (2500, 700) and ui.cfg["y"] == 700
    ui.tuck(animate=False)
    click(tab, dy=-9000)  # never above the screen
    assert tab.y == 0
    ui.untuck(animate=False)
    assert ui.root.winfo_y() == 0


def test_the_tab_goes_when_the_mouse_leaves(w, ui, tab, monkeypatch):
    at(ui, 2500, 400)
    pointer = {"at": (2510, 410)}
    monkeypatch.setattr(ui.root, "winfo_pointerxy", lambda: pointer["at"])
    ui._show_handle()
    tick(ui)
    assert tab.visible
    pointer["at"] = (tab.x + 3, tab.y + 3)  # on the tab itself
    tick(ui)
    assert tab.visible
    pointer["at"] = (100, 100)  # away: gone after a moment, not at once
    tick(ui)
    assert tab.visible
    tick(ui)
    assert not tab.visible


def test_games_hide_the_tab_and_hide_clears_tucked(w, ui, tab, monkeypatch):
    at(ui, 2500, 400)
    ui.tuck(animate=False)
    full = {"on": True}

    class U:
        GetForegroundWindow = staticmethod(lambda: 12345)

    monkeypatch.setattr(w, "_user32", U)
    monkeypatch.setattr(w, "fullscreen_app_active", lambda: full["on"])
    ui.fps_overlay, ui.cfg["in_games"] = None, "hide"
    ui._apply_dim()  # a fullscreen game: the tab steps aside
    assert not tab.visible and ui.tucked
    full["on"] = False
    ui._apply_dim()
    assert tab.visible and tab.docked
    ui.hide_widget()  # "Скрыть" in the menu: gone completely, not tucked any more
    assert not tab.visible and not ui.tucked and not ui.cfg["tucked"]
    assert w.sanitize_config({})["tucked"] is False and w.sanitize_config({"tucked": 1})["tucked"] is False

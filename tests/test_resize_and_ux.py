"""Resizing by the corner, Ctrl + wheel and the slider; sticking to screen edges;
rows that open their history; hints by the mouse."""
import math
import tkinter as tk

import pytest

from conftest import fake_data
from test_ui import live, widget  # noqa: F401  (fixtures)


class Ev:
    def __init__(self, x, y, root=(0, 0), state=0, delta=0):
        self.x, self.y = x, y
        self.x_root, self.y_root = root[0] + x, root[1] + y
        self.state, self.delta = state, delta


@pytest.fixture
def ui(w, live, monkeypatch):
    saved = []
    monkeypatch.setattr(w, "save_config", lambda cfg: saved.append(dict(cfg)))
    monkeypatch.setattr(w, "monitor_work_area", lambda x, y: (0, 0, 3000, 3000))  # roomy screen
    live.saved = saved
    live.tip = w.Tooltip(live.root)
    live.resizing, live.mouse_in, live.row_hover = None, False, None
    live.grip_hover = live.grip_pressed = False
    live.proc_pressed = live.close_pressed = live.pin_pressed = live.hero_pressed = False
    live.proc_armed, live.show_procs, live.visible = None, False, True
    live.cfg.update(pinned=False, compact=False, size=100, x=100, y=100)
    live.data = fake_data()
    live.root.deiconify()  # a withdrawn window has no real size
    live.root.geometry("+100+100")
    live._draw(force=True)
    live.root.update()
    return live


def center(box):
    return (box[0] + box[2]) // 2, (box[1] + box[3]) // 2


def test_size_is_any_step_between_80_and_200(w):
    for raw, want in ((125, 125), (333, 200), (77, 80), (112, 110), (113, 115), (True, 100),
                      ("big", 100), (math.nan, 100), (-5, 80)):
        assert w.sanitize_config({"size": raw})["size"] == want, raw


def test_corner_and_rows_are_drawn_where_they_are_hit(w):
    r = w.Renderer(1.0)
    data = fake_data()
    plain = r.render(data, "heat")
    assert r.grip_box == (0, 0, 0, 0)
    subjects = [s for s, _ in r.row_boxes]
    assert subjects == [0, 2, 1, 0, 3]  # cores, memory, GPU, temperature, network (+ its graph)
    net = dict((s, b) for s, b in r.row_boxes)[3]
    assert net[3] - net[1] > r.px(40)  # the network label and its graph are one row
    with_corner = r.render(data, "heat", grip=1)
    x0, y0, x1, y1 = r.grip_box
    assert (x1, y1) == with_corner.size and x1 - x0 == r.px(w.GRIP)
    assert plain.tobytes() != with_corner.tobytes()
    lit = r.render(data, "heat", hover_row=2)  # memory row highlighted
    assert lit.tobytes() != plain.tobytes() and lit.size == plain.size
    r.render(data, "heat", compact=True, grip=1)
    assert r.grip_box[3] == r.layout_height(data, compact=True) and r.row_boxes == []


def test_snap_to_screen_edges(w):
    area = (0, 0, 1920, 1040)
    assert w.snap_position(5, 500, 264, 700, area, 10, 16) == (10, 500)
    assert w.snap_position(40, 500, 264, 700, area, 10, 16) == (40, 500)  # too far: stays
    assert w.snap_position(1920 - 264 - 4, 330, 264, 700, area, 10, 16) == (1920 - 264 - 10, 330)
    assert w.snap_position(800, -3, 264, 700, area, 10, 16) == (800, 10)
    assert w.snap_position(800, 1040 - 700 - 25, 264, 700, area, 10, 16) == (800, 1040 - 700 - 10)


def test_shape_sanitized_and_drawn(w):
    for raw, want in ((400, 400), (100, 264), (5000, 640), ("x", 264), (True, 264)):
        assert w.sanitize_config({"width": raw})["width"] == want, raw
    for raw, want in ((120, 120), (-3, 0), (9999, 600), (math.inf, 0)):
        assert w.sanitize_config({"stretch": raw})["stretch"] == want, raw
    data = fake_data()
    std, shaped = w.Renderer(1.0), w.Renderer(1.0, 400, 200)
    a, b = std.render(data, "heat"), shaped.render(data, "heat", grip=1)
    assert a.size == (264, std.layout_height(data)) and b.size == (400, a.height + 200)
    assert shaped.hero_h(data) == w.HERO_H + 120  # the CPU graph gets 3/5, the network graph 2/5
    net = lambda r: [bottom - top for s, top, bottom in r._row_spans(data, ()) if s == 3][0]
    assert net(shaped) == net(std) + 80
    assert shaped.layout_height(data, compact=True) == std.layout_height(data, compact=True) + 200
    assert shaped.layout_height(data, hidden=("net_graph",)) - std.layout_height(data, hidden=("net_graph",)) == 200


def drag(ui, start, delta, root):
    ui._press(Ev(*start, root))
    ui._drag_move(Ev(start[0] + delta[0], start[1] + delta[1], root))
    ui.root.update()  # the new shape is drawn when Tk is idle
    ui._release(Ev(start[0] + delta[0], start[1] + delta[1], root))
    ui.root.update()


def test_drag_edges_and_corner_to_change_the_shape(w, ui):
    root = (ui.root.winfo_x(), ui.root.winfo_y())
    ui._enter(Ev(10, 10, root))
    ui.root.update()
    vw, vh = ui.root.winfo_width(), ui.root.winfo_height()
    assert ui._edge_at(Ev(vw - 2, vh // 2)) == "x" and ui._edge_at(Ev(vw // 2, vh - 2)) == "y"
    assert ui._edge_at(Ev(*center(ui.renderer.grip_box))) == "xy" and ui._edge_at(Ev(vw // 2, 50)) is None
    drag(ui, (vw - 2, vh // 2), (100, 40), root)  # right edge: only the width
    assert (ui.cfg["width"], ui.cfg["stretch"], ui.cfg["size"]) == (364, 0, 100)
    assert ui.root.winfo_width() == 364 and ui.root.winfo_height() == vh
    drag(ui, (ui.root.winfo_width() // 2, vh - 2), (30, 120), root)  # bottom edge: only the height
    assert (ui.cfg["width"], ui.cfg["stretch"]) == (364, 120) and ui.root.winfo_height() == vh + 120
    assert ui.saved[-1]["width"] == 364 and ui.saved[-1]["stretch"] == 120
    drag(ui, center(ui.renderer.grip_box), (-2000, -2000), root)  # never below the standard shape
    assert (ui.cfg["width"], ui.cfg["stretch"]) == (w.WIDTH_STD, 0)
    drag(ui, center(ui.renderer.grip_box), (50, 60), root)  # the corner: both
    assert (ui.cfg["width"], ui.cfg["stretch"]) == (314, 60) and ui.renderer.s == 1.0  # text as it was
    assert (ui.root.winfo_x(), ui.root.winfo_y()) == root
    ui._hover(Ev(ui.root.winfo_width() - 2, 200, root))
    assert ui.view.cget("cursor") == "size_we"
    ui._hover(Ev(100, ui.root.winfo_height() - 2, root))
    assert ui.view.cget("cursor") == "size_ns"


def test_a_grown_widget_moves_back_above_the_taskbar(w, ui, monkeypatch):
    monkeypatch.setattr(w, "monitor_work_area", lambda x, y: (0, 0, 1600, 1040))
    ui.cfg.update(x=1400, y=300)
    ui.root.geometry("+1400+300")
    ui.root.update()
    ui._set_size(150)  # 396 x ~880 px: sticks out to the right and below
    ui.root.update()
    x, y = ui.root.winfo_x(), ui.root.winfo_y()
    assert x + ui.root.winfo_width() <= 1600 and y + ui.root.winfo_height() <= 1040
    assert (x, y) != (1400, 300) and (ui.cfg["x"], ui.cfg["y"]) == (x, y)  # remembered


def test_no_row_stays_lit_after_resizing(w, ui):
    root = (ui.root.winfo_x(), ui.root.winfo_y())
    ui._enter(Ev(10, 10, root))
    ui._hover(Ev(*center(dict(ui.renderer.row_boxes)[2]), root))
    assert ui.row_hover == 2
    g = center(ui.renderer.grip_box)
    ui._press(Ev(*g, root))
    ui._drag_move(Ev(g[0] + 40, g[1] + 90, root))
    ui._release(Ev(g[0] + 40, g[1] + 90, root))
    assert ui.row_hover is None


def test_ctrl_wheel_and_double_click(w, ui):
    ui._wheel_size(Ev(5, 5, delta=120))
    assert ui.cfg["size"] == 110 and ui.size_var.get() == 110
    for _ in range(20):
        ui._wheel_size(Ev(5, 5, delta=120))
    assert ui.cfg["size"] == w.SIZE_MAX
    ui._wheel_size(Ev(5, 5, delta=-120))
    assert ui.cfg["size"] == 190
    ui.cfg.update(width=500, stretch=150)
    ui.renderer.set_shape(500, 150)
    ui.mouse_in = True
    ui._draw()
    ui._double_click(Ev(*center(ui.renderer.grip_box)))  # the standard shape, the size stays
    assert (ui.cfg["width"], ui.cfg["stretch"], ui.cfg["size"]) == (w.WIDTH_STD, 0, 190)
    assert ui.resizing is None
    ui._release(Ev(*center(ui.renderer.grip_box)))  # the second release does nothing else
    assert (ui.cfg["width"], ui.cfg["stretch"]) == (w.WIDTH_STD, 0)


def test_dragging_sticks_to_the_edges_unless_shift(w, ui, monkeypatch):
    monkeypatch.setattr(w, "monitor_work_area", lambda x, y: (0, 0, 1600, 900))
    ui._press(Ev(50, 15, (ui.root.winfo_x(), ui.root.winfo_y())))
    ui._drag_move(Ev(50, 15, (6, 300)))  # left edge 6 px off the screen side
    ui.root.update()
    assert ui.root.winfo_x() == round(w.SNAP_GAP * ui.dpi_scale)
    ui._drag_move(Ev(50, 15, (6, 300), state=0x1))  # Shift: exactly where the mouse puts it
    ui.root.update()
    assert ui.root.winfo_x() == 6


def test_a_click_on_a_row_opens_its_history(w, ui):
    calls = []

    class Card:
        visible, subject = False, 0

        def show_near(self, rect):
            self.visible = True
            calls.append(("show", self.subject))

        def hide(self):
            self.visible = False
            calls.append(("hide", self.subject))

        def refresh(self):
            calls.append(("refresh", self.subject))

    ui.history_card = Card()
    boxes = dict(ui.renderer.row_boxes)
    root = (ui.root.winfo_x(), ui.root.winfo_y())
    for subject in (2, 2, 3):  # memory, memory again (closes), network
        x, y = center(boxes[subject])
        ui._press(Ev(x, y, root))
        ui._release(Ev(x, y, root))
    ui._press(Ev(*center(boxes[1]), root))  # GPU while the card is open: it switches over
    ui._release(Ev(*center(boxes[1]), root))
    assert calls == [("show", 2), ("hide", 2), ("show", 3), ("refresh", 1)]
    # hovering a row highlights it and shows the hand
    ui._hover(Ev(*center(boxes[2]), root))
    assert ui.row_hover == 2 and ui.view.cget("cursor") == "hand2"
    ui._hover(Ev(*center(ui.renderer.grip_box), root))
    assert ui.row_hover is None and ui.view.cget("cursor") == "size_nw_se"
    ui._leave(None)
    assert ui.row_hover is None and not ui.mouse_in and ui.renderer.grip_box == (0, 0, 0, 0)


def test_hint_appears_after_a_moment_and_goes(w, ui, monkeypatch):
    monkeypatch.setattr(w.Tooltip, "DELAY_MS", 1)
    ui.tip.request("pin", "Закрепить на рабочем столе", 200, 200)
    ui.root.after(30, lambda: None)
    for _ in range(20):
        ui.root.update()
        if ui.tip.top is not None:
            break
        ui.root.after(5)
    assert ui.tip.top is not None and ui.tip.top.winfo_exists()
    ui.tip.request("pin", "same", 0, 0)  # the same control: the hint stays
    assert ui.tip.top is not None
    ui.tip.cancel()
    assert ui.tip.top is None and ui.tip.key is None


def test_settings_slider_follows_the_size(w, ui, monkeypatch):
    monkeypatch.setattr(w, "work_area", lambda: (0, 0, 1600, 860))
    win = w.SettingsWindow(ui, tab=1)
    ui._wheel_size(Ev(5, 5, delta=120))
    assert ui.size_var.get() == 110
    ui._set_size(132.4)  # the slider gives fractions: snapped to the step
    assert ui.cfg["size"] == 130 and ui.size_var.get() == 130
    win.close()
    ui.size_var.set(150)  # no error from a label that is gone
    ui.root.update()
    assert isinstance(ui.size_var, tk.DoubleVar)

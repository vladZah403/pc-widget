"""Checks from the bug, security and speed review: what comes from outside (GitHub,
the sensor web server, history.csv) is bounded, the FPS reader forgets ended
programs, and a hidden widget is not redrawn every second."""
import hashlib
import io
import zipfile

from conftest import fake_data
from test_install_and_update import _Resp
from test_ui import widget  # noqa: F401  (the fixture)


def test_only_our_release_page_is_opened(w):
    r = w.RELEASES_URL
    info = {"tag_name": "v9.0", "assets": []}
    assert w.newer_release(dict(info, html_url=r + "/tag/v9.0"), "1.6")[1] == r + "/tag/v9.0"
    for odd in (r + "/tag/v9.0?next=https://evil.example", r + "/download/v9/x.exe",
                r + "/tag/v9.0/../../../evil", "https://evil.example/releases/tag/v9.0", None):
        assert w.newer_release(dict(info, html_url=odd), "1.6")[1] == r + "/latest"


def test_updater_refuses_an_oversized_archive(w, tmp_path, monkeypatch):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("PC-Monitor/PC-Monitor.exe", b"e")
        zf.writestr("PC-Monitor/_internal/a.dll", b"d" * 5000)
    data = buf.getvalue()
    monkeypatch.setattr(w.urllib.request, "urlopen", lambda req, timeout=None: _Resp(data))
    asset = {"url": "https://example.invalid/PC-Monitor.zip",
             "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
    for limits in ({"UNPACK_MAX_BYTES": 4000}, {"UNPACK_MAX_FILES": 1}):
        with monkeypatch.context() as m:
            for name, value in limits.items():
                m.setattr(w, name, value)
            up = w.Updater(asset, (str(tmp_path / "App"), str(tmp_path / "App.new")))
            up.run()
            assert up.state == "error" and "в архиве нет программы" in up.error
    up = w.Updater(asset, (str(tmp_path / "App"), str(tmp_path / "App.new")))
    up.run()
    assert up.state == "ready", up.error


def test_sensor_web_server_answer_is_bounded(w, monkeypatch):
    monkeypatch.setattr(w, "LHM_MAX_BYTES", 1000)
    monkeypatch.setattr(w.urllib.request, "urlopen", lambda url, timeout=None: _Resp(b"[" * 5000))
    try:
        w._fetch_web()
        raise AssertionError("an oversized answer must be refused")
    except ValueError as exc:
        assert "too much data" in str(exc)
    tree = b'{"Text": "Sensor", "Children": []}'
    monkeypatch.setattr(w.urllib.request, "urlopen", lambda url, timeout=None: _Resp(tree))
    assert w._fetch_web()["cpu"] is None


def test_history_skips_nan_and_inf(w, tmp_path):
    import time
    now = time.time()
    path = tmp_path / "history.csv"
    path.write_text(f"{now - 50:.0f},nan,50.0,,,,,\n"
                    f"{now - 40:.0f},12.0,inf,-inf,nan,30.0,1.0,2.0\n"
                    f"nan,12.0,50.0,,,,,\n", encoding="utf-8")
    rows = w.History(str(path)).rows
    assert len(rows) == 1 and rows[0][1:] == (12.0, None, None, None, 30.0, 1.0, 2.0)


def test_fps_reader_forgets_programs_that_stopped_drawing(w):
    fps = w.FpsReader()
    for i in range(200):
        fps.add_frame(1, "krnl", 10 + i / 60)  # a game, then closed at ~13 s
    assert (1, "krnl") in fps.frames
    t = 10 + 200 / 60 + w.FpsReader.KEEP_S + w.FpsReader.PRUNE_EVERY_S + 1
    fps.add_frame(2, "dxgi", t)  # another program draws much later
    assert (1, "krnl") not in fps.frames and (2, "dxgi") in fps.frames
    # a long-running game keeps only its recent frames
    for i in range(60 * 60):
        fps.add_frame(3, "krnl", t + i / 60)
    assert len(fps.frames[(3, "krnl")]) < 60 * 2 * (w.FpsReader.KEEP_S + 1)


def test_hidden_widget_is_not_redrawn(w, widget, monkeypatch):
    renders = []

    class R(w.Renderer):
        def render(self, *a, **k):
            renders.append(1)
            return super().render(*a, **k)

    widget.renderer, widget.data = R(1.0), fake_data()
    widget.close_hover = widget.pin_hover = widget.close_armed = False
    widget.view, widget.photo, widget.height = w.tk.Label(widget.root), None, None
    widget.cfg["pinned"] = False
    widget.visible = True
    widget._draw()
    assert len(renders) == 1
    widget.hide_widget()
    for _ in range(3):
        widget._draw()  # the 1-second refresh while hidden
    assert len(renders) == 1
    widget.show_widget()
    assert len(renders) == 2  # drawn fresh before it appears
    widget.game_hidden_at = (10, 10)  # parked off-screen during a game
    widget._draw()
    assert len(renders) == 2
    widget.height = None  # a size change while hidden still gets its new height
    widget._draw()
    assert len(renders) == 3 and widget.height

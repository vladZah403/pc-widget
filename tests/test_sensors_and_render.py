"""Sensor parsing, drawing in every language, history, FPS maths."""
import pytest

from conftest import fake_data

TREE = {"Text": "Sensor", "Children": [{"Text": "PC", "Children": [
    {"Text": "AMD Ryzen 5 5500U", "ImageURL": "images_icon/cpu.png", "Children": [
        {"Text": "Clocks", "Children": [
            {"Text": "Bus Speed", "Value": "100.0 MHz"},
            {"Text": "Core #1", "Value": "3000.0 MHz"},
            {"Text": "Core #2", "Value": "3400.0 MHz"},
            {"Text": "Core #3", "Value": "0.0 MHz"}]},
        {"Text": "Powers", "Children": [
            {"Text": "Core #1 (SMU)", "Value": "2.1 W"},
            {"Text": "Package", "Value": "11.6 W"}]},
        {"Text": "Temperatures", "Children": [
            {"Text": "Core (Tctl/Tdie)", "Value": "61.5 °C", "SensorId": "/amdcpu/0/temperature/2"}]}]},
    {"Text": "NVIDIA GeForce RTX 3060", "ImageURL": "images_icon/nvidia.png", "Children": [
        {"Text": "Fans", "Children": [{"Text": "GPU Fan 1", "Value": "1450 RPM"}]}]},
    {"Text": "Battery", "ImageURL": "images_icon/battery.png", "Children": [
        {"Text": "Energy", "Children": [
            {"Text": "Designed Capacity", "Value": "42000 mWh"},
            {"Text": "Fully-Charged Capacity", "Value": "35290 mWh"}]},
        {"Text": "Powers", "Children": [{"Text": "Charge/Discharge Rate", "Value": "-8.2 W"}]}]},
]}]}


def test_read_lhm(w):
    r = w.read_lhm(TREE)
    assert set(r) == set(w.EMPTY_SENSORS)
    assert r["cpu"] == 61.5 and abs(r["cpu_clock"] - 3.2) < 1e-9 and r["cpu_power"] == 11.6
    assert r["fans"] == [1450.0] and r["battery_design"] == 42.0
    assert abs(r["battery_wear"] - 15.976) < 0.01 and r["battery_rate"] == 8.2
    assert w.read_lhm({"Text": "Sensor", "Children": []})["cpu"] is None


@pytest.mark.parametrize("lang", ["ru", "en", "uk", "de"])
@pytest.mark.parametrize("hidden", [(), ("gpu", "disks", "uptime"), None])
def test_window_height_matches_what_is_drawn(w, lang, hidden):
    w.set_language(lang)
    hidden = tuple(w.SECTIONS) if hidden is None else hidden
    r = w.Renderer(1.25)
    for data in (fake_data(), fake_data(procs=[("chrome", 12.3, 900 * 2**20)], update=("1.3", "u", None)),
                 fake_data(lhm_online=False, temp_cpu=None, temp_ssd=None, sensor_hint="x" * 200)):
        img = r.render(data, "heat", hidden=hidden)
        assert img.height == r.layout_height(data, False, hidden)
        assert img.width == r.W
    assert r.render(fake_data(), "custom", compact=True).height == r.px(178)


def test_history_store_and_buckets(w, tmp_path):
    path = str(tmp_path / "history.csv")
    now = 1_000_000.0
    h = w.History(path)
    h.last_trim = now
    for i in range(0, 30 * 3600, 10):
        h.add(10.0, 50.0 if i % 2 else None, None, now=now - 30 * 3600 + i)
    h.flush(now)
    assert all(r[0] >= now - w.HISTORY_KEEP_S for r in h.rows)
    with open(path, "a", encoding="utf-8") as f:
        f.write("garbage\n1,2\n")
    b = w.history_buckets([(0, 10, None, None), (5, 30, None, None), (95, 50, None, None)], 0, 100, 10, 1)
    assert b[0] == (20.0, 30) and b[9] == (50.0, 50) and b[5] is None


def test_fps_and_one_percent_low(w):
    times = []
    t = 0.0
    for i in range(700):
        t += 0.050 if i % 97 == 0 else 1 / 60
        times.append(t)
    fps, low, frames = w.frame_stats(times, times[-1] + 0.3)
    assert 55 < fps < 62 and 15 < low < 25 and len(frames) == 48
    assert w.fps_from_times(times, times[-1] + 10) is None  # stopped drawing
    pause = [i / 60 for i in range(300)] + [8 + i / 60 for i in range(300)]
    assert w.frame_stats(pause, pause[-1])[1] > 55  # alt-tab is not a stutter


def test_best_source_wins(w):
    dxgi = [i / 60 for i in range(600)]
    krnl = [i / 60 for i in range(600)]
    assert abs(w.best_stats([dxgi, krnl], dxgi[-1])[0] - 60) < 0.1   # same frames, no double count
    assert abs(w.best_stats([[], krnl], krnl[-1])[0] - 60) < 0.1     # DX9 / OpenGL: kernel only
    assert w.best_stats([[], []], 1.0) is None


def test_event_id_filter_layout(w):
    import ctypes
    params, keep = w._event_id_filter((184,))
    body, desc = keep
    assert params.Version == 2 and params.FilterDescCount == 1
    assert desc.Type == 0x80000200 and desc.Size == ctypes.sizeof(body) and body.Count == 1
    assert list(body.Events) == [184] and body.FilterIn == 1


def test_history_all_columns_and_old_files(w, tmp_path):
    import time as _t
    now = _t.time()
    path = str(tmp_path / "history.csv")
    with open(path, "w", encoding="utf-8") as f:  # a line written by 1.5: four columns
        f.write(f"{now - 50:.0f},12.0,55.0,\n")
    h = w.History(path)
    assert h.rows[0][1:] == (12.0, 55.0, None, None, None, None, None)  # padded to eight
    h.last_trim = now
    h.add(20.0, 60.0, 58.0, 30.0, 45.0, 2_500_000.0, 120_000.0, now=now)
    h.flush(now)
    again = w.History(path).rows
    assert len(again) == 2 and again[-1][1:] == (20.0, 60.0, 58.0, 30.0, 45.0, 2_500_000.0, 120_000.0)


def test_history_csv(w, tmp_path):
    rows = [(1_000_000.0, 20.0, 60.5, None, 30.0, 45.0, 2_500_000.0, 120_000.0), (1_000_010.0, 21.0, None, None)]
    path = tmp_path / "out.csv"
    w.set_language("ru")
    w.write_history_csv(str(path), rows)
    text = path.read_text(encoding="utf-8-sig").splitlines()
    assert text[0].startswith("time;cpu_load_%") and "60,5" in text[1] and text[2].count(";") == 7
    w.set_language("en")
    w.write_history_csv(str(path), rows)
    assert "60.5" in path.read_text(encoding="utf-8-sig").splitlines()[1]


def test_history_card_subjects(w):
    import time as _t
    import tkinter as tk
    root = tk.Tk()
    root.withdraw()
    now = _t.time()
    rows = [(now - 3000 + i * 10, 20.0, 60.0, 58.0, 30.0, 45.0, 1e6, 1e5) for i in range(300)]

    class FW:
        renderer = w.Renderer(1.0)
        cfg = {"theme": "heat"}

        class history:
            pass

    FW.root = root
    FW.history.rows = rows
    card = w.HistoryCard(FW())
    for i in range(len(card.SUBJECTS)):
        card.subject = i
        img = card.render(rows, "heat", now)
        assert img.size == (card.px(card.WIDTH), card.px(card.HEIGHT))
    x0, y0, x1, y1 = card.subject_boxes[2]

    class E:
        x, y = (x0 + x1) // 2, (y0 + y1) // 2

    card.visible, card.pos = True, (0, 0)
    card._click(E())
    assert card.subject == 2 and card.visible
    root.destroy()

"""Settings, log, formatting, host names, translations."""
import ast
import json
import logging
import os
import string

from conftest import ROOT


def test_sanitize_config_keeps_only_valid_values(w):
    c = w.sanitize_config({"x": 10, "y": "evil", "alpha": 5, "theme": "nope", "pinned": 1,
                           "hidden": ["gpu", "evil"], "alert_temp": 85, "alert_ram": 77,
                           "ping_host": "bad host", "custom_colors": ["red", "#000000"],
                           "size": 125, "lang": "xx", "junk": 1})
    assert c["x"] == 10 and c["y"] is None and c["alpha"] == 1.0 and c["theme"] == "heat"
    assert c["pinned"] is True and "junk" not in c and c["hidden"] == ["gpu"]
    assert c["alert_temp"] == 85 and c["alert_ram"] == 90 and c["ping_host"] == "1.1.1.1"
    assert c["custom_colors"] == w.DEFAULT_CONFIG["custom_colors"] and c["size"] == 125
    assert c["lang"] == "auto" and w.DEFAULT_CONFIG["run_as_admin"] is False
    assert w.sanitize_config([1, 2]) == w.DEFAULT_CONFIG


def test_defaults_are_fresh_copies(w):
    a, b = w.sanitize_config({}), w.sanitize_config({})
    a["hidden"].append("x")
    assert b["hidden"] == [] and w.DEFAULT_CONFIG["hidden"] == []


def test_config_migrates_saves_and_survives_garbage(w, tmp_path, monkeypatch):
    monkeypatch.setattr(w, "CONFIG_PATH", str(tmp_path / "widget_config.json"))
    monkeypatch.setattr(w, "OLD_CONFIG_PATH", str(tmp_path / "old.json"))
    (tmp_path / "old.json").write_text(json.dumps({"theme": "ice", "x": 5}))
    c = w.load_config()
    assert c["theme"] == "ice" and c["x"] == 5
    w.save_config(c)
    os.remove(tmp_path / "old.json")
    assert w.load_config()["theme"] == "ice"
    (tmp_path / "widget_config.json").write_text("{broken")
    assert w.load_config() == w.DEFAULT_CONFIG


def test_log_error_is_throttled(w, tmp_path, monkeypatch):
    path = tmp_path / "test.log"
    handler = logging.FileHandler(path, encoding="utf-8")
    w.log.addHandler(handler)
    try:
        for _ in range(3):
            try:
                1 / 0
            except ZeroDivisionError:
                w.log_error("throttle test")
    finally:
        w.log.removeHandler(handler)
        handler.close()
    assert path.read_text(encoding="utf-8").count("throttle test failed") == 1


def test_number_formats_follow_the_language(w):
    assert w.fmt_speed(500) == "500 Б/с" and w.fmt_speed(2.5 * 2**20) == "2,5 МБ/с"
    assert w.fmt_left(2 * 3600 + 15 * 60) == "2 ч 15 мин"
    assert w.cpu_extra_text({"cpu_clock": 3.21, "cpu_power": 4.25}) == "3,2 ГГц  ·  4,2 Вт"
    w.set_language("en")
    assert w.fmt_speed(2.5 * 2**20) == "2.5 MB/s" and w.fmt_uptime(90000) == "1 d 1 h"
    w.set_language("de")
    assert w.fmt_speed(2.5 * 2**20) == "2,5 MB/s"


def test_clean_host(w):
    cases = {"ya.ru": "ya.ru", "https://Eu.Battle.net/x": "eu.battle.net", "192.168.1.1": "192.168.1.1",
             "host:27015": "host", "bad host": None, "": None, "a..b": None, "-x.com": None,
             "x" * 300: None, "cmd & calc": None}
    for text, want in cases.items():
        assert w.clean_host(text) == want, text


def test_every_text_is_translated_with_the_same_fields():
    import translations
    tree = ast.parse(open(os.path.join(ROOT, "widget.pyw"), encoding="utf-8").read())
    keys = {n.args[0].value for n in ast.walk(tree)
            if isinstance(n, ast.Call) and getattr(n.func, "id", None) == "tr"
            and n.args and isinstance(n.args[0], ast.Constant)}
    missing = keys - set(translations.TEXTS)
    assert not missing, missing
    for key, row in translations.TEXTS.items():
        fields = {f[1] for f in string.Formatter().parse(key) if f[1]}
        assert len(row) == 3, key
        for text in row:
            assert {f[1] for f in string.Formatter().parse(text) if f[1]} == fields, (key, text)

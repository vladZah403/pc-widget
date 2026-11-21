"""Loads widget.pyw as the module `widget` with %APPDATA% in a temp folder, so the
tests never touch the real settings, log or history."""
import importlib.machinery
import importlib.util
import os
import sys
import tempfile

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)  # translations.py
os.environ["APPDATA"] = tempfile.mkdtemp(prefix="pcmon-tests-")


def _load():
    loader = importlib.machinery.SourceFileLoader("widget", os.path.join(ROOT, "widget.pyw"))
    spec = importlib.util.spec_from_loader("widget", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


W = _load()


@pytest.fixture
def w():
    """The widget module, in Russian (the source language) for every test."""
    W.set_language("ru")
    yield W
    W.set_language("ru")


def fake_data(**over):
    """A full data dict like Widget._collect() returns."""
    d = {
        "cpu": 37.0, "cpu_hist": [30 + i % 25 for i in range(60)],
        "cores": [5.0, 12.0, 3.0, 97.0, 8.0, 20.0, 1.0, 40.0],
        "ram_pct": 64.0, "ram_used": 14.2 * 2**30, "ram_total": 22.3 * 2**30,
        "disks": [("C:", 300 * 2**30, 476 * 2**30, 63.0)],
        "io_read": 1.2 * 2**20, "io_write": 300 * 1024,
        "net_down": 2.5 * 2**20, "net_up": 120 * 1024,
        "net_hist": [(3e6 * (i % 7) / 7, 4e5 * (i % 5) / 5) for i in range(60)],
        "battery": (87.0, False), "battery_left": 2 * 3600 + 15 * 60,
        "battery_wear": 16.0, "battery_full": 35.3, "battery_design": 42.0, "battery_rate": 8.2,
        "uptime": 3 * 3600 + 12 * 60,
        "temp_cpu": 66.0, "temp_gpu": None, "gpu_shared": True, "gpu_load": 23.0,
        "gpu_name": "AMD Radeon Graphics", "temp_ssd": 41.0,
        "cpu_clock": 3.21, "cpu_power": 12.4, "fans": [1450.0, 820.0],
        "lhm_online": True, "sensor_hint": None, "ping": 23, "ping_ready": True,
        "ping_host": "1.1.1.1", "procs": None, "update": None,
    }
    d.update(over)
    return d

"""Program Files copy and in-place updates, on temp folders (nothing is run)."""
import hashlib
import io
import os
import sys
import zipfile

import pytest


@pytest.fixture
def frozen_build(w, tmp_path, monkeypatch):
    """A fake onedir build in tmp/Downloads/PC-Monitor, as if we were the frozen exe."""
    build = tmp_path / "Downloads" / "PC-Monitor"
    (build / "_internal" / "lhm").mkdir(parents=True)
    exe = build / "PC-Monitor.exe"
    exe.write_bytes(b"exe v1")
    (build / "_internal" / "python313.dll").write_bytes(b"py")
    (tmp_path / "Downloads" / "unrelated.txt").write_text("must not be copied")
    (tmp_path / "Program Files").mkdir()
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(exe))
    monkeypatch.setattr(w, "APP_DIR", str(build))
    monkeypatch.setattr(w, "INSTALL_DIR", str(tmp_path / "Program Files" / "PC-Monitor"))
    return build


def test_install_copy_and_fingerprint(w, frozen_build):
    assert w.installed_exe() is None and not w.installed_is_current()
    assert w.elevation_target() == (sys.executable, "")
    assert w.install_copy()
    assert sorted(os.listdir(w.INSTALL_DIR)) == ["PC-Monitor.exe", "_internal"]
    assert w.installed_is_current() and w.elevation_target() == (w.installed_exe(), "")
    (frozen_build / "PC-Monitor.exe").write_bytes(b"exe v2")
    assert not w.installed_is_current()
    assert w.install_copy() and w.installed_is_current()
    assert not os.path.exists(w.INSTALL_DIR + ".old")


def test_task_only_for_the_protected_copy(w, frozen_build, monkeypatch):
    assert not w.task_allowed()
    monkeypatch.setattr(w, "APP_DIR", w.INSTALL_DIR)
    assert w.task_allowed() and w.install_copy() is False


def test_check_autostart_removes_unprotected_task(w, monkeypatch):
    calls, state = [], {"task": r"C:\Users\x\Python\pythonw.exe", "run": False}
    monkeypatch.setattr(w, "is_admin", lambda: True)
    monkeypatch.setattr(w, "admin_task_command", lambda: state["task"])
    monkeypatch.setattr(w, "installed_exe", lambda: None)
    monkeypatch.setattr(w, "autostart_enabled", lambda name=None: state["run"])
    monkeypatch.setattr(w, "set_autostart", lambda e: calls.append(("run", e)))
    monkeypatch.setattr(w, "set_admin_task", lambda e: calls.append(("task", e)) or True)
    w.check_autostart(prefer_task=True)
    assert calls == [("task", False), ("run", True)]


def test_release_asset_and_versions(w):
    r = w.RELEASES_URL
    asset = {"name": "PC-Monitor.zip", "size": 1000, "digest": "sha256:" + "a" * 64,
             "browser_download_url": f"https://github.com/{w.REPO}/releases/download/v9/PC-Monitor.zip"}
    info = {"tag_name": "v1.10", "html_url": r + "/tag/v1.10", "assets": [asset]}
    assert w.newer_release(info, "1.9") == ("1.10", r + "/tag/v1.10", {"url": asset["browser_download_url"],
                                                                        "sha256": "a" * 64, "size": 1000})
    assert w.newer_release(info, "1.10") is None
    assert w.newer_release(dict(info, prerelease=True), "1.0") is None
    assert w.release_asset({"assets": [dict(asset, digest="md5:x")]}) is None
    assert w.release_asset({"assets": [dict(asset, browser_download_url="https://evil.example/a.zip")]}) is None


def _zip(entries):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for name, data in entries.items():
            zf.writestr(name, data)
    return buf.getvalue()


class _Resp(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        pass


@pytest.mark.parametrize("entries, digest_ok, extra, error", [
    ({"PC-Monitor/PC-Monitor.exe": b"e", "PC-Monitor/_internal/a.dll": b"d", "x/y": b"z"}, True, 0, None),
    ({"PC-Monitor/PC-Monitor.exe": b"e", "PC-Monitor/_internal/a.dll": b"d"}, False, 0,
     "контрольная сумма не совпала"),
    ({"PC-Monitor/PC-Monitor.exe": b"e", "PC-Monitor/_internal/a.dll": b"d"}, True, 10,
     "файл скачался не полностью"),
    ({"PC-Monitor/_internal/a.dll": b"d"}, True, 0, "в архиве нет программы"),
    ({"PC-Monitor/../evil.txt": b"x", "PC-Monitor/PC-Monitor.exe": b"e"}, True, 0, "bad path"),
])
def test_updater(w, tmp_path, monkeypatch, entries, digest_ok, extra, error):
    data = _zip(entries)
    monkeypatch.setattr(w.urllib.request, "urlopen", lambda req, timeout=None, context=None: _Resp(data))
    digest = hashlib.sha256(data).hexdigest() if digest_ok else "0" * 64
    asset = {"url": "https://example.invalid/PC-Monitor.zip", "sha256": digest, "size": len(data) + extra}
    up = w.Updater(asset, (str(tmp_path / "App"), str(tmp_path / "App.new")))
    up.run()
    if error is None:
        assert up.state == "ready", up.error
        assert sorted(os.listdir(up.staging)) == ["PC-Monitor.exe", "_internal"]
    else:
        assert up.state == "error" and error in up.error
        assert not (tmp_path / "evil.txt").exists()


def test_swap_starts_outside_the_program_folder(w, tmp_path, monkeypatch):
    """A process sitting in the program folder blocks renaming it (the 1.3/1.3.1 bug)."""
    import subprocess
    calls = []
    monkeypatch.setattr(subprocess, "Popen", lambda *a, **k: calls.append(k))
    w.swap_and_restart(str(tmp_path / "App"), str(tmp_path / "App.new"))
    assert calls and calls[0]["cwd"] == w._system32()


def test_leave_program_folder(w, tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    w.leave_program_folder()
    assert os.path.normcase(os.getcwd()) == os.path.normcase(w.DATA_DIR)


def test_updater_keeps_the_uninstaller(w, tmp_path, monkeypatch):
    """A setup install has unins000.exe/.dat in the program folder: an update keeps them."""
    data = _zip({"PC-Monitor/PC-Monitor.exe": b"new", "PC-Monitor/_internal/a.dll": b"d"})
    monkeypatch.setattr(w.urllib.request, "urlopen", lambda req, timeout=None, context=None: _Resp(data))
    app = tmp_path / "App"
    app.mkdir()
    (app / "unins000.exe").write_bytes(b"u")
    (app / "unins000.dat").write_bytes(b"d")
    asset = {"url": "https://example.invalid/PC-Monitor.zip",
             "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
    up = w.Updater(asset, (str(app), str(tmp_path / "App.new")))
    up.run()
    assert up.state == "ready", up.error
    assert {"unins000.exe", "unins000.dat", "PC-Monitor.exe"} <= set(os.listdir(up.staging))


def test_short_cpu_name(w):
    assert w.short_cpu_name("AMD Ryzen 5 5500U with Radeon Graphics") == "Ryzen 5 5500U"
    assert w.short_cpu_name("Intel(R) Core(TM) i5-10400F CPU @ 2.90GHz") == "Core i5-10400F"
    assert w.short_cpu_name("12th Gen Intel(R) Core(TM) i7-12700H") == "Core i7-12700H"
    assert w.short_cpu_name("AMD Ryzen 7 7800X3D 8-Core Processor") == "Ryzen 7 7800X3D"


def test_short_gpu_name(w):
    assert w.short_gpu_name("NVIDIA GeForce RTX 3060 Laptop GPU") == "GeForce RTX 3060 Laptop GPU"
    assert w.short_gpu_name("AMD Radeon(TM) Graphics") == "Radeon Graphics"
    assert w.short_gpu_name("Intel(R) UHD Graphics 620") == "UHD Graphics 620"
    assert w.short_gpu_name("Intel(R) Arc(TM) Graphics") == "Arc Graphics"
    assert w.short_gpu_name("Intel(R) Graphics") == "Intel Graphics"  # Core Ultra without Arc

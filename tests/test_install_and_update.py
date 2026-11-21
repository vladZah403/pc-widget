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
    monkeypatch.setattr(w.urllib.request, "urlopen", lambda req, timeout=None: _Resp(data))
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

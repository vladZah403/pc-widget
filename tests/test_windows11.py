"""What the first Windows 11 laptop showed (Core Ultra, fresh install): GitHub's
root certificate missing for Python, the sensor thread failing while the program
exits, no hint when only the SSD gives a temperature."""
import ssl
import time

from conftest import fake_data


def test_https_trusts_certifi_roots_too(w):
    import certifi
    ctx = w.https_context()
    assert isinstance(ctx, ssl.SSLContext) and ctx.verify_mode == ssl.CERT_REQUIRED and ctx.check_hostname
    pem = open(certifi.where(), encoding="ascii").read()
    assert "USERTrust ECC" in pem  # GitHub's root at the time of writing
    names = {dict(x[0] for x in c["subject"]).get("commonName") for c in ctx.get_ca_certs()}
    assert "USERTrust ECC Certification Authority" in names
    assert w.https_context() is ctx  # built once


def test_sensor_reader_stops_and_closes_the_library(w, monkeypatch):
    closed, reads = [], []

    class Native:
        def __init__(self, path):
            pass

        def tree(self):
            reads.append(1)
            return {"Text": "Sensor", "Children": []}

        def close(self):
            closed.append(1)

    monkeypatch.setattr(w, "lhm_dir", lambda: "lhm")
    monkeypatch.setattr(w, "NativeSensors", Native)
    monkeypatch.setattr(w, "_fetch_web", lambda: (_ for _ in ()).throw(OSError("no server")))
    monkeypatch.setattr(w, "LHM_POLL_S", 0.01)
    reader = w.LhmReader()
    reader.start()
    deadline = time.monotonic() + 5
    while not reads and time.monotonic() < deadline:
        time.sleep(0.01)
    reader.stop()
    assert not reader.is_alive() and closed == [1] and reads


def test_hint_when_only_the_ssd_gives_a_temperature(w):
    reader = w.LhmReader()
    reader.native_ok, reader.admin = True, True
    reader.sensors = dict(w.EMPTY_SENSORS, ssd=28.0)
    assert reader.hint() == "CPU и GPU: нужен PawnIO"  # short: it shares the row with the SSD
    reader.sensors = dict(w.EMPTY_SENSORS)
    assert reader.hint() == "Нет доступа к датчикам: нужен PawnIO"
    reader.sensors = dict(w.EMPTY_SENSORS, ssd=28.0, cpu=55.0)
    assert reader.hint() is None

    shown = []

    class R(w.Renderer):
        def _t(self, d, x, y, text, font, fill, anchor="ls"):
            shown.append(text)
            super()._t(d, x, y, text, font, fill, anchor)

    r = R(1.0)
    for lang in ("ru", "en", "uk", "de"):  # the whole hint fits beside the SSD
        w.set_language(lang)
        hint = w.tr("CPU и GPU: нужен PawnIO")
        data = fake_data(temp_cpu=None, temp_gpu=None, temp_ssd=28.0, lhm_online=True, sensor_hint=hint)
        shown.clear()
        assert r.render(data, "heat").height == r.layout_height(data)
        assert hint in shown

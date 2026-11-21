"""When Windows' own ways fail: SSD health from the sensor library (Intel RST / VMD
refuses Windows' NVMe query) and a CPU temperature from the ACPI thermal zones
(an Intel CPU without the PawnIO driver)."""
from conftest import fake_data

# what the library shows for an NVMe drive behind Intel RST, as NativeSensors builds it
KIOXIA = {"Text": "KBG50ZNV512G KIOXIA", "ImageURL": "images_icon/hdd.png", "Children": [
    {"Text": "Temperatures", "Children": [
        {"Text": "Composite Temperature", "Value": "41.0 °C", "SensorId": "/nvme/0/temperature/0"},
        {"Text": "Warning Temperature", "Value": "83.0 °C", "SensorId": "/nvme/0/temperature/5"}]},
    {"Text": "Levels", "Children": [
        {"Text": "Available Spare", "Value": "100.0 %", "SensorId": "/nvme/0/level/1"},
        {"Text": "Available Spare Threshold", "Value": "10.0 %", "SensorId": "/nvme/0/level/2"},
        {"Text": "Percentage Used", "Value": "4.0 %", "SensorId": "/nvme/0/level/3"}]},
    {"Text": "Data", "Children": [
        {"Text": "Data Read", "Value": "11234.5 GB", "SensorId": "/nvme/0/data/4"},
        {"Text": "Data Written", "Value": "9876.5 GB", "SensorId": "/nvme/0/data/5"}]}]}
# a drive the library sees but can't read the health log of
USB_DISK = {"Text": "Generic Flash Disk", "ImageURL": "images_icon/hdd.png", "Children": [
    {"Text": "Load", "Children": [{"Text": "Used Space", "Value": "40.0 %", "SensorId": "/hdd/2/load/0"}]}]}
INTEL = {"Text": "Intel Core i5-1235U", "ImageURL": "images_icon/cpu.png", "Children": [
    {"Text": "Load", "Children": [{"Text": "CPU Total", "Value": "12.0 %", "SensorId": "/intelcpu/0/load/0"}]}]}


def tree(*hardware):
    return {"Text": "Sensor", "Children": [{"Text": "PC", "Children": list(hardware)}]}


def test_ssd_health_from_the_library(w):
    r = w.read_lhm(tree(INTEL, KIOXIA, USB_DISK))
    assert set(r) == set(w.EMPTY_SENSORS) and r["ssd"] == 41.0 and r["cpu"] is None
    assert r["drives"] == [{"drive": 0, "model": "KBG50ZNV512G KIOXIA", "letters": [], "critical": 0,
                            "temp": 41, "spare": 100, "spare_min": 10, "wear": 4,
                            "written": round(9876.5e9), "hours": None, "source": "library"}]


def test_library_drives_get_their_letters_and_draw(w):
    class Health:
        drives, letters = [], {0: ["D:", "C:"]}

    sensors = w.read_lhm(tree(KIOXIA))
    drives = w.library_drives(sensors, Health())
    assert drives[0]["letters"] == ["C:", "D:"] and sensors["drives"][0]["letters"] == []  # a copy
    assert w.library_drives(sensors, None)[0]["letters"] == []
    assert w.library_drives(w.EMPTY_SENSORS, Health()) == []
    img = w.Renderer(1.0).render(fake_data(drives=drives), "heat")  # the "SSD C: D:" row
    no_written = w.Renderer(1.0).render(fake_data(drives=[dict(drives[0], written=None)]), "heat")
    assert img.size == no_written.size


def test_thermal_zone_stands_in_for_the_cpu(w, monkeypatch):
    zones = w.ThermalZones()
    monkeypatch.setattr(zones, "zones", lambda: [("\\_TZ.TZ00", 27.8), ("\\_TZ.CPUZ", 63.25),
                                                 ("\\_TZ.BAD", -273.15), ("\\_TZ.HOT", 180.0)])
    assert zones.read() == 63.2 or zones.read() == 63.3  # the hottest plausible zone
    monkeypatch.setattr(zones, "zones", lambda: [])
    assert zones.read() is None


def test_reader_uses_the_zone_only_without_a_real_cpu_temperature(w, monkeypatch):
    monkeypatch.setattr(w, "lhm_dir", lambda: None)  # no library: no web server either here
    monkeypatch.setattr(w, "_fetch_web", lambda: (_ for _ in ()).throw(OSError("no server")))
    reader = w.LhmReader()
    monkeypatch.setattr(reader.zones, "read", lambda: 58.0)
    monkeypatch.setattr(reader.zones, "zones", lambda: [("\\_TZ.CPUZ", 58.0)])
    monkeypatch.setattr(reader.stopping, "wait", lambda s: reader.stopping.set())  # one round
    reader.run()
    assert reader.sensors["cpu"] == 58.0 and reader.sensors["cpu_approx"] is True
    assert reader.online and reader.source == "thermal zone" and reader.hint() is None
    # chips: "CPU ≈58°" in the full widget and the compact one
    data = fake_data(temp_cpu=58.0, temp_cpu_approx=True, temp_gpu=None, gpu_shared=True)
    for compact in (False, True):
        w.Renderer(1.0).render(data, "heat", compact=compact)

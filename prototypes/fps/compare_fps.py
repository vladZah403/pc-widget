"""Stage 0 check: FPS of the Rust prototype against PC Monitor 1.8 on the same frames.

Runs 1.8's FpsReader (from widget.pyw, in its own ETW session) and
`pcm-proto-fps --json` side by side, samples both once a second and prints,
per drawing process, the average FPS of each and how far apart they were.
Goal from the 2.0 plan: within ±1 FPS in 3 games.

    python prototypes/fps/compare_fps.py [--seconds 60] [--exe path]

Run it as administrator for OpenGL / Vulkan / DirectX 9 games: their frames
come from the kernel graphics provider, which may need admin rights.
"""
import argparse
import importlib.machinery
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))


def load_widget():
    """widget.pyw as a module, with %APPDATA% in a temp folder so the real
    settings, log and history are never touched."""
    sys.path.insert(0, ROOT)  # translations.py
    os.environ["APPDATA"] = tempfile.mkdtemp(prefix="pcm-compare-")
    loader = importlib.machinery.SourceFileLoader("widget", os.path.join(ROOT, "widget.pyw"))
    spec = importlib.util.spec_from_loader("widget", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def default_exe():
    target = os.path.join(ROOT, "target")
    try:
        with open(os.path.join(ROOT, ".cargo", "config.toml"), encoding="utf-8") as f:
            m = re.search(r'^target-dir\s*=\s*"([^"]+)"', f.read(), re.M)
        if m:
            target = m.group(1)
    except OSError:
        pass
    return os.path.join(target, "release", "pcm-proto-fps.exe")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=int, default=60)
    ap.add_argument("--exe", default=default_exe())
    args = ap.parse_args()
    if not os.path.exists(args.exe):
        sys.exit(f"no {args.exe}: build it with  cargo build -p pcm-proto-fps --release")

    w = load_widget()
    w.FPS_SESSION = "PCMonitorCompare"  # not the running widget's session
    reader = w.FpsReader()
    reader.start()
    rust = subprocess.Popen([args.exe, "--json", "--seconds", str(args.seconds)],
                            stdout=subprocess.PIPE, text=True, encoding="utf-8")
    print(f"comparing for {args.seconds} s - play now (windowed or borderless is fine)")

    pairs = {}  # pid -> [(rust fps or None, 1.8 fps or None)]
    names = {}
    sources = {}
    for line in rust.stdout:
        tick = json.loads(line)
        now_rust = {p["pid"]: p["fps"] for p in tick["procs"]}
        for p in tick["procs"]:
            names[p["pid"]] = p["name"]
            sources.setdefault(p["pid"], set()).add(p["src"])
        with reader.lock:
            py_pids = {pid for pid, _ in reader.frames}
        now_py = {}
        for pid in py_pids:
            st = reader.stats(pid)
            if st:
                now_py[pid] = st[0]
        for pid in set(now_rust) | set(now_py):
            pairs.setdefault(pid, []).append((now_rust.get(pid), now_py.get(pid)))
        t = int(tick["t"])
        if t % 10 == 0:
            front = tick["fg"]
            print(f"  {t:>3} s  front: {names.get(front, front)}"
                  f"  2.0 {now_rust.get(front, '-')}  1.8 {now_py.get(front, '-')}")
    rust.wait()
    reader.stop()
    if reader.error:
        print("1.8 reader:", reader.error)
    py_sources = {src for _, src in list(reader.frames)}

    print("\nprocess                       samples   2.0 avg   1.8 avg   mean diff  max diff")
    worst = 0.0
    for pid, rows in sorted(pairs.items(), key=lambda kv: -len(kv[1])):
        both = [(a, b) for a, b in rows if a is not None and b is not None]
        if len(both) < 5:
            continue
        diffs = [abs(a - b) for a, b in both]
        mean_diff = sum(diffs) / len(diffs)
        worst = max(worst, mean_diff)
        only = len(rows) - len(both)
        print(f"{names.get(pid, pid):<28} {len(both):>8} {sum(a for a, _ in both) / len(both):>9.2f}"
              f" {sum(b for _, b in both) / len(both):>9.2f} {mean_diff:>10.2f} {max(diffs):>9.2f}"
              f"  {'/'.join(sorted(sources.get(pid, ())))}" + (f"  ({only} s seen by one only)" if only else ""))
    print(f"\nkernel provider (DX9/OpenGL/Vulkan) seen: 2.0 "
          f"{'yes' if any('krnl' in s for s in sources.values()) else 'no'}, 1.8 "
          f"{'yes' if 'krnl' in py_sources else 'no'}")
    print("RESULT:", "OK, within 1 FPS" if worst <= 1.0 else f"DIFFERENT (mean diff up to {worst:.2f} FPS)")


if __name__ == "__main__":
    main()

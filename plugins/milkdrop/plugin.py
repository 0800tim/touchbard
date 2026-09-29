#!/usr/bin/env python3
"""Touch Bar plugin: Milkdrop via libprojectM, fed from the default sink's monitor.

Presets: ~/.local/share/touchbar/milkdrop-presets (fetched on first run from
projectM's "cream of the crop" pack), textures alongside.
"""
import json, os, signal, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DATA = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / "touchbar"
PRESETS, TEXTURES = DATA / "milkdrop-presets", DATA / "milkdrop-textures"
REPOS = {PRESETS: "https://github.com/projectM-visualizer/presets-cream-of-the-crop.git",
         TEXTURES: "https://github.com/projectM-visualizer/presets-milkdrop-texture-pack.git"}

def log(msg):
    sys.stdout.write(json.dumps({"t": "log", "msg": msg}) + "\n"); sys.stdout.flush()

def fetch():
    for path, url in REPOS.items():
        if not path.exists():
            log(f"downloading {url}")
            path.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["git", "clone", "-q", "--depth", "1", url, str(path)], check=False)

def binary():
    # Built on first use into the cache: the plugin folder may be read-only.
    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "touchbar"
    cache.mkdir(parents=True, exist_ok=True)
    b = cache / "touchbar-milkdrop"
    src = HERE / "touchbar-milkdrop.c"
    if not b.exists() or (src.exists() and src.stat().st_mtime > b.stat().st_mtime):
        subprocess.run(["gcc", "-O2", "-o", str(b), str(src), "-lprojectM-4", "-lprojectM-4-playlist",
                        "-lEGL", "-lOpenGL"], check=True)
    return b

class Renderer:
    def __init__(self):
        self.parec = self.md = None

    def start(self, w, h):
        self.stop()
        self.parec = subprocess.Popen(["parec", "-d", "@DEFAULT_MONITOR@", "--format=float32le", "--channels=2",
                                       "--rate=44100", "--latency-msec=20"], stdout=subprocess.PIPE,
                                      stderr=subprocess.DEVNULL)
        # Frames go straight from the renderer to our stdout, i.e. to the agent.
        self.md = subprocess.Popen([str(binary()), str(w), str(h), str(PRESETS), str(TEXTURES), "30"],
                                   stdin=self.parec.stdout, stdout=sys.stdout.fileno())

    def stop(self):
        for p in (self.md, self.parec):
            if p and p.poll() is None:
                p.terminate()
                try:
                    p.wait(2)
                except subprocess.TimeoutExpired:
                    p.kill()
        self.parec = self.md = None

def main():
    fetch()
    r = Renderer()
    size = None
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("t") == "size":
            new = (int(msg["w"]), int(msg["h"]))
            if new != size:
                size = new
                r.start(*size)
        elif msg.get("t") == "cmd" and msg.get("cmd") in ("next", "tap") and r.md:
            r.md.send_signal(signal.SIGUSR1)
    r.stop()

main()

#!/usr/bin/env python3
"""Touch Bar plugin: stream cava's spectrum as {"t":"bars"} messages."""
import json, os, subprocess, sys, tempfile, threading

BARS = 96
CONFIG = f"""
[general]
bars = {BARS}
framerate = 30
autosens = 1
lower_cutoff_freq = 40
higher_cutoff_freq = 12000
[input]
method = pipewire
source = auto
[output]
method = raw
raw_target = /dev/stdout
data_format = binary
bit_format = 16bit
channels = mono
mono_option = average
[smoothing]
noise_reduction = 70
monstercat = 0
"""

def main():
    with tempfile.NamedTemporaryFile("w", suffix=".cava", delete=False) as f:
        f.write(CONFIG)
    proc = subprocess.Popen(["cava", "-p", f.name], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    # The agent closing our stdin means stop.
    threading.Thread(target=lambda: (sys.stdin.read(), proc.terminate()), daemon=True).start()
    out = sys.stdout.buffer
    frame = BARS * 2
    try:
        while True:
            chunk = proc.stdout.read(frame)
            if len(chunk) < frame:
                break
            vals = [round(int.from_bytes(chunk[i:i + 2], "little") / 65535, 3) for i in range(0, frame, 2)]
            out.write((json.dumps({"t": "bars", "v": vals}, separators=(",", ":")) + "\n").encode())
            out.flush()
    except BrokenPipeError:
        pass
    finally:
        proc.terminate()
        os.unlink(f.name)

main()

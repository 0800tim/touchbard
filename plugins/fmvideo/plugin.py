#!/usr/bin/env python3
"""Touch Bar plugin: the music video for what's playing, in sync.

Finds the YouTube video (from the player's URL or artwork when it's
fm.video/YouTube, otherwise a YouTube search on artist + title), decodes a
tiny 144p copy with ffmpeg in step with the player's position, and draws:

  ambient    a tiny live player over a soft glow of the same video
  filmstrip  ten 16:9 frames that scroll left as the video plays; the
             newest, on the right, is live

"next" (or a tap) switches style. Needs yt-dlp and ffmpeg.
"""
import json
import math
import re
import subprocess
import sys
import threading
import time

import cairo

FPS = 15
STRIP_FRAMES = 10
STRIP_INTERVAL = 2.5  # seconds of video between filmstrip frames
YT_ID = re.compile(r"(?:youtu\.be/|[?&]v=|/vi(?:_webp)?/|/embed/|/shorts/)([\w-]{11})")

out_lock = threading.Lock()


def emit(obj, payload=None):
    data = (json.dumps(obj, separators=(",", ":")) + "\n").encode()
    with out_lock:
        try:
            sys.stdout.buffer.write(data)
            if payload is not None:
                sys.stdout.buffer.write(payload)
            sys.stdout.buffer.flush()
        except BrokenPipeError:
            sys.exit(0)


def log(msg):
    emit({"t": "log", "msg": msg})


def hex_rgb(s, default=(0.5, 0.6, 1.0)):
    try:
        s = s.lstrip("#")
        return tuple(int(s[i:i + 2], 16) / 255 for i in (0, 2, 4))
    except (ValueError, AttributeError):
        return default


class State:
    def __init__(self):
        self.size = None
        self.style = "ambient"
        self.media = {}
        self.media_at = time.monotonic()
        self.theme = {}
        self.lock = threading.Lock()

    def position(self):
        m = self.media
        pos = float(m.get("position") or 0)
        if m.get("playing"):
            pos += time.monotonic() - self.media_at
        return pos


S = State()


# ---- finding the video ------------------------------------------------------

_search_cache = {}


def run(cmd, timeout=25):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return ""


def video_id(media):
    for key in ("url", "art_url"):
        m = YT_ID.search(media.get(key) or "")
        if m:
            return m.group(1)
    title, artist = media.get("title") or "", media.get("artist") or ""
    if not title:
        return None
    q = f"{artist} {title} official music video".strip()
    if q not in _search_cache:
        _search_cache[q] = run(["yt-dlp", "--no-warnings", "--flat-playlist", "--print", "id", f"ytsearch1:{q}"]) or None
    return _search_cache[q]


def stream_url(vid):
    # 144p/240p video-only is plenty for a 60 px tall strip.
    return run(["yt-dlp", "--no-warnings", "-f", "160/278/394/133/242/worst[vcodec!=none]", "-g",
                f"https://www.youtube.com/watch?v={vid}"]).splitlines()[0:1]


# ---- decoding -----------------------------------------------------------------

class Decoder:
    """ffmpeg playing the video silently in real time from a start position."""

    def __init__(self):
        self.proc = None
        self.start_pos = 0.0
        self.frames = 0
        self.latest = None  # cairo surface
        self.history = []  # (video_time, surface) for the filmstrip
        self.fw, self.fh = 0, 0
        self.lock = threading.Lock()

    def running(self):
        return self.proc is not None and self.proc.poll() is None

    def pos(self):
        return self.start_pos + self.frames / FPS

    def start(self, url, pos, fh):
        self.stop()
        self.fh = fh
        self.fw = int(round(fh * 16 / 9))
        self.start_pos, self.frames = pos, 0
        self.proc = subprocess.Popen(
            ["ffmpeg", "-loglevel", "error", "-ss", f"{max(pos, 0):.2f}", "-re", "-i", url, "-an",
             "-vf", f"fps={FPS},scale={self.fw}:{self.fh}:force_original_aspect_ratio=increase,"
                    f"crop={self.fw}:{self.fh}",
             "-f", "rawvideo", "-pix_fmt", "bgra", "-"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        threading.Thread(target=self._read, args=(self.proc,), daemon=True).start()

    def _read(self, proc):
        size = self.fw * self.fh * 4
        while True:
            buf = proc.stdout.read(size)
            if len(buf) < size:
                return
            surf = cairo.ImageSurface.create_for_data(bytearray(buf), cairo.FORMAT_RGB24, self.fw, self.fh,
                                                      self.fw * 4)
            with self.lock:
                if proc is not self.proc:
                    return
                self.frames += 1
                self.latest = surf
                t = self.pos()
                slot = math.floor(t / STRIP_INTERVAL)
                if not self.history or math.floor(self.history[-1][0] / STRIP_INTERVAL) != slot:
                    self.history.append((t, surf))
                    self.history = self.history[-(STRIP_FRAMES + 2):]

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
        self.proc = None


# ---- drawing --------------------------------------------------------------------

def rounded(c, x, y, w, h, r):
    c.new_sub_path()
    c.arc(x + w - r, y + r, r, -math.pi / 2, 0)
    c.arc(x + w - r, y + h - r, r, 0, math.pi / 2)
    c.arc(x + r, y + h - r, r, math.pi / 2, math.pi)
    c.arc(x + r, y + r, r, math.pi, 1.5 * math.pi)
    c.close_path()


def draw_frame(c, surf, x, y, w, h, r=7, border=None, alpha=1.0):
    c.save()
    rounded(c, x, y, w, h, r)
    c.clip()
    c.translate(x, y)
    c.scale(w / surf.get_width(), h / surf.get_height())
    c.set_source_surface(surf, 0, 0)
    c.get_source().set_filter(cairo.FILTER_GOOD)
    c.paint_with_alpha(alpha)
    c.restore()
    if border:
        c.set_source_rgba(*border, 0.9)
        c.set_line_width(2)
        rounded(c, x, y, w, h, r)
        c.stroke()


def glow(c, surf, w, h, bg):
    """The video blown up and blurred behind everything, like YouTube's ambient mode."""
    tiny = cairo.ImageSurface(cairo.FORMAT_RGB24, 24, 2)
    tc = cairo.Context(tiny)
    tc.scale(24 / surf.get_width(), 2 / surf.get_height())
    tc.set_source_surface(surf, 0, 0)
    tc.get_source().set_filter(cairo.FILTER_GOOD)
    tc.paint()
    c.save()
    c.scale(w / 24, h / 2)
    c.set_source_surface(tiny, 0, 0)
    c.get_source().set_filter(cairo.FILTER_BILINEAR)
    c.paint()
    c.restore()
    c.set_source_rgba(*bg, 0.5)
    c.paint()


def render(dec, w, h, status):
    bg = hex_rgb(S.theme.get("bg"), (0.05, 0.05, 0.08))
    accent = hex_rgb(S.theme.get("accent"))
    fg = hex_rgb(S.theme.get("fg"), (0.8, 0.8, 0.9))
    surf = cairo.ImageSurface(cairo.FORMAT_RGB24, w, h)
    c = cairo.Context(surf)
    c.set_source_rgb(*bg)
    c.paint()
    with dec.lock:
        latest, history, vpos = dec.latest, list(dec.history), dec.pos()
    if latest is None:
        c.set_source_rgb(*fg)
        c.select_font_face("JetBrainsMono Nerd Font", cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
        c.set_font_size(18)
        ext = c.text_extents(status)
        c.move_to((w - ext.width) / 2, h / 2 + ext.height / 2)
        c.show_text(status)
        return surf
    glow(c, latest, w, h, bg)
    pad = 4
    fh = h - 2 * pad
    fw = fh * 16 / 9
    if S.style == "filmstrip":
        gap = 6
        step = fw + gap
        total = STRIP_FRAMES * step - gap
        right = (w + total) / 2  # right edge of the live frame
        frac = (vpos % STRIP_INTERVAL) / STRIP_INTERVAL
        past = [f for f in history if f[0] < vpos - 0.2][::-1]  # newest first
        for k, (_, frame) in enumerate(past[:STRIP_FRAMES]):
            # Slot k+1 left of the live frame, sliding left as the interval runs;
            # when a new frame lands everything moves up a slot, so it's seamless.
            x = right - fw - (k + 1 + frac) * step
            if x + fw < 0:
                continue
            fade = max(0.25, 1 - k * 0.07)
            draw_frame(c, frame, x, pad, fw, fh, alpha=fade)
        draw_frame(c, latest, right - fw, pad, fw, fh, border=accent)
    else:
        draw_frame(c, latest, pad + 2, pad, fw, fh, r=8, border=accent)
    return surf


# ---- main -----------------------------------------------------------------------

def worker():
    dec = Decoder()
    track = None
    url = None
    status = "Looking for the video…"
    resolving = [False]

    def resolve(media, key):
        nonlocal url, status
        vid = video_id(media)
        u = stream_url(vid) if vid else []
        if key != track_key(S.media):
            return
        url = u[0] if u else None
        status = "No video found" if not url else "Loading…"
        emit({"t": "state", "has_video": bool(url)})
        resolving[0] = False

    def track_key(m):
        return (m.get("title"), m.get("artist"), m.get("url"))

    next_frame = time.monotonic()
    while True:
        with S.lock:
            media, size = dict(S.media), S.size
        key = track_key(media)
        if key != track:
            track, url = key, None
            dec.stop()
            dec.latest, dec.history = None, []
            status = "Looking for the video…"
            if media.get("title") and not resolving[0]:
                resolving[0] = True
                threading.Thread(target=resolve, args=(media, key), daemon=True).start()
        if size and url:
            want = S.position()
            if not media.get("playing"):
                dec.stop()  # hold the last frame while paused
            elif not dec.running() or abs(dec.pos() - want) > 2.5 or dec.fh != size[1] - 8:
                dec.start(url, want, size[1] - 8)
        if size:
            frame = render(dec, size[0], size[1], status)
            frame.flush()
            emit({"t": "frame", "w": size[0], "h": size[1], "len": size[0] * size[1] * 4}, bytes(frame.get_data()))
        next_frame += 1 / FPS
        time.sleep(max(0.0, next_frame - time.monotonic()))
        if next_frame < time.monotonic() - 1:
            next_frame = time.monotonic()


def main():
    threading.Thread(target=worker, daemon=True).start()
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        t = msg.get("t")
        with S.lock:
            if t == "size":
                S.size = (int(msg["w"]), int(msg["h"]))
                style = (msg.get("options") or {}).get("style")
                if style in ("ambient", "filmstrip"):
                    S.style = style
            elif t == "media":
                S.media = msg
                S.media_at = time.monotonic()
            elif t == "theme":
                S.theme = msg
            elif t == "cmd" and msg.get("cmd") in ("next", "tap"):
                S.style = "filmstrip" if S.style == "ambient" else "ambient"


main()

# Starter prompt: port touchbard to an on-screen overlay (Omarchy plugin)

Paste everything below the line into Claude Code on an Omarchy PC. It works best with a touchscreen, but a mouse works too.

---

I want to port **touchbard** (https://github.com/0800tim/touchbard, MIT) into an **on-screen overlay** for ordinary Omarchy PCs that have no Touch Bar, packaged as an **Omarchy plugin**. It should do exactly what the Touch Bar version does for music, from Spotify, YouTube in any browser, and fm.video (a PWA), but draw on the screen instead of the Touch Bar. This PC has a touchscreen, so touch must work well; mouse must work too.

Clone the repo first and read `README.md`, `daemon/src/{ui,render,proto,main}.rs` and `agent/touchbar-agent`. Don't start writing code until you've read them.

## What touchbard is

- **`touchbard`** (Rust, cairo + pango): a model/renderer pair.
  - `ui.rs` holds all state and touch logic in one `Model` with `touch_down/motion/up(slot, x)`.
  - `render.rs` draws a frame from the model into any cairo context. It has no hardware dependencies; the hardware code is all in `hw.rs` and the loop in `main.rs`.
  - `touchbard --preview out.png layout.json theme.json key=value...` renders any state to a PNG. Use this constantly to check your work visually.
  - `TOUCHBARD_BENCH=N` with `--preview` measures the cost per frame.
- **`touchbar-agent`** (Python + GLib): runs in the session and feeds the daemon over a Unix socket (JSON lines, `proto.rs`). It supplies:
  - the Omarchy theme and config
  - MPRIS (title, artist, art, position, seek)
  - volume via `wpctl`
  - the cava spectrum (the `plugins/cava` plugin)
  - lyrics from LRCLIB (`fetch_lyrics`, `clean_track`)
  - a colour palette from the artwork (`art_palette`)
  - live-footage colours (`grim -T`)
  - weather and Hyprland state
  - saved preferences
- **The music experience to reproduce** (the full-screen "viz" overlay):
  - album art, the title with a beat-reactive dot-matrix treatment, and karaoke lyrics with a left-to-right fill
  - 7 visualisers (bars, peaks, dots, ripple, aurora, pixels, comet)
  - 9 colour moods (music from the artwork, mono, smoke, amethyst, matrix, disco, rasta, rainbow, theme)
  - 8 text styles (dot matrix, wave, outline, typewriter, scatter, blocks, sparkle, explode)
  - a volume bar drawn over the visualiser
  - the lyrics sync adjuster: tap the time; ±0.1 s, remembered per track, optional Share back to LRCLIB
  - a "No lyrics available" banner in sparkle/explode blocks when a song has no lyrics
  - a drag-to-seek progress bar in fm.video's pink → magenta → purple (`#ff2e9a`, `#ce34c6`, `#9b4dff`) in the music mood, and the mood's own gradient otherwise
  - the transport controls
  - the buttons: magic-wand style, 🎨 mood, Aa (a live preview of the text style), 🎤 karaoke, 🔊 volume

## The architecture I want (reuse, don't rewrite)

Add an **overlay output backend** to the existing daemon, so the renderer and all the effects stay one codebase:

1. **`touchbard --overlay`:** a Wayland **layer-shell** surface (`zwlr_layer_shell_v1`; `smithay-client-toolkit` is a good fit) instead of DRM.
   - Default: anchored to the bottom of the focused monitor, full width, about 96 logical px tall, on the `overlay` or `top` layer, with no exclusive zone (it floats over windows).
   - Make the position, height, width (full, or centred at 60%) and corner radius configurable.
   - Render with the existing `render::draw` into a shm buffer. Keep the model's coordinate space (height 60) and scale it with a cairo transform, so every effect looks identical. Respect the output scale for crisp HiDPI.
2. **Input:** map `wl_touch` down/motion/up (multi-touch slots) and `wl_pointer` (a button press is a touch down, motion while pressed is a drag, release is a touch up) onto the existing `Model::touch_*` calls, converting surface coordinates to model x.
   - A mouse wheel over the bar should nudge the volume.
   - Tapping and dragging must feel the same with a finger or a mouse.
3. **No root.** The overlay runs as the user: no DRM, no Touch Bar backlight, no `/dev/uinput`.
   - Key actions (play/pause, next, previous, mute, volume) become agent commands instead of uinput keys: MPRIS calls through the agent's existing session bus code, and `wpctl` / `omarchy` commands.
   - Drop the Touch Bar-only features in overlay mode: the fn/F-key layers, the Esc logic, the Touch ID prompt, and backlight idle-dimming. Guard them behind the backend so the Touch Bar build is unchanged.
4. **Socket and authentication:** in overlay mode use `$XDG_RUNTIME_DIR/touchbard-overlay.sock` (mode 0600). Accept only peers whose `SO_PEERCRED` UID equals the daemon's own UID.
   - The agent already honours `TOUCHBARD_SOCKET`.
   - Its `daemon_peer_ok` currently accepts only root or `nobody`. Add an overlay mode that accepts its own UID, and only for the user-owned runtime socket.
   - This was a real review finding on the Touch Bar version, so keep the auth strict.
5. **The overlay's own layout:**
   - The main view is the music experience itself: essentially the full-screen viz overlay, with art, title or lyrics, visualiser and all the buttons.
   - A small compact state (art, scrolling title, spectrum, transport) that expands on tap.
   - Auto-show when something starts playing and auto-hide after N seconds of silence (configurable), with a keybinding to toggle it (`omarchy` bindings in `~/.config/hypr/bindings.lua`).
6. **Omarchy plugin packaging:** follow what the repo already does: a root `manifest.json` plus a `BarWidget.qml`, validated with `omarchy plugin validate`.
   - The bar icon toggles the overlay (start or stop a `touchbard-overlay` **systemd user service**) and shows whether it's running.
   - The install needs **no sudo** (a user service and user binaries), which makes the marketplace review much simpler.
   - It can live in this repo (a `--overlay` feature plus a second manifest folder) or in a new repo such as `touchbard-overlay` that depends on this one. Choose one and explain why.

## Hard-won lessons from the Touch Bar version (please respect them)

- **Frame budget:** cap redraws at 30 fps and only animate while something moves. Every visual currently costs 0.7–2.9 ms/frame; keep it that way, and check with `TOUCHBARD_BENCH`.
- **Never leak child processes:** stop plugins by process group, give children `PR_SET_PDEATHSIG`, and debounce background plugins. A bug once left 272 `cava` processes running, which starved the audio DSP and made the speakers crackle. After your changes, `pgrep -c -x cava` should never exceed 1.
- **Don't let a slow echo from the agent fight the finger:** the model holds a value locally for a moment after a drag, and the overlay needs the same behaviour.
- **Lyrics:** use LRCLIB's exact match first, then search; clean YouTube titles; cache on disk. When the agent is restarted after an update, make sure the new agent is actually the one running. The install must restart the user service.
- **Security:** no `curl | sh`, no unpinned git fetches of code, no sudoers rules, and strict socket auth. The Omarchy marketplace scans for all of these.

## How to work

- Plan first: write the backend design, then build it in small commits, previewing each visual step with `--preview`.
- Test with real players and with a fake MPRIS player. A ~20-line Python `Gio.bus_own_name` script works, with no audio needed.
- Test touch and mouse on this machine; check that drag-to-seek, the sliders and the buttons all work with a finger.
- Performance: measure CPU with the overlay idle and while playing, and report the numbers.
- Keep the Touch Bar build green. `cargo build --release` for the normal daemon must still produce an identical Touch Bar experience.

## Done means

- [ ] `omarchy plugin add <repo>` then clicking the bar icon shows the overlay, with no sudo anywhere.
- [ ] Spotify, YouTube in the browser, and fm.video all drive it: art, title, spectrum, karaoke lyrics, seek, volume and transport.
- [ ] Every style, mood and text style from the Touch Bar version works and looks the same.
- [ ] Touch and mouse both work; the overlay auto-shows and auto-hides; the keybinding toggles it.
- [ ] Idle CPU is near zero; playing costs ≤ 5% of one core; no leaked processes.
- [ ] README section and screenshots for the overlay; `omarchy plugin validate` passes.

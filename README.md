# Touch Bar for Omarchy

A macOS-style Touch Bar for the 13" M1 MacBook Pro running Omarchy, in the colours of the current Omarchy theme.

## What's on it

**Main layer**, left to right: esc · fn · apps · overview · screenshot · now playing · ⏮ ⏯ ⏭ · screen brightness · keyboard light · volume · mute.

- **Sliders** (brightness, keyboard light, volume) work like the Mac. Tap one and it opens full width. Drag anywhere on the track, or tap the icons at either end to step. Faster still: press the button and slide straight away without lifting, and it tidies itself up when you let go. The thin line under each button shows the current level.
- **fn** (on the bar) switches to F1–F12; tap it again to come back. **Holding the physical fn key** shows the F-keys for as long as you hold it.
- **Touch ID:** when anything asks for your fingerprint (sudo, polkit, 1Password, the lock screen), the right end of the bar shows a pulsing fingerprint with chevrons running towards the sensor. It turns green on a match, shakes yellow on a retry, and goes red if the scan fails.
- The bar follows your screen brightness. It dims after 30 s without input and turns off after 60 s; touching it only wakes it. It's off while the lid is closed.

## Customise

Edit `~/.config/touchbar/config.toml`. It applies when you save, and the file documents every option. Add buttons that run any command or send any key chord, make your own layers, add a clock or battery, and override theme colours (for example `background = "#000000"` for true black).

```bash
touchbar-agent --dump-layout   # see what your config expands to
touchbar-agent --layer function
```

## How it fits together

| Part | Runs as | Job |
|---|---|---|
| `touchbard` (Rust) | system service, from boot | owns the Touch Bar screen and digitiser; draws; sends keys through a virtual keyboard |
| `touchbar-agent` (Python) | systemd user service | theme, config, volume/brightness/media state, Touch ID prompts from fprintd; runs button commands |

They talk over `/run/touchbard/touchbard.sock`, one JSON line per message. Until the agent connects, for example at the login screen, the daemon shows a plain key-only layout.

touchbard starts as root, opens the panel, backlight, uinput and socket, and then drops to `nobody` with only the `input` and `video` groups.

## Install / uninstall

```bash
cd daemon && cargo build --release && cd ..
sudo system/install.sh                 # replaces tiny-dfr, rolls back if it fails
install -Dm644 system/touchbar-agent.service ~/.config/systemd/user/touchbar-agent.service
systemctl --user daemon-reload && systemctl --user enable --now touchbar-agent

sudo system/uninstall.sh               # back to stock tiny-dfr
```

`touchbard --preview out.png layout.json theme.json volume=0.5 finger=scan` renders a frame to a PNG, for designing without the hardware.

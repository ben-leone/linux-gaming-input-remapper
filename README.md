# GameRemapper

Copyright (C) 2026 Ben Leone — licensed under the [GNU General Public License v3.0](LICENSE).

A Linux gaming input remapper with per-game profiles, precise macro timing, and support for gaming mice with extra buttons. Run it when you want remapping; close it to stop.

---

## Features

- **Per-game profiles** — automatically switch remaps when a game launches (Steam and Proton supported)
- **Macros** — record or hand-craft key sequences with precise millisecond timing; loop them while a button is held
- **Modifier layers** — hold a key to activate a completely different set of bindings
- **Extra mouse buttons** — full support for MMO mice and gaming keyboards with G-keys (see [Supported Devices](SUPPORTED_DEVICES.md))
- **Low latency** — written in Rust, reads directly from evdev, no X11 or Wayland dependency
- **Portable** — ships as a single AppImage; no installation required beyond a one-time permission setup

---

## Supported Devices

See [SUPPORTED_DEVICES.md](SUPPORTED_DEVICES.md) for the full list of tested hardware and notes on requesting support for new devices.

---

## Requirements

- Linux (kernel 4.15+)
- Your user account in the `input` group (the app will offer to set this up for you)
- `uinput` kernel module loaded (`sudo modprobe uinput`)

---

## Getting Started

> Release builds and AppImage downloads are available on the [Releases](https://github.com/ben-leone/linux-gaming-input-remapper/releases) page.

To build from source:

```sh
cargo build --release
```

### First Run

Launch the profile editor:

```sh
./gameremap profile
```

On first launch, if your user is not in the `input` group the app will prompt you to run a one-time setup (requires polkit/pkexec). After setup you will need to log out and back in.

---

## Usage

```
gameremap <subcommand>
```

| Subcommand | Description |
|------------|-------------|
| `profile` | Open the graphical profile and macro editor |
| `debug` | Open the live key event monitor (useful for identifying key names) |
| `monitor` | Watch the remapper's output device and print injected events |
| `diagnose` | Print device enumeration and key name diagnostics |

### Typical workflow

1. Run `gameremap profile` to open the editor
2. Create a profile, add macros and key assignments
3. Click **Run** — remapping starts immediately
4. Alt-tab to your game and play; the editor window can stay open or minimized
5. Click **Stop** to pause remapping, or close the window to stop everything
6. Hold **Escape for 7 seconds** at any time to force-quit if the remapper gets stuck

---

## How It Works

GameRemapper reads raw input directly from `/dev/input/event*` using the Linux evdev interface. When a profile is active it grabs the relevant devices (so raw events don't reach games), applies your macros and remaps, and re-injects the result through a virtual uinput device that games see normally. Everything runs inside the profile editor process — closing the window stops all remapping cleanly.

Extra buttons on supported gaming mice and keyboards are handled separately via the hidraw interface and injected as standard Linux macro key events (`KEY_MACRO1` and up), which you can then assign in the profile editor like any other key.

---


## Contributors

| Contributor | Role |
|-------------|------|
| Ben Leone | Author |
| [Claude](https://claude.ai) (Anthropic) | AI pair programmer |
| Boots | Senior Code Reviewer |
| Rabbit | Senior Code Reviewer |
| Google Gemini | App icon |

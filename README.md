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

## One-Time Permission Setup

GameRemapper needs read access to `/dev/input/event*` (raw input) and write access to `/dev/uinput` and `/dev/hidraw*` (virtual device injection). On first launch the app checks whether it can open these nodes. If it cannot, it prompts you to run a one-time setup via a polkit (pkexec) dialog — no terminal required.

### What the setup does

The embedded setup script runs as root and performs three steps:

1. **Adds your user to the `input` group**
   ```sh
   usermod -aG input <your-username>
   ```
   You must log out and back in (or reboot) for the group membership to take effect.

2. **Installs a udev rules file** at `/etc/udev/rules.d/99-gameremap.rules`:
   ```
   KERNEL=="event*", SUBSYSTEM=="input", GROUP="input", MODE="0660"
   KERNEL=="uinput",                     GROUP="input", MODE="0660"
   KERNEL=="hidraw*",                    GROUP="input", MODE="0660"
   ```
   These rules set the group owner and permissions on input, uinput, and hidraw nodes so any member of `input` can access them without sudo.

3. **Reloads udev** and re-triggers the input subsystem so the new rules apply to already-present devices immediately:
   ```sh
   udevadm control --reload
   udevadm trigger --subsystem-match=input
   ```

### Teardown

The teardown option (available in the app settings) reverses step 2 — it deletes `/etc/udev/rules.d/99-gameremap.rules` and reloads udev. It does not remove you from the `input` group; do that manually if desired:

```sh
sudo gpasswd -d $USER input
```

### Doing it yourself

If you prefer not to use polkit, or want to apply the setup on a headless machine, run these commands directly:

```sh
# 1. Add your user to the input group
sudo usermod -aG input $USER

# 2. Write the udev rules
sudo tee /etc/udev/rules.d/99-gameremap.rules << 'EOF'
KERNEL=="event*", SUBSYSTEM=="input", GROUP="input", MODE="0660"
KERNEL=="uinput",                     GROUP="input", MODE="0660"
KERNEL=="hidraw*",                    GROUP="input", MODE="0660"
EOF

# 3. Apply the rules immediately
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input

# 4. Log out and back in (or reboot) for the group change to take effect
```

To undo manually:
```sh
sudo rm /etc/udev/rules.d/99-gameremap.rules
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input
sudo gpasswd -d $USER input   # optional: remove from input group
```

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

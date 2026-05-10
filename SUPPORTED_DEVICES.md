# Supported Devices

Standard keyboards and mice work out of the box through the normal Linux evdev interface — no special setup needed.

Some gaming peripherals expose extra buttons that don't appear in the standard evdev stream. These require a custom HID driver to decode the raw reports and inject them as remappable keys. The devices below have been tested personally and are fully supported.

---

## Extra-Button Devices (custom HID driver)

These devices have buttons beyond what evdev exposes natively. GameRemapper includes a driver for each that decodes the raw HID reports and injects the extra buttons as standard `KEY_MACRO` events, which you can assign in the profile editor like any other key.

| Device | Extra Buttons |
|--------|---------------|
| Corsair K95 RGB | G1–G18 |
| UTechSmart Venus MMO Mouse | M1–M12 side buttons |

---

## Standard HID Devices (tested, no driver needed)

These devices work through evdev as-is. They are listed here because they have been tested end-to-end with GameRemapper.

| Device | Notes |
|--------|-------|
| Razer Orbweaver | All keys fully remappable |
| Corsair M65 RGB Ultra | Standard buttons fully remappable; the sniper button is a hardware DPI modifier handled entirely in firmware and cannot be addressed in software |

---

## Requesting Support for a New Device

Device coverage is limited to hardware I personally own. If you'd like your device supported, [open a feature request](https://github.com/ben-leone/linux-gaming-input-remapper/issues). Adding support requires decoding the device's raw HID reports and writing a small driver — the more detail you can provide (HID descriptor, raw report captures via `hid-recorder` or `usbhid-dump`), the faster it can be added.

# Keywork management tool

The kernel exposes KWMP on USB interface 2 through bulk OUT endpoint `0x02`
and bulk IN endpoint `0x82`.

On Arch Linux:

```bash
sudo pacman -S python-pyusb
sudo install -m 0644 tools/70-keywork.rules /etc/udev/rules.d/70-keywork.rules
sudo udevadm control --reload-rules
```

Reconnect Keywork after installing the rule.

## Device information

```bash
python tools/keywork.py info
```

## Firmware staging

The first implementation is intended for development. It:

1. uploads into RAM in chunks;
2. verifies the host-provided SHA-256;
3. writes `kernel8.new` on the boot filesystem;
4. re-reads and verifies the staged file;
5. reports `PendingReboot`.

It accepts images up to 3 MiB because the current kernel heap is 4 MiB.
Signatures are not implemented yet, so the host sends an empty signature.

Stage without activating:

```bash
python tools/keywork.py firmware target/aarch64-unknown-none/release/kernel8.img
```

Stage, verify, replace `kernel8.img`, and reboot:

```bash
python tools/keywork.py firmware target/aarch64-unknown-none/release/kernel8.img --reboot
```

Check the current transfer state:

```bash
python tools/keywork.py firmware-status
```

This is a staged file swap (`kernel8.img` / `kernel8.new` / `kernel8.old`), not
yet a true power-loss-safe A/B boot scheme. Do not remove power during commit.

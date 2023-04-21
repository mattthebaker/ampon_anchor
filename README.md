# Ampon Anchor Firmware

This project is a firmware for the [Provok3d ADXL Shaper](https://provok3d.com/product/nozzle-adxl-board/), an accelerometer resonance measuring device, written in Rust using the [Anchor](https://github.com/Annex-Engineering/anchor) library to speak with Klipper.

It demonstrates how firmware can be rapidly developed for new sensors or new MCU targets using Anchor.

## Loading Release Builds

Checkout the repository and load your device using:
```
% git clone https://github.com/mattthebaker/ampon_anchor.git
% ./ampon_anchor/update.sh
```

After the update completes, the device should be available as:
`/dev/serial/by-id/usb-Anchor_Ampon-if00`.

## Klipper Config

```ini
[mcu ampon]
serial: /dev/serial/by-id/usb-Anchor_Ampon-if00

[adxl345]
cs_pin: ampon:CS

[resonance_tester]
accel_chip: adxl345
probe_points: 90, 90, 20
```

## Developers

---

## Building Firmware

To compile the project, you will need a Rust toolchain installed, `cargo-binutils`, and the compile target for ARM Cortex-M0. They can be installed with:

```
% rustup component add llvm-tools-preview
% rustup target add thumbv6m-none-eabi
```

To build the project, and convert the executable to a `.bin` file, run:
```
% cargo build --release
% rust-objcopy target/thumbv6m-none-eabi/release/ampon_anchor -O binary ampon.bin
```

To enter the bootloader and program the device, run:
```
% stty -F /dev/serial/by-id/usb-Anchor_Ampon-if00 1200
% sudo dfu-util -d ,0483:df11 -a 0 -D ampon.bin --dfuse-address 0x08000000:leave
```

After the update completes, the device should be available as:
`/dev/serial/by-id/usb-Anchor_Ampon-if00`.

## Credits / Related Projects

Ampon is a derivative of [Annex Engineering]'s [crampon_anchor], name inspired by [rampon_anchor]

[Annex Engineering]: <https://github.com/Annex-Engineering>
[crampon_anchor]: <https://github.com/Annex-Engineering/crampon_anchor>
[rampon_anchor]: <https://github.com/rogerlz/rampon_anchor>

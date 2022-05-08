build:
    cargo objcopy --release --bin keyboard-thing -- -O binary target/out.bin
    uf2conv target/out.bin --family 0xADA52840 --base 0x1000 --output firmware.uf2

left:
    cargo objcopy --no-default-features --release --bin left -- -O binary target/out_left.bin
    uf2conv target/out_left.bin --family 0xADA52840 --base 0x1000 --output firmware_left.uf2
right:
    cargo objcopy --no-default-features --release --bin right -- -O binary target/out_right.bin
    uf2conv target/out_right.bin --family 0xADA52840 --base 0x1000 --output firmware_right.uf2

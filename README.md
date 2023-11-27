# Some firmware for a corne v3 using two nice!nanos

![image](https://user-images.githubusercontent.com/5330444/171196800-3247e66a-f399-47ae-921f-89090905b679.png)

https://user-images.githubusercontent.com/5330444/172443165-bc76f323-c769-49e6-9992-025ef0be5f02.mp4

## Required mods

The corne v3 uses only one of the TR(R)S pins, but this firmware needs both data
pins to use a uart connection (I don't feel like doing what QMK does), you'll
need to add a bodge wire between P1.04 and the unused pin of the TRRS jack (it's
the one not part of the group of three pins in a line)

## Flashing

It's possible to create uf2 files for rust firmware by using the following `memory.x`:

```
MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  FLASH : ORIGIN = 0x00026000, LENGTH = 868K
  RAM : ORIGIN = 0x20020000, LENGTH = 128K
}
```

You can then use my fork of elf2uf2-rs to convert to uf2: https://github.com/simmsb/elf2uf2-rs

`elf2uf2-rs target/thumbv7em-none-eabihf/release/left left.uf2`

Make sure the softdevice hasn't been wiped from the nice!nano (you can just reflash it if it has)

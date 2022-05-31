# Some firmware for a corne v3 using two nice!nanos

![image](https://user-images.githubusercontent.com/5330444/171196800-3247e66a-f399-47ae-921f-89090905b679.png)

## Required mods

The corne v3 uses only one of the TR(R)S pins, but this firmware needs both data
pins to use a uart connection (I don't feel like doing what QMK does), you'll
need to add a bodge wire between P1.04 and the unused pin of the TRRS jack (it's
the one not part of the group of three pins in a line)

## Flashing

~~I have some scripts in [here](justfile) for generating UF2 files that can be
used to flash the nice!nanos, there's no need for a debugger.~~

I never figured out how to make a flashable UF2 when using embassy, you'll want
a st-link/j-link to flash the nice!nanos.

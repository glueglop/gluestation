**gluestation**

A PlayStation 1 emulator written in Rust. It boots the BIOS, draws the Sony logo, and plays the startup chime (badly, for now).

Devlog: https://glueglop.github.io/gluestation/

Status
------

Works:

MIPS R3000A CPU with branch and load delay slots, COP0 exceptions and interrupts
BIOS boots from the reset vector

GPU rasterizer drawing into 1024x512 VRAM (polygons, rectangles, lines, textures with CLUTs, semi-transparency)

GTE (the geometry coprocessor)

DMA (GPU, SPU, ordering table clear), root counters, interrupt controller

SPU with ADPCM voices and native audio output

Not done yet:
-------------

Audio quality: no ADSR envelopes, no interpolation, no reverb, so the startup chime sounds distorted

CD-ROM: the controller pretends there is no disc, so games can't load

Controllers and memory cards

Timer sync modes

Anything that isn't Linux

Requirements
------------
Linux with X11 (no Wayland-only setups)

A GPU and driver that support OpenGL 3.3 core through GLX

ALSA (libasound.so.2). PipeWire and PulseAudio work through their ALSA compatibility layers

A recent Rust toolchain

A PS1 BIOS dump (see below)

If audio can't be opened, the emulator prints a message and keeps running without sound.

BIOS
----

You need to supply your own BIOS file.
No BIOS or game files are included or distributed here, so dump one from a console you own. 

The emulator defaults to a file named SCPH1001.BIN (512 KB) in the current directory.

Tested with SCPH-1001 BIOS, so if you would like to test with any others, replace the file path with the name of your BIOS file, and create an issue or email me at glueglop[at]protonmail[dot]com. 

Build and run
bash
cargo run --release -- SCPH1001.BIN

The path argument is optional if the file is called SCPH1001.BIN and sits where you run the command. Always use --release, since the emulator steps the CPU one cycle at a time and a debug build will be far too slow.

Run the unit tests with:

bash
cargo test
Controls
Key	Action
Tab	Toggle between the 4:3 TV view and the full 1024x512 VRAM view
Esc	Quit

Closing the window also quits.

Audio debugging

The first five seconds of audio are saved to chime.wav in the current directory so you can inspect what the SPU is producing.

What each file does
-------------------

cpu.rs	R3000A CPU, COP0, hooks into the GTE

gte.rs	Geometry Transformation Engine (COP2)

bus.rs	Memory map, interrupts, unmapped access counters

gpu.rs	GP0/GP1 commands and software rasterizer

dma.rs	DMA channels

spu.rs	Sound unit and ADPCM decoding

cdrom.rs	CD-ROM controller (no disc)

timers.rs	Root counters

ram.rs, bios.rs	Main RAM and BIOS ROM

renderer.rs	X11/GLX window and OpenGL output

audio.rs	ALSA output, loaded at runtime with dlopen

main.rs	Main loop and frame timing

The command I like to use to compile and run it is:
"cargo run --release -- _________ (bios file)".
The blank depends on your PlayStation model/name of your BIOS dump.

Disclaimer:
PlayStation is a trademark of Sony Interactive Entertainment.
This project is not affiliated with or endorsed by Sony.

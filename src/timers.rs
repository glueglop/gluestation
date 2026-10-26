// Root counters 0..2.
//
// Clock sources (approximate, NTSC):
//   Timer 0: system clock, or dot clock (mode bit 8)      ~ sysclk / 6
//   Timer 1: system clock, or HBlank (mode bit 8)         ~ sysclk / 2146
//   Timer 2: system clock, or sysclk / 8 (mode bit 9)
//
// Sync modes (mode bit 0..2) are not implemented: counters free-run.

#[derive(Default, Clone, Copy)]
pub struct Timer {
    pub value: u32,
    pub mode: u32,
    pub target: u32,
    frac: u32,
    irq_done: bool,
}

pub struct Timers {
    pub timers: [Timer; 3],
}

impl Timers {
    pub fn new() -> Self {
        Self {
            timers: [Timer::default(); 3],
        }
    }

    /// Advance all counters by `cycles` CPU cycles.
    /// Returns a bitmask of timers that raised an IRQ (bit n = timer n).
    pub fn tick(&mut self, cycles: u32) -> u32 {
        let mut irqs = 0u32;

        for i in 0..3 {
            let t = &mut self.timers[i];

            let div: u32 = match i {
                0 => {
                    if t.mode & 0x100 != 0 {
                        6
                    } else {
                        1
                    }
                }
                1 => {
                    if t.mode & 0x100 != 0 {
                        2146
                    } else {
                        1
                    }
                }
                _ => {
                    if t.mode & 0x200 != 0 {
                        8
                    } else {
                        1
                    }
                }
            };

            t.frac += cycles;
            let inc = t.frac / div;
            t.frac %= div;

            for _ in 0..inc {
                t.value += 1;
                let mut fire = false;

                // Target reached
                if t.value == t.target {
                    t.mode |= 1 << 11;
                    if t.mode & (1 << 3) != 0 {
                        t.value = 0;
                    }
                    if t.mode & (1 << 4) != 0 {
                        fire = true;
                    }
                }

                // 16-bit overflow
                if t.value > 0xFFFF {
                    t.value = 0;
                    t.mode |= 1 << 12;
                    if t.mode & (1 << 5) != 0 {
                        fire = true;
                    }
                }

                // Bit 6: repeat IRQs; otherwise only fire once per mode write
                if fire && ((t.mode & (1 << 6)) != 0 || !t.irq_done) {
                    t.irq_done = true;
                    irqs |= 1 << i;
                }
            }
        }

        irqs
    }

    pub fn read32(&mut self, offset: u32) -> u32 {
        let timer_idx = (offset / 0x10) as usize;
        let reg = offset % 0x10;

        if timer_idx > 2 {
            return 0;
        }

        let t = &mut self.timers[timer_idx];
        match reg {
            0x0 => t.value & 0xFFFF,
            0x4 => {
                let m = t.mode;
                // Bits 11 and 12 (reached target / overflow) clear on read
                t.mode &= !((1 << 11) | (1 << 12));
                // Bit 10 = 1 means "no IRQ request"
                m | (1 << 10)
            }
            0x8 => t.target,
            _ => 0,
        }
    }

    pub fn read16(&mut self, offset: u32) -> u16 {
        self.read32(offset) as u16
    }

    pub fn write32(&mut self, offset: u32, val: u32) {
        let timer_idx = (offset / 0x10) as usize;
        let reg = offset % 0x10;

        if timer_idx > 2 {
            return;
        }

        let t = &mut self.timers[timer_idx];
        match reg {
            0x0 => {
                t.value = val & 0xFFFF;
            }
            0x4 => {
                t.mode = val & 0x3FF;
                // Writing to mode resets the counter
                t.value = 0;
                t.frac = 0;
                t.irq_done = false;
            }
            0x8 => {
                t.target = val & 0xFFFF;
            }
            _ => {}
        }
    }

    pub fn write16(&mut self, offset: u32, val: u16) {
        self.write32(offset, val as u32);
    }
}

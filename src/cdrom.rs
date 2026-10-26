// Minimal CD-ROM controller: behaves like a console with NO DISC inserted.
// Registers (byte-wide) at 0x1F801800..0x1F801803, banked by an index (0-3).
//
//   0x1F801800  R: status      W: index
//   0x1F801801  R: response    W: command (index 0)
//   0x1F801802  R: data        W: parameter (index 0) / interrupt enable (index 1)
//   0x1F801803  R: int enable (idx 0,2) / int flag (idx 1,3)
//               W: request (idx 0) / int flag acknowledge (idx 1)

use std::collections::VecDeque;

/// Status byte reported when no disc is present. Bit 4 = shell (lid) open.
/// If the BIOS behaves oddly, try 0x00 or 0x02 here.
const STAT_NO_DISC: u8 = 0x10;

/// Delay (CPU cycles) before the first and second responses to a command.
const FIRST_RESPONSE_CYCLES: u32 = 50_000;
const SECOND_RESPONSE_CYCLES: u32 = 40_000;

/// How many commands to log to the console.
const LOG_LIMIT: u32 = 300;

struct Pending {
    delay: u32,
    int_type: u8,
    data: Vec<u8>,
}

pub struct CdRom {
    index: u8,
    param_fifo: VecDeque<u8>,
    response_fifo: VecDeque<u8>,
    int_enable: u8,
    int_flag: u8,
    pending: VecDeque<Pending>,
    irq_out: bool,
    log_count: u32,
}

impl CdRom {
    pub fn new() -> Self {
        Self {
            index: 0,
            param_fifo: VecDeque::new(),
            response_fifo: VecDeque::new(),
            int_enable: 0,
            int_flag: 0,
            pending: VecDeque::new(),
            irq_out: false,
            log_count: 0,
        }
    }

    /// Returns true (once) if the CD-ROM wants to raise IRQ2.
    pub fn take_irq(&mut self) -> bool {
        std::mem::take(&mut self.irq_out)
    }

    pub fn read(&mut self, offset: u32) -> u8 {
        match offset {
            0 => {
                let mut s = self.index & 3;
                if self.param_fifo.is_empty() {
                    s |= 0x08; // parameter FIFO empty
                }
                if self.param_fifo.len() < 16 {
                    s |= 0x10; // parameter FIFO not full
                }
                if !self.response_fifo.is_empty() {
                    s |= 0x20; // response FIFO not empty
                }
                s
            }
            1 => self.response_fifo.pop_front().unwrap_or(0),
            2 => 0, // data FIFO (no disc, nothing to read)
            3 => match self.index & 1 {
                0 => self.int_enable | 0xE0,
                _ => self.int_flag | 0xE0,
            },
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, val: u8) {
        match offset {
            0 => self.index = val & 3,
            1 => {
                if self.index == 0 {
                    self.command(val);
                }
            }
            2 => match self.index {
                0 => {
                    if self.param_fifo.len() < 16 {
                        self.param_fifo.push_back(val);
                    }
                }
                1 => {
                    self.int_enable = val & 0x1F;
                    if self.int_flag & self.int_enable != 0 {
                        self.irq_out = true;
                    }
                }
                _ => {}
            },
            3 => {
                if self.index == 1 {
                    // Acknowledge interrupt bits; bit 6 resets the parameter FIFO
                    self.int_flag &= !(val & 0x1F);
                    if val & 0x40 != 0 {
                        self.param_fifo.clear();
                    }
                }
            }
            _ => {}
        }
    }

    fn queue(&mut self, delay: u32, int_type: u8, data: Vec<u8>) {
        self.pending.push_back(Pending {
            delay,
            int_type,
            data,
        });
    }

    fn command(&mut self, cmd: u8) {
        let params: Vec<u8> = self.param_fifo.drain(..).collect();

        if self.log_count < LOG_LIMIT {
            self.log_count += 1;
            println!("  [CDROM] command 0x{:02X} params {:02X?}", cmd, params);
        }

        let stat = STAT_NO_DISC;
        self.pending.clear(); // a new command cancels any outstanding second response

        match cmd {
            // GetStat, Setloc, Mute, Demute, Setfilter, Setmode
            0x01 | 0x02 | 0x0B | 0x0C | 0x0D | 0x0E => {
                self.queue(FIRST_RESPONSE_CYCLES, 3, vec![stat]);
            }
            // Stop, Pause, Init: INT3 then INT2
            0x08 | 0x09 | 0x0A => {
                self.queue(FIRST_RESPONSE_CYCLES, 3, vec![stat]);
                self.queue(SECOND_RESPONSE_CYCLES, 2, vec![stat]);
            }
            // Test
            0x19 => {
                if params.first() == Some(&0x20) {
                    // BIOS version/date (SCPH-1001: 94/09/19, C0)
                    self.queue(FIRST_RESPONSE_CYCLES, 3, vec![0x94, 0x09, 0x19, 0xC0]);
                } else {
                    self.queue(FIRST_RESPONSE_CYCLES, 5, vec![stat | 1, 0x10]);
                }
            }
            // GetID: no disc -> INT3 then INT5 (error: no disc)
            0x1A => {
                self.queue(FIRST_RESPONSE_CYCLES, 3, vec![stat]);
                self.queue(SECOND_RESPONSE_CYCLES, 5, vec![0x11, 0x80]);
            }
            // Commands that need a disc: Play, ReadN, GetlocL, GetlocP, GetTN, GetTD,
            // SeekL, SeekP, ReadS, ReadTOC
            0x03 | 0x06 | 0x10 | 0x11 | 0x13 | 0x14 | 0x15 | 0x16 | 0x1B | 0x1E => {
                self.queue(FIRST_RESPONSE_CYCLES, 5, vec![stat | 1, 0x80]);
            }
            // Unknown command: error
            _ => {
                self.queue(FIRST_RESPONSE_CYCLES, 5, vec![stat | 1, 0x40]);
            }
        }
    }

    /// Advance by `cycles` CPU cycles; delivers queued responses.
    pub fn tick(&mut self, cycles: u32) {
        let ready = match self.pending.front_mut() {
            None => return,
            Some(p) => {
                if p.delay > cycles {
                    p.delay -= cycles;
                    false
                } else {
                    p.delay = 0;
                    true
                }
            }
        };

        // The next response waits until the previous interrupt has been acknowledged
        if !ready || self.int_flag != 0 {
            return;
        }

        let p = self.pending.pop_front().unwrap();
        self.response_fifo.clear();
        self.response_fifo.extend(p.data);
        self.int_flag = p.int_type;
        if self.int_flag & self.int_enable != 0 {
            self.irq_out = true;
        }
    }
}
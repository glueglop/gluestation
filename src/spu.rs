pub const SPU_RAM_SIZE: usize = 512 * 1024;

const POS_ADPCM_TABLE: [i32; 5] = [0, 60, 115, 98, 122];
const NEG_ADPCM_TABLE: [i32; 5] = [0, 0, -52, -55, -60];

#[derive(Default, Clone, Copy)]
pub struct Voice {
    pub vol_l: i16,
    pub vol_r: i16,
    pub sample_rate: u16,
    pub start_addr: u32,
    pub cur_addr: u32,
    pub repeat_addr: u32,

    pub on: bool,
    pub adpcm_old1: i32,
    pub adpcm_old2: i32,
    pub sample_buffer: [i16; 28],
    pub sample_index: usize,
    pub counter: u32,
}

impl Voice {
    fn decode_block(&mut self, ram: &[u8]) {
        let addr = (self.cur_addr as usize) & (SPU_RAM_SIZE - 1);
        if addr + 16 > SPU_RAM_SIZE {
            self.on = false;
            return;
        }

        let shift = (ram[addr] & 0x0F).min(12) as i32;
        let filter = ((ram[addr] >> 4) & 0x07).min(4) as usize;
        let flags = ram[addr + 1];

        let f0 = POS_ADPCM_TABLE[filter];
        let f1 = NEG_ADPCM_TABLE[filter];

        let mut out_idx = 0;
        for i in 2..16 {
            let byte = ram[addr + i];
            for nibble in [byte & 0x0F, byte >> 4] {
                let nibble_s = ((nibble as i16) << 12) >> 12;
                let sample = ((nibble_s as i32) << 12) >> shift;

                let mut s = sample + ((self.adpcm_old1 * f0 + self.adpcm_old2 * f1 + 32) >> 6);
                s = s.clamp(-32768, 32767);

                self.adpcm_old2 = self.adpcm_old1;
                self.adpcm_old1 = s;
                self.sample_buffer[out_idx] = s as i16;
                out_idx += 1;
            }
        }

        self.cur_addr = (self.cur_addr + 16) & ((SPU_RAM_SIZE as u32) - 1);

        if flags & 4 != 0 {
            self.repeat_addr = addr as u32;
        }
        if flags & 1 != 0 {
            if flags & 2 != 0 {
                self.cur_addr = self.repeat_addr;
            } else {
                self.on = false;
            }
        }
    }

    fn step(&mut self, ram: &[u8]) -> (i32, i32) {
        if !self.on || (self.vol_l == 0 && self.vol_r == 0) {
            return (0, 0);
        }

        if self.sample_index >= 28 {
            self.decode_block(ram);
            self.sample_index = 0;
            if !self.on {
                return (0, 0);
            }
        }

        let s = self.sample_buffer[self.sample_index] as i32;

        let rate = if self.sample_rate == 0 { 0x1000 } else { self.sample_rate as u32 };
        self.counter += rate;

        while self.counter >= 0x1000 {
            self.counter -= 0x1000;
            self.sample_index += 1;
            if self.sample_index >= 28 {
                self.decode_block(ram);
                self.sample_index = 0;
                if !self.on {
                    break;
                }
            }
        }

        let left = (s * (self.vol_l as i32)) >> 14;
        let right = (s * (self.vol_r as i32)) >> 14;
        (left, right)
    }
}

pub struct Spu {
    pub ram: Box<[u8; SPU_RAM_SIZE]>,
    pub voices: [Voice; 24],
    pub main_vol_l: i16,
    pub main_vol_r: i16,
    pub spucnt: u16,
    pub spustat: u16,
    pub ram_dma_addr: u32,
    pub out_samples: Vec<i16>,
}

impl Spu {
    pub fn new() -> Self {
        Self {
            ram: vec![0u8; SPU_RAM_SIZE].into_boxed_slice().try_into().unwrap(),
            voices: [Voice::default(); 24],
            main_vol_l: 0x3FFF,
            main_vol_r: 0x3FFF,
            spucnt: 0xC000,
            spustat: 0,
            ram_dma_addr: 0,
            out_samples: Vec::with_capacity(2048),
        }
    }

    pub fn take_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.out_samples)
    }

    pub fn step_samples(&mut self, samples_to_generate: usize) {
        let master_l = if self.main_vol_l != 0 { self.main_vol_l as i32 } else { 0x3FFF };
        let master_r = if self.main_vol_r != 0 { self.main_vol_r as i32 } else { 0x3FFF };

        for _ in 0..samples_to_generate {
            let mut mix_l: i32 = 0;
            let mut mix_r: i32 = 0;

            for v in self.voices.iter_mut() {
                if v.on {
                    let (vl, vr) = v.step(&self.ram[..]);
                    mix_l += vl;
                    mix_r += vr;
                }
            }

            let out_l = (((mix_l * master_l) >> 15).clamp(-32768, 32767)) as i16;
            let out_r = (((mix_r * master_r) >> 15).clamp(-32768, 32767)) as i16;

            self.out_samples.push(out_l);
            self.out_samples.push(out_r);
        }
    }

    pub fn read16(&mut self, offset: u32) -> u16 {
        match offset {
            0x000..=0x17F => {
                let voice_idx = (offset / 0x10) as usize;
                let reg = offset % 0x10;
                match reg {
                    0x0 => self.voices[voice_idx].vol_l as u16,
                    0x2 => self.voices[voice_idx].vol_r as u16,
                    0x4 => self.voices[voice_idx].sample_rate,
                    0x6 => (self.voices[voice_idx].start_addr >> 3) as u16,
                    0xE => (self.voices[voice_idx].repeat_addr >> 3) as u16,
                    _ => 0,
                }
            }
            0x180 => self.main_vol_l as u16,
            0x182 => self.main_vol_r as u16,
            0x188 => {
                let mut v = 0u16;
                for i in 0..16 {
                    if self.voices[i].on { v |= 1 << i; }
                }
                v
            }
            0x18A => {
                let mut v = 0u16;
                for i in 16..24 {
                    if self.voices[i].on { v |= 1 << (i - 16); }
                }
                v
            }
            0x1AA => self.spucnt,
            0x1AE => self.spustat,
            _ => 0,
        }
    }

    pub fn write16(&mut self, offset: u32, val: u16) {
        match offset {
            0x000..=0x17F => {
                let voice_idx = (offset / 0x10) as usize;
                let reg = offset % 0x10;
                match reg {
                    0x0 => self.voices[voice_idx].vol_l = val as i16,
                    0x2 => self.voices[voice_idx].vol_r = val as i16,
                    0x4 => self.voices[voice_idx].sample_rate = val,
                    0x6 => {
                        self.voices[voice_idx].start_addr = (val as u32) << 3;
                        self.voices[voice_idx].cur_addr = self.voices[voice_idx].start_addr;
                        self.voices[voice_idx].repeat_addr = self.voices[voice_idx].start_addr;
                    }
                    0xE => self.voices[voice_idx].repeat_addr = (val as u32) << 3,
                    _ => {}
                }
            }
            0x180 => self.main_vol_l = val as i16,
            0x182 => self.main_vol_r = val as i16,
            0x188 => {
                for i in 0..16 {
                    if (val & (1 << i)) != 0 {
                        self.voices[i].on = true;
                        self.voices[i].cur_addr = self.voices[i].start_addr;
                        self.voices[i].sample_index = 28;
                        self.voices[i].adpcm_old1 = 0;
                        self.voices[i].adpcm_old2 = 0;
                    }
                }
            }
            0x18A => {
                for i in 0..8 {
                    if (val & (1 << i)) != 0 {
                        self.voices[16 + i].on = true;
                        self.voices[16 + i].cur_addr = self.voices[16 + i].start_addr;
                        self.voices[16 + i].sample_index = 28;
                        self.voices[16 + i].adpcm_old1 = 0;
                        self.voices[16 + i].adpcm_old2 = 0;
                    }
                }
            }
            0x18C => {
                for i in 0..16 {
                    if (val & (1 << i)) != 0 {
                        self.voices[i].on = false;
                    }
                }
            }
            0x18E => {
                for i in 0..8 {
                    if (val & (1 << i)) != 0 {
                        self.voices[16 + i].on = false;
                    }
                }
            }
            0x1A6 => {
                self.ram_dma_addr = (val as u32) << 3;
            }
            0x1A8 => {
                let addr = (self.ram_dma_addr as usize) & (SPU_RAM_SIZE - 1);
                let bytes = val.to_le_bytes();
                self.ram[addr] = bytes[0];
                self.ram[(addr + 1) & (SPU_RAM_SIZE - 1)] = bytes[1];
                self.ram_dma_addr = (self.ram_dma_addr + 2) & ((SPU_RAM_SIZE as u32) - 1);
            }
            0x1AA => {
                self.spucnt = val;
                self.spustat = (self.spustat & !0x3F) | (val & 0x3F);
            }
            _ => {}
        }
    }
}

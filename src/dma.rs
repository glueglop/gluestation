use crate::gpu::Gpu;
use crate::ram::Ram;
use crate::spu::Spu;

const RAM_ADDR_MASK: u32 = 0x001F_FFFC;

#[derive(Default, Clone, Copy)]
pub struct DmaChannel {
    pub madr: u32,
    pub bcr: u32,
    pub chcr: u32,
}

pub struct Dma {
    pub channels: [DmaChannel; 7],
    pub dpcr: u32,
    pub dicr: u32,
    irq_active: bool,
}

impl Dma {
    pub fn new() -> Self {
        Self {
            channels: [DmaChannel::default(); 7],
            dpcr: 0x0765_4321,
            dicr: 0,
            irq_active: false,
        }
    }

    fn dicr_master(&self) -> bool {
        let force = self.dicr & (1 << 15) != 0;
        let master_en = self.dicr & (1 << 23) != 0;
        let enables = (self.dicr >> 16) & 0x7F;
        let flags = (self.dicr >> 24) & 0x7F;
        force || (master_en && (enables & flags) != 0)
    }

    fn dicr_read(&self) -> u32 {
        (self.dicr & 0x7FFF_FFFF) | ((self.dicr_master() as u32) << 31)
    }

    fn update_irq(&mut self) -> bool {
        let now = self.dicr_master();
        let rose = now && !self.irq_active;
        self.irq_active = now;
        rose
    }

    fn complete(&mut self, ch: usize) {
        self.channels[ch].chcr &= !((1 << 24) | (1 << 28));
        if (self.dicr >> (16 + ch)) & 1 != 0 {
            self.dicr |= 1 << (24 + ch);
        }
    }

    pub fn read32(&self, offset: u32) -> u32 {
        match offset {
            0x00..=0x6C => {
                let channel_idx = (offset / 0x10) as usize;
                let reg = offset % 0x10;
                match reg {
                    0x0 => self.channels[channel_idx].madr,
                    0x4 => self.channels[channel_idx].bcr,
                    0x8 => self.channels[channel_idx].chcr,
                    _ => 0,
                }
            }
            0x70 => self.dpcr,
            0x74 => self.dicr_read(),
            _ => 0,
        }
    }

    pub fn write32(&mut self, offset: u32, val: u32, ram: &mut Ram, gpu: &mut Gpu, spu: &mut Spu) -> bool {
        match offset {
            0x00..=0x6C => {
                let channel_idx = (offset / 0x10) as usize;
                let reg = offset % 0x10;
                match reg {
                    0x0 => self.channels[channel_idx].madr = val & 0x00FF_FFFF,
                    0x4 => self.channels[channel_idx].bcr = val,
                    0x8 => {
                        self.channels[channel_idx].chcr = val;
                        let trigger = (val & 0x0100_0000) != 0;
                        if trigger {
                            self.run_dma(channel_idx, ram, gpu, spu);
                        }
                    }
                    _ => {}
                }
            }
            0x70 => self.dpcr = val,
            0x74 => {
                let ack = val & 0x7F00_0000;
                self.dicr = (val & 0x00FF_803F) | (self.dicr & 0x7F00_0000 & !ack);
            }
            _ => {}
        }

        self.update_irq()
    }

    fn run_dma(&mut self, channel: usize, ram: &mut Ram, gpu: &mut Gpu, spu: &mut Spu) {
        let chcr = self.channels[channel].chcr;
        let to_ram = chcr & 1 == 0;
        let step: u32 = if chcr & 2 != 0 { 0xFFFF_FFFC } else { 4 };
        let sync_mode = (chcr >> 9) & 3;

        match channel {
            2 => {
                let bcr = self.channels[2].bcr;
                match sync_mode {
                    0 | 1 => {
                        let block_size = if bcr & 0xFFFF == 0 { 0x10000 } else { bcr & 0xFFFF };
                        let blocks = (bcr >> 16) & 0xFFFF;
                        let count = if sync_mode == 0 {
                            block_size
                        } else if blocks == 0 {
                            block_size
                        } else {
                            blocks * block_size
                        };

                        let mut addr = self.channels[2].madr & RAM_ADDR_MASK;
                        for _ in 0..count {
                            if to_ram {
                                let word = gpu.read_gp0();
                                ram.write32(addr, word);
                            } else {
                                let word = ram.read32(addr);
                                gpu.write_gp0(word);
                            }
                            addr = addr.wrapping_add(step) & RAM_ADDR_MASK;
                        }
                        self.channels[2].madr = addr;
                    }
                    2 => {
                        let mut cur_addr = self.channels[2].madr & RAM_ADDR_MASK;
                        let mut nodes_left: u32 = 0x0004_0000;

                        loop {
                            let header = ram.read32(cur_addr);
                            let packet_len = (header >> 24) & 0xFF;

                            for i in 0..packet_len {
                                let word = ram.read32((cur_addr + 4 + i * 4) & RAM_ADDR_MASK);
                                gpu.write_gp0(word);
                            }

                            if (header & 0x0080_0000) != 0 || (header & 0x00FF_FFFF) == 0x00FF_FFFF {
                                break;
                            }
                            cur_addr = header & RAM_ADDR_MASK;

                            nodes_left -= 1;
                            if nodes_left == 0 {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }

            4 => {
                let bcr = self.channels[4].bcr;
                let blocks = (bcr >> 16) & 0xFFFF;
                let block_size = bcr & 0xFFFF;
                let count = if blocks == 0 { block_size } else { blocks * block_size };

                let mut addr = self.channels[4].madr & RAM_ADDR_MASK;
                println!("  [DMA 4 - SPU] Uploading {} words to Sound RAM...", count);

                for _ in 0..count {
                    let word = ram.read32(addr);
                    spu.write16(0x1A8, word as u16);
                    spu.write16(0x1A8, (word >> 16) as u16);
                    addr = addr.wrapping_add(step) & RAM_ADDR_MASK;
                }
                self.channels[4].madr = addr;
            }

            6 => {
                let mut cur_addr = self.channels[6].madr & RAM_ADDR_MASK;
                let count = if self.channels[6].bcr & 0xFFFF == 0 {
                    0x10000
                } else {
                    self.channels[6].bcr & 0xFFFF
                };

                for _ in 0..(count - 1) {
                    let next_addr = cur_addr.wrapping_sub(4);
                    ram.write32(cur_addr, next_addr & 0x00FF_FFFF);
                    cur_addr = next_addr & RAM_ADDR_MASK;
                }
                ram.write32(cur_addr, 0x00FF_FFFF);
            }

            _ => {}
        }

        self.complete(channel);
    }
}

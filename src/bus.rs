use std::collections::HashMap;

use crate::bios::{Bios, BIOS_SIZE};
use crate::cdrom::CdRom;
use crate::dma::Dma;
use crate::gpu::Gpu;
use crate::ram::Ram;
use crate::spu::Spu;
use crate::timers::Timers;

const RAM_START: u32 = 0x0000_0000;
const RAM_MIRROR_END: u32 = 0x007F_FFFF;

const BIOS_START: u32 = 0x1FC0_0000;
const BIOS_END: u32 = BIOS_START + (BIOS_SIZE as u32) - 1;

const EXP1_START: u32 = 0x1F00_0000;
const EXP1_END: u32 = 0x1F07_FFFF;

const SCRATCH_START: u32 = 0x1F80_0000;
const SCRATCH_END: u32 = 0x1F80_03FF;
const SCRATCH_MASK: usize = 0x3FF;

const CACHE_CONTROL: u32 = 0xFFFE_0130;

const I_STAT: u32 = 0x1F80_1070;
const I_STAT_HI: u32 = 0x1F80_1072;
const I_MASK: u32 = 0x1F80_1074;

const TIMERS_START: u32 = 0x1F80_1100;
const TIMERS_END: u32 = 0x1F80_112F;

const DMA_START: u32 = 0x1F80_1080;
const DMA_END: u32 = 0x1F80_10F7;

const GPU_GP0: u32 = 0x1F80_1810;
const GPU_GP1: u32 = 0x1F80_1814;

const CDROM_START: u32 = 0x1F80_1800;
const CDROM_END: u32 = 0x1F80_1803;

const SPU_START: u32 = 0x1F80_1C00;
const SPU_END: u32 = 0x1F80_1E7F;

pub const IRQ_VBLANK: u32 = 1 << 0;
pub const IRQ_CDROM: u32 = 1 << 2;
pub const IRQ_DMA: u32 = 1 << 3;

pub struct Bus {
    bios: Bios,
    ram: Ram,
    scratch: Box<[u8; 1024]>,
    pub dma: Dma,
    pub gpu: Gpu,
    pub timers: Timers,
    pub cdrom: CdRom,
    pub spu: Spu,
    pub i_stat: u32,
    pub i_mask: u32,

    pub unmapped_reads: HashMap<u32, u32>,
    pub unmapped_writes: HashMap<u32, u32>,
}

impl Bus {
    pub fn new(bios: Bios) -> Self {
        Self {
            bios,
            ram: Ram::new(),
            scratch: Box::new([0u8; 1024]),
            dma: Dma::new(),
            gpu: Gpu::new(),
            timers: Timers::new(),
            cdrom: CdRom::new(),
            spu: Spu::new(),
            i_stat: 0,
            i_mask: 0,
            unmapped_reads: HashMap::new(),
            unmapped_writes: HashMap::new(),
        }
    }

    fn mask_virtual_address(addr: u32) -> u32 {
        addr & 0x1FFF_FFFF
    }

    fn note_write(&mut self, paddr: u32) {
        *self.unmapped_writes.entry(paddr).or_insert(0) += 1;
    }

    pub fn irq_pending(&self) -> bool {
        (self.i_stat & self.i_mask) != 0
    }

    pub fn raise_irq(&mut self, bits: u32) {
        self.i_stat |= bits;
    }

    pub fn tick(&mut self, cycles: u32) {
        let timer_irqs = self.timers.tick(cycles);
        if timer_irqs != 0 {
            self.i_stat |= timer_irqs << 4;
        }

        self.cdrom.tick(cycles);
        if self.cdrom.take_irq() {
            self.i_stat |= IRQ_CDROM;
        }
    }

    pub fn vblank(&mut self) {
        self.i_stat |= IRQ_VBLANK;
        self.gpu.vblank();
    }

    fn scratch_read(&self, offset: u32, bytes: usize) -> u32 {
        let mut v = 0u32;
        for i in 0..bytes {
            v |= (self.scratch[(offset as usize + i) & SCRATCH_MASK] as u32) << (8 * i);
        }
        v
    }

    fn scratch_write(&mut self, offset: u32, bytes: usize, val: u32) {
        for i in 0..bytes {
            self.scratch[(offset as usize + i) & SCRATCH_MASK] = (val >> (8 * i)) as u8;
        }
    }

    fn read_io32(&mut self, paddr: u32) -> u32 {
        match paddr {
            TIMERS_START..=TIMERS_END => self.timers.read32(paddr - TIMERS_START),
            DMA_START..=DMA_END => self.dma.read32(paddr - DMA_START),
            I_STAT => self.i_stat,
            I_MASK => self.i_mask,
            GPU_GP0 => self.gpu.read_gp0(),
            GPU_GP1 => self.gpu.read_gpustat(),
            _ => {
                *self.unmapped_reads.entry(paddr).or_insert(0) += 1;
                0
            }
        }
    }

    pub fn read8(&mut self, addr: u32) -> u8 {
        let paddr = Self::mask_virtual_address(addr);
        match paddr {
            RAM_START..=RAM_MIRROR_END => {
                let offset = paddr & 0x001F_FFFF;
                self.ram.read8(offset)
            }
            BIOS_START..=BIOS_END => {
                let offset = paddr - BIOS_START;
                self.bios.read8(offset)
            }
            SCRATCH_START..=SCRATCH_END => self.scratch_read(paddr - SCRATCH_START, 1) as u8,
            CDROM_START..=CDROM_END => self.cdrom.read(paddr - CDROM_START),
            SPU_START..=SPU_END => {
                let o = paddr - SPU_START;
                (self.spu.read16(o & !1) >> ((o & 1) * 8)) as u8
            }
            EXP1_START..=EXP1_END => 0xFF,
            _ => {
                let w = self.read_io32(paddr & !3);
                (w >> ((paddr & 3) * 8)) as u8
            }
        }
    }

    pub fn read16(&mut self, addr: u32) -> u16 {
        let paddr = Self::mask_virtual_address(addr);
        match paddr {
            RAM_START..=RAM_MIRROR_END => {
                let offset = paddr & 0x001F_FFFF;
                self.ram.read16(offset)
            }
            BIOS_START..=BIOS_END => {
                let offset = paddr - BIOS_START;
                self.bios.read16(offset)
            }
            SCRATCH_START..=SCRATCH_END => self.scratch_read(paddr - SCRATCH_START, 2) as u16,
            SPU_START..=SPU_END => self.spu.read16(paddr - SPU_START),
            EXP1_START..=EXP1_END => 0xFFFF,
            _ => {
                let w = self.read_io32(paddr & !3);
                (w >> ((paddr & 2) * 8)) as u16
            }
        }
    }

    pub fn read32(&mut self, addr: u32) -> u32 {
        let paddr = Self::mask_virtual_address(addr);
        match paddr {
            RAM_START..=RAM_MIRROR_END => {
                let offset = paddr & 0x001F_FFFF;
                self.ram.read32(offset)
            }
            BIOS_START..=BIOS_END => {
                let offset = paddr - BIOS_START;
                self.bios.read32(offset)
            }
            SCRATCH_START..=SCRATCH_END => self.scratch_read(paddr - SCRATCH_START, 4),
            SPU_START..=SPU_END => {
                let o = paddr - SPU_START;
                (self.spu.read16(o) as u32) | ((self.spu.read16(o + 2) as u32) << 16)
            }
            EXP1_START..=EXP1_END => 0xFFFF_FFFF,
            _ => self.read_io32(paddr),
        }
    }

    pub fn write8(&mut self, addr: u32, val: u8) {
        let paddr = Self::mask_virtual_address(addr);
        match paddr {
            RAM_START..=RAM_MIRROR_END => {
                let offset = paddr & 0x001F_FFFF;
                self.ram.write8(offset, val);
            }
            SCRATCH_START..=SCRATCH_END => {
                self.scratch_write(paddr - SCRATCH_START, 1, val as u32);
            }
            CDROM_START..=CDROM_END => {
                self.cdrom.write(paddr - CDROM_START, val);
                if self.cdrom.take_irq() {
                    self.i_stat |= IRQ_CDROM;
                }
            }
            SPU_START..=SPU_END => {}
            _ => self.note_write(paddr),
        }
    }

    pub fn write16(&mut self, addr: u32, val: u16) {
        let paddr = Self::mask_virtual_address(addr);
        match paddr {
            RAM_START..=RAM_MIRROR_END => {
                let offset = paddr & 0x001F_FFFF;
                self.ram.write16(offset, val);
            }
            SCRATCH_START..=SCRATCH_END => {
                self.scratch_write(paddr - SCRATCH_START, 2, val as u32);
            }
            TIMERS_START..=TIMERS_END => {
                let offset = paddr - TIMERS_START;
                self.timers.write16(offset, val);
            }
            SPU_START..=SPU_END => self.spu.write16(paddr - SPU_START, val),
            I_STAT => self.i_stat &= (val as u32) | 0xFFFF_0000,
            I_STAT_HI => self.i_stat &= ((val as u32) << 16) | 0x0000_FFFF,
            I_MASK => self.i_mask = (val as u32) & 0x7FF,
            _ => self.note_write(paddr),
        }
    }

    pub fn write32(&mut self, addr: u32, val: u32) {
        if addr == CACHE_CONTROL {
            return;
        }

        let paddr = Self::mask_virtual_address(addr);
        match paddr {
            RAM_START..=RAM_MIRROR_END => {
                let offset = paddr & 0x001F_FFFF;
                self.ram.write32(offset, val);
            }
            SCRATCH_START..=SCRATCH_END => {
                self.scratch_write(paddr - SCRATCH_START, 4, val);
            }
            TIMERS_START..=TIMERS_END => {
                let offset = paddr - TIMERS_START;
                self.timers.write32(offset, val);
            }
            DMA_START..=DMA_END => {
                let offset = paddr - DMA_START;
                let irq = self.dma.write32(offset, val, &mut self.ram, &mut self.gpu, &mut self.spu);
                if irq {
                    self.i_stat |= IRQ_DMA;
                }
            }
            SPU_START..=SPU_END => {
                let o = paddr - SPU_START;
                if o == 0x1A8 {
                    self.spu.write16(0x1A8, val as u16);
                    self.spu.write16(0x1A8, (val >> 16) as u16);
                } else {
                    self.spu.write16(o, val as u16);
                    self.spu.write16(o + 2, (val >> 16) as u16);
                }
            }
            I_STAT => self.i_stat &= val,
            I_MASK => self.i_mask = val & 0x7FF,
            GPU_GP0 => self.gpu.write_gp0(val),
            GPU_GP1 => self.gpu.write_gp1(val),
            _ => self.note_write(paddr),
        }
    }
}

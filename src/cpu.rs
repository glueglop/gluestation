use crate::bus::Bus;
use crate::gte::Gte;

const COP0_BADVADDR: usize = 8;
const COP0_SR: usize = 12;
const COP0_CAUSE: usize = 13;
const COP0_EPC: usize = 14;

pub struct Cpu {
    regs: [u32; 32],
    cop0_regs: [u32; 32],
    pc: u32,
    next_pc: u32,
    current_pc: u32,

    hi: u32,
    lo: u32,

    load_pending: Option<(usize, u32)>,
    load_next: Option<(usize, u32)>,

    branch_flag: bool,
    in_delay_slot: bool,

    pub irq_count: u32,

    gte: Gte,
}

impl Cpu {
    pub fn new() -> Self {
        let mut cop0_regs = [0u32; 32];
        cop0_regs[COP0_SR] = 0x1090_0000;
        cop0_regs[15] = 0x0000_0002;

        Self {
            regs: [0; 32],
            cop0_regs,
            pc: 0xBFC0_0000,
            next_pc: 0xBFC0_0004,
            current_pc: 0xBFC0_0000,
            hi: 0,
            lo: 0,
            load_pending: None,
            load_next: None,
            branch_flag: false,
            in_delay_slot: false,
            irq_count: 0,
            gte: Gte::new(),
        }
    }

    pub fn get_pc(&self) -> u32 {
        self.current_pc
    }

    pub fn sr(&self) -> u32 {
        self.cop0_regs[COP0_SR]
    }

    pub fn cause(&self) -> u32 {
        self.cop0_regs[COP0_CAUSE]
    }

    pub fn read_reg(&self, index: usize) -> u32 {
        self.regs[index]
    }

    pub fn write_reg(&mut self, index: usize, val: u32) {
        if index != 0 {
            if let Some((r, _)) = self.load_pending {
                if r == index {
                    self.load_pending = None;
                }
            }
            self.regs[index] = val;
        }
    }

    pub fn trigger_exception(&mut self, cause: u32) {
        if self.in_delay_slot {
            self.cop0_regs[COP0_EPC] = self.current_pc.wrapping_sub(4);
            self.cop0_regs[COP0_CAUSE] |= 1 << 31;
        } else {
            self.cop0_regs[COP0_EPC] = self.current_pc;
            self.cop0_regs[COP0_CAUSE] &= !(1 << 31);
        }
        self.cop0_regs[COP0_CAUSE] =
            (self.cop0_regs[COP0_CAUSE] & !0x7C) | ((cause & 0x1F) << 2);

        let mode = self.cop0_regs[COP0_SR] & 0x3F;
        self.cop0_regs[COP0_SR] = (self.cop0_regs[COP0_SR] & !0x3F) | ((mode << 2) & 0x3F);

        let bev = (self.cop0_regs[COP0_SR] & (1 << 22)) != 0;
        let vector = if bev { 0xBFC0_0180 } else { 0x8000_0080 };

        self.pc = vector;
        self.next_pc = vector.wrapping_add(4);
        self.branch_flag = false;
    }

    fn address_error(&mut self, addr: u32, store: bool) {
        self.cop0_regs[COP0_BADVADDR] = addr;
        self.trigger_exception(if store { 5 } else { 4 });
    }

    pub fn step(&mut self, bus: &mut Bus) {
        self.current_pc = self.pc;
        self.in_delay_slot = self.branch_flag;

        if bus.irq_pending() {
            self.cop0_regs[COP0_CAUSE] |= 1 << 10;
        } else {
            self.cop0_regs[COP0_CAUSE] &= !(1 << 10);
        }

        let sr = self.cop0_regs[COP0_SR];
        if (sr & 1) != 0 && (self.cop0_regs[COP0_CAUSE] & sr & 0xFF00) != 0 {
            self.irq_count += 1;
            self.trigger_exception(0);
            return;
        }

        self.pc = self.next_pc;
        self.next_pc = self.next_pc.wrapping_add(4);

        if let Some((reg, val)) = self.load_pending.take() {
            self.write_reg(reg, val);
        }
        self.load_pending = self.load_next.take();

        let instruction = bus.read32(self.current_pc);

        let opcode = instruction >> 26;
        let is_branch = matches!(opcode, 1..=7)
            || (opcode == 0 && matches!(instruction & 0x3F, 8 | 9));

        self.branch_flag = false;
        self.execute(instruction, bus);
        if is_branch {
            self.branch_flag = true;
        }
    }

    fn execute(&mut self, instr: u32, bus: &mut Bus) {
        let opcode = instr >> 26;
        let rs = ((instr >> 21) & 0x1F) as usize;
        let rt = ((instr >> 16) & 0x1F) as usize;
        let rd = ((instr >> 11) & 0x1F) as usize;
        let shamt = (instr >> 6) & 0x1F;
        let funct = instr & 0x3F;

        let imm = instr & 0xFFFF;
        let imm_se = (instr as i16 as i32) as u32;

        match opcode {
            0x00 => match funct {
                0x00 => {
                    let val = self.read_reg(rt) << shamt;
                    self.write_reg(rd, val);
                }
                0x02 => {
                    let val = self.read_reg(rt) >> shamt;
                    self.write_reg(rd, val);
                }
                0x03 => {
                    let val = ((self.read_reg(rt) as i32) >> shamt) as u32;
                    self.write_reg(rd, val);
                }
                0x04 => {
                    let s = self.read_reg(rs) & 0x1F;
                    let val = self.read_reg(rt) << s;
                    self.write_reg(rd, val);
                }
                0x06 => {
                    let s = self.read_reg(rs) & 0x1F;
                    let val = self.read_reg(rt) >> s;
                    self.write_reg(rd, val);
                }
                0x07 => {
                    let s = self.read_reg(rs) & 0x1F;
                    let val = ((self.read_reg(rt) as i32) >> s) as u32;
                    self.write_reg(rd, val);
                }
                0x08 => {
                    self.next_pc = self.read_reg(rs);
                }
                0x09 => {
                    let return_addr = self.next_pc;
                    self.next_pc = self.read_reg(rs);
                    self.write_reg(rd, return_addr);
                }
                0x0C => {
                    self.trigger_exception(8);
                }
                0x0D => {
                    self.trigger_exception(9);
                }
                0x10 => {
                    self.write_reg(rd, self.hi);
                }
                0x11 => {
                    self.hi = self.read_reg(rs);
                }
                0x12 => {
                    self.write_reg(rd, self.lo);
                }
                0x13 => {
                    self.lo = self.read_reg(rs);
                }
                0x18 => {
                    let a = (self.read_reg(rs) as i32) as i64;
                    let b = (self.read_reg(rt) as i32) as i64;
                    let res = (a * b) as u64;
                    self.lo = res as u32;
                    self.hi = (res >> 32) as u32;
                }
                0x19 => {
                    let a = self.read_reg(rs) as u64;
                    let b = self.read_reg(rt) as u64;
                    let res = a * b;
                    self.lo = res as u32;
                    self.hi = (res >> 32) as u32;
                }
                0x1A => {
                    let num = self.read_reg(rs) as i32;
                    let den = self.read_reg(rt) as i32;
                    if den == 0 {
                        self.hi = num as u32;
                        self.lo = if num >= 0 { 0xFFFF_FFFF } else { 1 };
                    } else if (num as u32) == 0x8000_0000 && den == -1 {
                        self.hi = 0;
                        self.lo = 0x8000_0000;
                    } else {
                        self.lo = (num / den) as u32;
                        self.hi = (num % den) as u32;
                    }
                }
                0x1B => {
                    let num = self.read_reg(rs);
                    let den = self.read_reg(rt);
                    if den == 0 {
                        self.hi = num;
                        self.lo = 0xFFFF_FFFF;
                    } else {
                        self.lo = num / den;
                        self.hi = num % den;
                    }
                }
                0x20 => {
                    match (self.read_reg(rs) as i32).checked_add(self.read_reg(rt) as i32) {
                        Some(v) => self.write_reg(rd, v as u32),
                        None => self.trigger_exception(12),
                    }
                }
                0x21 => {
                    let val = self.read_reg(rs).wrapping_add(self.read_reg(rt));
                    self.write_reg(rd, val);
                }
                0x22 => {
                    match (self.read_reg(rs) as i32).checked_sub(self.read_reg(rt) as i32) {
                        Some(v) => self.write_reg(rd, v as u32),
                        None => self.trigger_exception(12),
                    }
                }
                0x23 => {
                    let val = self.read_reg(rs).wrapping_sub(self.read_reg(rt));
                    self.write_reg(rd, val);
                }
                0x24 => {
                    let val = self.read_reg(rs) & self.read_reg(rt);
                    self.write_reg(rd, val);
                }
                0x25 => {
                    let val = self.read_reg(rs) | self.read_reg(rt);
                    self.write_reg(rd, val);
                }
                0x26 => {
                    let val = self.read_reg(rs) ^ self.read_reg(rt);
                    self.write_reg(rd, val);
                }
                0x27 => {
                    let val = !(self.read_reg(rs) | self.read_reg(rt));
                    self.write_reg(rd, val);
                }
                0x2A => {
                    let cond = (self.read_reg(rs) as i32) < (self.read_reg(rt) as i32);
                    self.write_reg(rd, cond as u32);
                }
                0x2B => {
                    let cond = self.read_reg(rs) < self.read_reg(rt);
                    self.write_reg(rd, cond as u32);
                }
                _ => panic!(
                    "Unhandled SPECIAL funct: 0x{:02X} (instruction: 0x{:08X})",
                    funct, instr
                ),
            },

            0x01 => {
                let target = self.pc.wrapping_add(imm_se << 2);
                let rs_val = self.read_reg(rs) as i32;
                let is_bgez = (rt & 1) != 0;
                let link = (rt & 0x1E) == 0x10;
                let cond = if is_bgez { rs_val >= 0 } else { rs_val < 0 };
                if link {
                    self.write_reg(31, self.next_pc);
                }
                if cond {
                    self.next_pc = target;
                }
            }

            0x02 => {
                let target = instr & 0x03FF_FFFF;
                let jump_addr = (self.pc & 0xF000_0000) | (target << 2);
                self.next_pc = jump_addr;
            }

            0x03 => {
                let target = instr & 0x03FF_FFFF;
                let jump_addr = (self.pc & 0xF000_0000) | (target << 2);
                self.write_reg(31, self.next_pc);
                self.next_pc = jump_addr;
            }

            0x04 => {
                let target = self.pc.wrapping_add(imm_se << 2);
                if self.read_reg(rs) == self.read_reg(rt) {
                    self.next_pc = target;
                }
            }

            0x05 => {
                let target = self.pc.wrapping_add(imm_se << 2);
                if self.read_reg(rs) != self.read_reg(rt) {
                    self.next_pc = target;
                }
            }

            0x06 => {
                let target = self.pc.wrapping_add(imm_se << 2);
                if (self.read_reg(rs) as i32) <= 0 {
                    self.next_pc = target;
                }
            }

            0x07 => {
                let target = self.pc.wrapping_add(imm_se << 2);
                if (self.read_reg(rs) as i32) > 0 {
                    self.next_pc = target;
                }
            }

            0x08 => {
                match (self.read_reg(rs) as i32).checked_add(imm_se as i32) {
                    Some(v) => self.write_reg(rt, v as u32),
                    None => self.trigger_exception(12),
                }
            }
            0x09 => {
                let val = self.read_reg(rs).wrapping_add(imm_se);
                self.write_reg(rt, val);
            }

            0x0A => {
                let cond = (self.read_reg(rs) as i32) < (imm_se as i32);
                self.write_reg(rt, cond as u32);
            }

            0x0B => {
                let cond = self.read_reg(rs) < imm_se;
                self.write_reg(rt, cond as u32);
            }

            0x0C => {
                let val = self.read_reg(rs) & imm;
                self.write_reg(rt, val);
            }

            0x0D => {
                let val = self.read_reg(rs) | imm;
                self.write_reg(rt, val);
            }

            0x0E => {
                let val = self.read_reg(rs) ^ imm;
                self.write_reg(rt, val);
            }

            0x0F => {
                let val = imm << 16;
                self.write_reg(rt, val);
            }

            0x10 => {
                let cop_op = (instr >> 21) & 0x1F;
                match cop_op {
                    0x00 => {
                        let val = self.cop0_regs[rd];
                        self.load_next = Some((rt, val));
                    }
                    0x04 => {
                        let val = self.read_reg(rt);
                        match rd {
                            13 => {
                                self.cop0_regs[13] = (self.cop0_regs[13] & !0x300) | (val & 0x300);
                            }
                            8 | 15 => {}
                            _ => self.cop0_regs[rd] = val,
                        }
                    }
                    0x10 => {
                        if funct == 0x10 {
                            let mode = self.cop0_regs[COP0_SR] & 0x3F;
                            self.cop0_regs[COP0_SR] =
                                (self.cop0_regs[COP0_SR] & !0x0F) | (mode >> 2);
                        } else {
                            panic!("Unhandled COP0 instruction: funct 0x{:02X}", funct);
                        }
                    }
                    _ => panic!(
                        "Unhandled COP0 sub-op: 0x{:02X} (instruction: 0x{:08X})",
                        cop_op, instr
                    ),
                }
            }

            0x20 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let val = (bus.read8(addr) as i8 as i32) as u32;
                self.load_next = Some((rt, val));
            }

            0x21 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                if addr & 1 != 0 {
                    return self.address_error(addr, false);
                }
                let val = (bus.read16(addr) as i16 as i32) as u32;
                self.load_next = Some((rt, val));
            }

            0x22 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let aligned_addr = addr & !3;
                let mem = bus.read32(aligned_addr);

                let cur = match self.load_pending {
                    Some((reg, val)) if reg == rt => val,
                    _ => self.read_reg(rt),
                };

                let val = match addr & 3 {
                    0 => (cur & 0x00FF_FFFF) | (mem << 24),
                    1 => (cur & 0x0000_FFFF) | (mem << 16),
                    2 => (cur & 0x0000_00FF) | (mem << 8),
                    3 => mem,
                    _ => unreachable!(),
                };

                self.load_next = Some((rt, val));
            }

            0x23 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                if addr & 3 != 0 {
                    return self.address_error(addr, false);
                }
                let val = bus.read32(addr);
                self.load_next = Some((rt, val));
            }

            0x24 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let val = bus.read8(addr) as u32;
                self.load_next = Some((rt, val));
            }

            0x25 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                if addr & 1 != 0 {
                    return self.address_error(addr, false);
                }
                let val = bus.read16(addr) as u32;
                self.load_next = Some((rt, val));
            }

            0x26 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let aligned_addr = addr & !3;
                let mem = bus.read32(aligned_addr);

                let cur = match self.load_pending {
                    Some((reg, val)) if reg == rt => val,
                    _ => self.read_reg(rt),
                };

                let val = match addr & 3 {
                    0 => mem,
                    1 => (cur & 0xFF00_0000) | (mem >> 8),
                    2 => (cur & 0xFFFF_0000) | (mem >> 16),
                    3 => (cur & 0xFFFF_FF00) | (mem >> 24),
                    _ => unreachable!(),
                };

                self.load_next = Some((rt, val));
            }

            0x28 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let val = self.read_reg(rt) as u8;
                let is_cache_isolated = (self.cop0_regs[COP0_SR] & 0x10000) != 0;
                if !is_cache_isolated {
                    bus.write8(addr, val);
                }
            }

            0x29 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                if addr & 1 != 0 {
                    return self.address_error(addr, true);
                }
                let val = self.read_reg(rt) as u16;
                let is_cache_isolated = (self.cop0_regs[COP0_SR] & 0x10000) != 0;
                if !is_cache_isolated {
                    bus.write16(addr, val);
                }
            }

            0x2A => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let aligned_addr = addr & !3;
                let cur = bus.read32(aligned_addr);
                let reg_val = self.read_reg(rt);

                let val = match addr & 3 {
                    0 => (cur & 0xFFFF_FF00) | (reg_val >> 24),
                    1 => (cur & 0xFFFF_0000) | (reg_val >> 16),
                    2 => (cur & 0xFF00_0000) | (reg_val >> 8),
                    3 => reg_val,
                    _ => unreachable!(),
                };

                let is_cache_isolated = (self.cop0_regs[COP0_SR] & 0x10000) != 0;
                if !is_cache_isolated {
                    bus.write32(aligned_addr, val);
                }
            }

            0x2B => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let val = self.read_reg(rt);

                let is_cache_isolated = (self.cop0_regs[COP0_SR] & 0x10000) != 0;
                if is_cache_isolated {
                    return;
                }

                if addr & 3 != 0 {
                    return self.address_error(addr, true);
                }
                bus.write32(addr, val);
            }

            0x2E => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                let aligned_addr = addr & !3;
                let cur = bus.read32(aligned_addr);
                let reg_val = self.read_reg(rt);

                let val = match addr & 3 {
                    0 => reg_val,
                    1 => (cur & 0x0000_00FF) | (reg_val << 8),
                    2 => (cur & 0x0000_FFFF) | (reg_val << 16),
                    3 => (cur & 0x00FF_FFFF) | (reg_val << 24),
                    _ => unreachable!(),
                };

                let is_cache_isolated = (self.cop0_regs[COP0_SR] & 0x10000) != 0;
                if !is_cache_isolated {
                    bus.write32(aligned_addr, val);
                }
            }

            0x12 => {
                if instr & (1 << 25) != 0 {
                    self.gte.execute(instr);
                } else {
                    match rs {
                        0x00 => {
                            let val = self.gte.read_data(rd);
                            self.load_next = Some((rt, val));
                        }
                        0x02 => {
                            let val = self.gte.read_ctrl(rd);
                            self.load_next = Some((rt, val));
                        }
                        0x04 => {
                            let val = self.read_reg(rt);
                            self.gte.write_data(rd, val);
                        }
                        0x06 => {
                            let val = self.read_reg(rt);
                            self.gte.write_ctrl(rd, val);
                        }
                        _ => panic!(
                            "Unhandled COP2 sub-op: 0x{:02X} (instruction: 0x{:08X})",
                            rs, instr
                        ),
                    }
                }
            }

            0x32 => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                if addr & 3 != 0 {
                    return self.address_error(addr, false);
                }
                let val = bus.read32(addr);
                self.gte.write_data(rt, val);
            }

            0x3A => {
                let addr = self.read_reg(rs).wrapping_add(imm_se);
                if addr & 3 != 0 {
                    return self.address_error(addr, true);
                }
                let val = self.gte.read_data(rt);
                bus.write32(addr, val);
            }

            _ => panic!(
                "Unhandled opcode: 0x{:02X} (instruction: 0x{:08X})",
                opcode, instr
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bios::{Bios, BIOS_SIZE};

    #[test]
    fn vblank_interrupt_reaches_handler() {
        let main_prog: [u32; 8] = [0x3C081F80, 0x34090001, 0xAD091074, 0x3C0A0040, 0x354A0401, 0x408A6000, 0x0BF00006, 0x00000000];
        let handler: [u32; 6] = [0x3C0BA000, 0x340CBEEF, 0xAD6C0100, 0xAD001070, 0x0BF00064, 0x00000000];

        let mut image = vec![0u8; BIOS_SIZE];
        for (i, w) in main_prog.iter().enumerate() {
            image[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        for (i, w) in handler.iter().enumerate() {
            image[0x180 + i * 4..0x180 + i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let path = std::env::temp_dir().join("ps1_emu_test_bios.bin");
        std::fs::write(&path, &image).unwrap();

        let bios = Bios::new(&path).unwrap();
        let mut bus = Bus::new(bios);
        let mut cpu = Cpu::new();

        for _ in 0..200 {
            cpu.step(&mut bus);
        }
        assert_eq!(bus.read32(0xA000_0100), 0, "handler ran without an interrupt");

        bus.vblank();
        for _ in 0..50 {
            cpu.step(&mut bus);
        }
        assert_eq!(bus.read32(0xA000_0100), 0xBEEF, "handler did not run after VBLANK");
        assert!(!bus.irq_pending(), "I_STAT should have been acknowledged");
    }
}

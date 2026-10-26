// PS1 GTE (COP2): Geometry Transformation Engine.
//
// Data registers   (cop2r0..31):  `data[0..32]`
// Control registers (cop2r32..63): `ctrl[0..32]`
//
// Command word (bit 25 set): bits 0-5 command, 10 lm, 13-14 translation vector,
// 15-16 multiply vector, 17-18 multiply matrix, 19 sf.
//
// Known simplifications: the MVMVA "far colour translation" hardware bug is not
// reproduced, and cycle timing is ignored (commands complete instantly).

const MAX44: i64 = 0x7FF_FFFF_FFFF;
const MIN44: i64 = -0x800_0000_0000;

pub struct Gte {
    data: [u32; 32],
    ctrl: [u32; 32],
}

fn lo16(v: u32) -> i32 {
    v as u16 as i16 as i32
}

fn hi16(v: u32) -> i32 {
    (v >> 16) as u16 as i16 as i32
}

impl Gte {
    pub fn new() -> Self {
        Self {
            data: [0; 32],
            ctrl: [0; 32],
        }
    }

    // ------------------------------------------------------------------
    // Register access (MFC2 / MTC2 / CFC2 / CTC2 / LWC2 / SWC2)
    // ------------------------------------------------------------------

    pub fn read_data(&self, r: usize) -> u32 {
        match r {
            15 => self.data[14], // SXYP reads back SXY2
            28 | 29 => {
                // IRGB / ORGB: IR1..3 compressed to 5 bits each
                let c = |v: u32| -> u32 { ((v as i32) >> 7).clamp(0, 0x1F) as u32 };
                c(self.data[9]) | (c(self.data[10]) << 5) | (c(self.data[11]) << 10)
            }
            31 => {
                // LZCR: leading zeros (or ones, if negative) of LZCS
                let v = self.data[30];
                if (v as i32) < 0 {
                    (!v).leading_zeros()
                } else {
                    v.leading_zeros()
                }
            }
            _ => self.data[r & 31],
        }
    }

    pub fn write_data(&mut self, r: usize, v: u32) {
        match r {
            1 | 3 | 5 | 8 | 9 | 10 | 11 => self.data[r] = (v as u16 as i16 as i32) as u32,
            7 | 16 | 17 | 18 | 19 => self.data[r] = v & 0xFFFF,
            15 => {
                // SXYP write pushes the screen XY FIFO
                self.data[12] = self.data[13];
                self.data[13] = self.data[14];
                self.data[14] = v;
            }
            28 => {
                self.data[9] = (v & 0x1F) * 0x80;
                self.data[10] = ((v >> 5) & 0x1F) * 0x80;
                self.data[11] = ((v >> 10) & 0x1F) * 0x80;
            }
            29 | 31 => {} // read-only
            _ => self.data[r & 31] = v,
        }
    }

    pub fn read_ctrl(&self, r: usize) -> u32 {
        match r {
            31 => {
                let f = self.ctrl[31] & 0x7FFF_F000;
                if f & 0x7F87_E000 != 0 {
                    f | 0x8000_0000
                } else {
                    f
                }
            }
            // 16-bit registers read back sign-extended
            4 | 12 | 20 | 26 | 27 | 29 | 30 => (self.ctrl[r] as u16 as i16 as i32) as u32,
            _ => self.ctrl[r & 31],
        }
    }

    pub fn write_ctrl(&mut self, r: usize, v: u32) {
        match r {
            31 => self.ctrl[31] = v & 0x7FFF_F000,
            4 | 12 | 20 | 26 | 27 | 29 | 30 => self.ctrl[r] = (v as u16 as i16 as i32) as u32,
            _ => self.ctrl[r & 31] = v,
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn flag(&mut self, bit: u32) {
        self.ctrl[31] |= 1 << bit;
    }

    /// 44-bit overflow check for MAC1..3 (i = 1..3). Returns the value wrapped to 44 bits.
    fn check_mac(&mut self, i: usize, v: i64) -> i64 {
        if v > MAX44 {
            self.flag(31 - i as u32);
        } else if v < MIN44 {
            self.flag(28 - i as u32);
        }
        (v << 20) >> 20
    }

    /// 32-bit overflow check for MAC0.
    fn check_mac0(&mut self, v: i64) -> i32 {
        if v > i32::MAX as i64 {
            self.flag(16);
        } else if v < i32::MIN as i64 {
            self.flag(15);
        }
        v as i32
    }

    /// Clamp to the IR range (setting the flag) without storing.
    fn clamp_ir(&mut self, i: usize, v: i32, lm: bool) -> i32 {
        let min = if lm { 0 } else { -0x8000 };
        let r = v.clamp(min, 0x7FFF);
        if r != v {
            self.flag(25 - i as u32); // IR1 = bit 24, IR2 = bit 23, IR3 = bit 22
        }
        r
    }

    fn sat_ir(&mut self, i: usize, v: i32, lm: bool) {
        let r = self.clamp_ir(i, v, lm);
        self.data[8 + i] = r as u32;
    }

    fn sat_ir0(&mut self, v: i32) {
        let r = v.clamp(0, 0x1000);
        if r != v {
            self.flag(12);
        }
        self.data[8] = r as u32;
    }

    /// Set IR1..3 from MAC1..3.
    fn ir_from_mac(&mut self, lm: bool) {
        for i in 1..=3 {
            let mac = self.data[24 + i] as i32;
            self.sat_ir(i, mac, lm);
        }
    }

    fn set_mac_shifted(&mut self, i: usize, v: i64, sf: u32) {
        self.data[24 + i] = ((v >> (sf * 12)) as i32) as u32;
    }

    fn vector(&self, n: usize) -> [i32; 3] {
        [
            lo16(self.data[2 * n]),
            hi16(self.data[2 * n]),
            lo16(self.data[2 * n + 1]),
        ]
    }

    fn ir(&self) -> [i32; 3] {
        [
            self.data[9] as i32,
            self.data[10] as i32,
            self.data[11] as i32,
        ]
    }

    fn rgb(&self) -> [i64; 3] {
        let c = self.data[6];
        [
            (c & 0xFF) as i64,
            ((c >> 8) & 0xFF) as i64,
            ((c >> 16) & 0xFF) as i64,
        ]
    }

    /// 0 = rotation, 1 = light, 2 = light colour, 3 = (buggy) garbage matrix
    fn matrix(&self, which: usize) -> [[i32; 3]; 3] {
        if which == 3 {
            let r = (self.data[6] & 0xFF) as i32;
            let rt13 = lo16(self.ctrl[1]);
            let rt22 = lo16(self.ctrl[2]);
            let ir0 = self.data[8] as i32;
            return [
                [-(r << 4), r << 4, ir0],
                [rt13, rt13, rt13],
                [rt22, rt22, rt22],
            ];
        }
        let b = match which {
            0 => 0,
            1 => 8,
            _ => 16,
        };
        let c = &self.ctrl;
        [
            [lo16(c[b]), hi16(c[b]), lo16(c[b + 1])],
            [hi16(c[b + 1]), lo16(c[b + 2]), hi16(c[b + 2])],
            [lo16(c[b + 3]), hi16(c[b + 3]), lo16(c[b + 4])],
        ]
    }

    /// 0 = TR, 1 = BK, 2 = FC, anything else = zero vector
    fn translation(&self, which: usize) -> [i64; 3] {
        let b = match which {
            0 => 5,
            1 => 13,
            2 => 21,
            _ => return [0, 0, 0],
        };
        [
            self.ctrl[b] as i32 as i64,
            self.ctrl[b + 1] as i32 as i64,
            self.ctrl[b + 2] as i32 as i64,
        ]
    }

    fn push_sxy(&mut self, sx: i32, sy: i32) {
        let sxc = sx.clamp(-0x400, 0x3FF);
        if sxc != sx {
            self.flag(14);
        }
        let syc = sy.clamp(-0x400, 0x3FF);
        if syc != sy {
            self.flag(13);
        }
        self.data[12] = self.data[13];
        self.data[13] = self.data[14];
        self.data[14] = (sxc as u16 as u32) | ((syc as u16 as u32) << 16);
    }

    fn push_sz(&mut self, v: i64) {
        let z = v.clamp(0, 0xFFFF);
        if z != v {
            self.flag(18);
        }
        self.data[16] = self.data[17];
        self.data[17] = self.data[18];
        self.data[18] = self.data[19];
        self.data[19] = z as u32;
    }

    /// Push MAC1..3 >> 4 (saturated to 0..255) onto the colour FIFO.
    fn push_color(&mut self) {
        let mut c = [0u32; 3];
        for i in 0..3 {
            let v = (self.data[25 + i] as i32) >> 4;
            let r = v.clamp(0, 255);
            if r != v {
                self.flag(21 - i as u32); // R = bit 21, G = 20, B = 19
            }
            c[i] = r as u32;
        }
        let code = self.data[6] >> 24;
        self.data[20] = self.data[21];
        self.data[21] = self.data[22];
        self.data[22] = c[0] | (c[1] << 8) | (c[2] << 16) | (code << 24);
    }

    /// GTE reciprocal-based division (Unsigned Newton-Raphson), result max 0x1FFFF.
    fn divide(&mut self, h: u32, sz3: u32) -> u32 {
        if h < sz3 * 2 {
            let z = (sz3 as u16).leading_zeros();
            let n = (h as u64) << z;
            let d = (sz3 as u64) << z;
            let idx = ((d - 0x7FC0) >> 7) as u32;
            let t = ((0x40000 / (idx + 0x100)) + 1) / 2;
            let u = t.max(0x101) as u64;
            let d2 = (0x200_0080u64 - d * u) >> 8;
            let d3 = (0x80 + d2 * u) >> 8;
            ((n * d3 + 0x8000) >> 16).min(0x1FFFF) as u32
        } else {
            self.flag(17);
            0x1FFFF
        }
    }

    // ------------------------------------------------------------------
    // Building blocks
    // ------------------------------------------------------------------

    /// MAC = (T<<12 + M*V) >> (sf*12); IR = saturate(MAC)
    fn mat_mul_add(&mut self, m: [[i32; 3]; 3], v: [i32; 3], t: [i64; 3], sf: u32, lm: bool) {
        for i in 0..3 {
            let mut acc = t[i] << 12;
            acc = self.check_mac(i + 1, acc + (m[i][0] as i64) * (v[0] as i64));
            acc = self.check_mac(i + 1, acc + (m[i][1] as i64) * (v[1] as i64));
            acc = self.check_mac(i + 1, acc + (m[i][2] as i64) * (v[2] as i64));
            self.set_mac_shifted(i + 1, acc, sf);
        }
        self.ir_from_mac(lm);
    }

    /// Depth-cue interpolation towards the far colour.
    /// `base` is in 1.19.12 scale. Result goes to MAC and IR.
    fn interpolate(&mut self, base: [i64; 3], sf: u32, lm: bool) {
        let fc = self.translation(2);
        let ir0 = self.data[8] as i32 as i64;
        for i in 0..3 {
            let diff = self.check_mac(i + 1, (fc[i] << 12) - base[i]);
            let tmp = self.clamp_ir(i + 1, (diff >> (sf * 12)) as i32, false) as i64;
            let sum = self.check_mac(i + 1, tmp * ir0 + base[i]);
            self.set_mac_shifted(i + 1, sum, sf);
        }
        self.ir_from_mac(lm);
    }

    /// Final stage of NCCx / CC: colour * IR.
    fn ncc_final(&mut self, sf: u32, lm: bool) {
        let rgb = self.rgb();
        let ir = self.ir();
        for i in 0..3 {
            let v = self.check_mac(i + 1, (rgb[i] * ir[i] as i64) << 4);
            self.set_mac_shifted(i + 1, v, sf);
        }
        self.ir_from_mac(lm);
        self.push_color();
    }

    /// Final stage of NCDx / CDP / DCPL: colour * IR, then depth-cue.
    fn ncd_final(&mut self, sf: u32, lm: bool) {
        let rgb = self.rgb();
        let ir = self.ir();
        let base = [
            (rgb[0] * ir[0] as i64) << 4,
            (rgb[1] * ir[1] as i64) << 4,
            (rgb[2] * ir[2] as i64) << 4,
        ];
        self.interpolate(base, sf, lm);
        self.push_color();
    }

    /// Light-colour stage: IR = (BK<<12 + LCM * IR) >> sf*12, then finish by `kind`.
    /// kind: 0 = push colour (NCS), 1 = NCC, 2 = NCD
    fn nc_color(&mut self, sf: u32, lm: bool, kind: u8) {
        let lcm = self.matrix(2);
        let bk = self.translation(1);
        let ir = self.ir();
        self.mat_mul_add(lcm, ir, bk, sf, lm);
        match kind {
            0 => self.push_color(),
            1 => self.ncc_final(sf, lm),
            _ => self.ncd_final(sf, lm),
        }
    }

    /// Normal-colour pipeline for one vertex.
    fn nc(&mut self, v: [i32; 3], sf: u32, lm: bool, kind: u8) {
        let llm = self.matrix(1);
        self.mat_mul_add(llm, v, [0, 0, 0], sf, lm);
        self.nc_color(sf, lm, kind);
    }

    /// Perspective transform of one vertex (RTPS step).
    fn rtp(&mut self, v: [i32; 3], sf: u32, lm: bool) {
        let m = self.matrix(0);
        let tr = self.translation(0);
        let mut sums = [0i64; 3];

        for i in 0..3 {
            let mut acc = tr[i] << 12;
            acc = self.check_mac(i + 1, acc + (m[i][0] as i64) * (v[0] as i64));
            acc = self.check_mac(i + 1, acc + (m[i][1] as i64) * (v[1] as i64));
            acc = self.check_mac(i + 1, acc + (m[i][2] as i64) * (v[2] as i64));
            sums[i] = acc;
            self.set_mac_shifted(i + 1, acc, sf);
        }

        let mac1 = self.data[25] as i32;
        let mac2 = self.data[26] as i32;
        let mac3 = self.data[27] as i32;
        self.sat_ir(1, mac1, lm);
        self.sat_ir(2, mac2, lm);

        // IR3 saturation flag is computed from MAC3 >> 12 regardless of sf (hardware quirk)
        let z = (sums[2] >> 12) as i32;
        if z < -0x8000 || z > 0x7FFF {
            self.flag(22);
        }
        let ir3_min = if lm { 0 } else { -0x8000 };
        self.data[11] = mac3.clamp(ir3_min, 0x7FFF) as u32;

        self.push_sz(sums[2] >> 12);

        let h = self.ctrl[26] & 0xFFFF;
        let sz3 = self.data[19] & 0xFFFF;
        let q = self.divide(h, sz3) as i64;

        let ofx = self.ctrl[24] as i32 as i64;
        let ofy = self.ctrl[25] as i32 as i64;
        let ir1 = self.data[9] as i32 as i64;
        let ir2 = self.data[10] as i32 as i64;

        let sx = self.check_mac0(q * ir1 + ofx) >> 16;
        let sy = self.check_mac0(q * ir2 + ofy) >> 16;
        self.push_sxy(sx, sy);

        let dqa = self.ctrl[27] as i32 as i64;
        let dqb = self.ctrl[28] as i32 as i64;
        let m0 = self.check_mac0(q * dqa + dqb);
        self.data[24] = m0 as u32;
        self.sat_ir0(m0 >> 12);
    }

    // ------------------------------------------------------------------
    // Command dispatch
    // ------------------------------------------------------------------

    pub fn execute(&mut self, instr: u32) {
        self.ctrl[31] = 0; // every command starts with a clean FLAG register

        let sf = (instr >> 19) & 1;
        let lm = (instr >> 10) & 1 != 0;

        match instr & 0x3F {
            0x01 => {
                let v = self.vector(0);
                self.rtp(v, sf, lm);
            }
            0x06 => self.nclip(),
            0x0C => self.op(sf, lm),
            0x10 => {
                let rgb = self.rgb();
                let base = [rgb[0] << 16, rgb[1] << 16, rgb[2] << 16];
                self.interpolate(base, sf, lm);
                self.push_color();
            }
            0x11 => {
                let ir = self.ir();
                let base = [
                    (ir[0] as i64) << 12,
                    (ir[1] as i64) << 12,
                    (ir[2] as i64) << 12,
                ];
                self.interpolate(base, sf, lm);
                self.push_color();
            }
            0x12 => self.mvmva(instr, sf, lm),
            0x13 => {
                let v = self.vector(0);
                self.nc(v, sf, lm, 2);
            }
            0x14 => self.nc_color(sf, lm, 2),
            0x16 => {
                for n in 0..3 {
                    let v = self.vector(n);
                    self.nc(v, sf, lm, 2);
                }
            }
            0x1B => {
                let v = self.vector(0);
                self.nc(v, sf, lm, 1);
            }
            0x1C => self.nc_color(sf, lm, 1),
            0x1E => {
                let v = self.vector(0);
                self.nc(v, sf, lm, 0);
            }
            0x20 => {
                for n in 0..3 {
                    let v = self.vector(n);
                    self.nc(v, sf, lm, 0);
                }
            }
            0x28 => self.sqr(sf, lm),
            0x29 => self.ncd_final(sf, lm), // DCPL
            0x2A => {
                // DPCT: three passes using RGB0, pushing each result
                for _ in 0..3 {
                    let c = self.data[20];
                    let base = [
                        ((c & 0xFF) as i64) << 16,
                        (((c >> 8) & 0xFF) as i64) << 16,
                        (((c >> 16) & 0xFF) as i64) << 16,
                    ];
                    self.interpolate(base, sf, lm);
                    self.push_color();
                }
            }
            0x2D => {
                let zsf3 = self.ctrl[29] as i32 as i64;
                let sum = (self.data[17] & 0xFFFF) as i64
                    + (self.data[18] & 0xFFFF) as i64
                    + (self.data[19] & 0xFFFF) as i64;
                self.avsz(zsf3 * sum);
            }
            0x2E => {
                let zsf4 = self.ctrl[30] as i32 as i64;
                let sum = (self.data[16] & 0xFFFF) as i64
                    + (self.data[17] & 0xFFFF) as i64
                    + (self.data[18] & 0xFFFF) as i64
                    + (self.data[19] & 0xFFFF) as i64;
                self.avsz(zsf4 * sum);
            }
            0x30 => {
                for n in 0..3 {
                    let v = self.vector(n);
                    self.rtp(v, sf, lm);
                }
            }
            0x3D => self.gpf(sf, lm, false),
            0x3E => self.gpf(sf, lm, true),
            0x3F => {
                for n in 0..3 {
                    let v = self.vector(n);
                    self.nc(v, sf, lm, 1);
                }
            }
            _ => {} // unknown command: ignored
        }
    }

    fn nclip(&mut self) {
        let (sx0, sy0) = (lo16(self.data[12]) as i64, hi16(self.data[12]) as i64);
        let (sx1, sy1) = (lo16(self.data[13]) as i64, hi16(self.data[13]) as i64);
        let (sx2, sy2) = (lo16(self.data[14]) as i64, hi16(self.data[14]) as i64);
        let v = sx0 * sy1 + sx1 * sy2 + sx2 * sy0 - sx0 * sy2 - sx1 * sy0 - sx2 * sy1;
        let m0 = self.check_mac0(v);
        self.data[24] = m0 as u32;
    }

    fn avsz(&mut self, v: i64) {
        let m0 = self.check_mac0(v);
        self.data[24] = m0 as u32;
        let otz = (m0 >> 12).clamp(0, 0xFFFF);
        if otz != (m0 >> 12) {
            self.flag(18);
        }
        self.data[7] = otz as u32;
    }

    fn op(&mut self, sf: u32, lm: bool) {
        let m = self.matrix(0);
        let (d1, d2, d3) = (m[0][0] as i64, m[1][1] as i64, m[2][2] as i64);
        let ir = self.ir();
        let (i1, i2, i3) = (ir[0] as i64, ir[1] as i64, ir[2] as i64);

        let a = self.check_mac(1, i3 * d2 - i2 * d3);
        let b = self.check_mac(2, i1 * d3 - i3 * d1);
        let c = self.check_mac(3, i2 * d1 - i1 * d2);
        self.set_mac_shifted(1, a, sf);
        self.set_mac_shifted(2, b, sf);
        self.set_mac_shifted(3, c, sf);
        self.ir_from_mac(lm);
    }

    fn sqr(&mut self, sf: u32, lm: bool) {
        let ir = self.ir();
        for i in 0..3 {
            let v = self.check_mac(i + 1, (ir[i] as i64) * (ir[i] as i64));
            self.set_mac_shifted(i + 1, v, sf);
        }
        self.ir_from_mac(lm);
    }

    /// GPF (accumulate = false) and GPL (accumulate = true).
    fn gpf(&mut self, sf: u32, lm: bool, accumulate: bool) {
        let ir = self.ir();
        let ir0 = self.data[8] as i32 as i64;
        for i in 0..3 {
            let base = if accumulate {
                ((self.data[25 + i] as i32) as i64) << (sf * 12)
            } else {
                0
            };
            let v = self.check_mac(i + 1, base + (ir[i] as i64) * ir0);
            self.set_mac_shifted(i + 1, v, sf);
        }
        self.ir_from_mac(lm);
        self.push_color();
    }

    fn mvmva(&mut self, instr: u32, sf: u32, lm: bool) {
        let mx = ((instr >> 17) & 3) as usize;
        let mv = ((instr >> 15) & 3) as usize;
        let tv = ((instr >> 13) & 3) as usize;

        let m = self.matrix(mx);
        let v = if mv == 3 { self.ir() } else { self.vector(mv) };
        let t = self.translation(tv);
        self.mat_mul_add(m, v, t, sf, lm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtps_identity_transform() {
        let mut g = Gte::new();
        g.write_ctrl(0, 0x1000); // RT11 = 1.0
        g.write_ctrl(2, 0x1000); // RT22 = 1.0
        g.write_ctrl(4, 0x1000); // RT33 = 1.0
        g.write_ctrl(26, 500); // H
        g.write_data(0, 100 | (200 << 16)); // VX0, VY0
        g.write_data(1, 1000); // VZ0

        g.execute(0x0208_0001); // RTPS, sf = 1

        assert_eq!(g.read_data(25), 100);
        assert_eq!(g.read_data(26), 200);
        assert_eq!(g.read_data(27), 1000);
        assert_eq!(g.read_data(19), 1000); // SZ3

        let sxy = g.read_data(14);
        let (sx, sy) = (lo16(sxy), hi16(sxy));
        assert!((sx - 50).abs() <= 1, "sx = {}", sx);
        assert!((sy - 100).abs() <= 1, "sy = {}", sy);
        assert_eq!(g.read_ctrl(31), 0, "no flags expected");
    }

    #[test]
    fn control_register_quirks() {
        let mut g = Gte::new();
        g.write_ctrl(26, 0xFFFF); // H reads back sign-extended
        assert_eq!(g.read_ctrl(26), 0xFFFF_FFFF);
        g.write_data(28, 0x7FFF); // IRGB sets IR1..3 = 31 * 0x80
        assert_eq!(g.read_data(9), 0xF80);
        assert_eq!(g.read_data(29), 0x7FFF); // ORGB
    }

    #[test]
    fn division_matches_exact_for_simple_ratio() {
        let mut g = Gte::new();
        // 500/1000 in 0x10000 fixed point = 0x8000 (hardware approximation within 1)
        let q = g.divide(500, 1000) as i64;
        assert!((q - 0x8000).abs() <= 2, "q = {:X}", q);
        // overflow case
        assert_eq!(g.divide(2000, 1000), 0x1FFFF);
    }
}
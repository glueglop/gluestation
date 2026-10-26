pub const VRAM_WIDTH: usize = 1024;
pub const VRAM_HEIGHT: usize = 512;

#[derive(Clone, Copy)]
struct Transfer {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    cur_x: usize,
    cur_y: usize,
}

#[derive(Clone, Copy)]
enum GpuState {
    Ready,
    ImageLoad(Transfer),
    ImageStore(Transfer),
}

#[derive(Clone, Copy, Default)]
struct Vertex {
    x: i32,
    y: i32,
    r: i32,
    g: i32,
    b: i32,
    u: i32,
    v: i32,
}

#[derive(Clone, Copy)]
struct DrawParams {
    textured: bool,
    raw: bool,
    semi: bool,
    clut: u32,
}

fn sext11(v: u32) -> i32 {
    ((v << 21) as i32) >> 21
}

fn pack555(r: i32, g: i32, b: i32) -> u16 {
    let r = (r.clamp(0, 255) >> 3) as u16;
    let g = (g.clamp(0, 255) >> 3) as u16;
    let b = (b.clamp(0, 255) >> 3) as u16;
    r | (g << 5) | (b << 10)
}

fn edge(ax: i32, ay: i32, bx: i32, by: i32, px: i32, py: i32) -> i64 {
    ((bx - ax) as i64) * ((py - ay) as i64) - ((by - ay) as i64) * ((px - ax) as i64)
}

fn blend(back: u16, fore: u16, mode: u32) -> u16 {
    let comp = |shift: u32| -> u16 {
        let b = ((back >> shift) & 0x1F) as i32;
        let f = ((fore >> shift) & 0x1F) as i32;
        let r = match mode {
            0 => (b + f) / 2,
            1 => b + f,
            2 => b - f,
            _ => b + f / 4,
        };
        r.clamp(0, 31) as u16
    };
    comp(0) | (comp(5) << 5) | (comp(10) << 10) | (fore & 0x8000)
}

fn modulate(texel: u16, r: i32, g: i32, b: i32) -> u16 {
    let m = |t: u32, c: i32| -> u16 { (((t * (c.clamp(0, 255) as u32)) >> 7).min(31)) as u16 };
    let tr = (texel & 0x1F) as u32;
    let tg = ((texel >> 5) & 0x1F) as u32;
    let tb = ((texel >> 10) & 0x1F) as u32;
    m(tr, r) | (m(tg, g) << 5) | (m(tb, b) << 10) | (texel & 0x8000)
}

pub struct Gpu {
    pub vram: Box<[u16; VRAM_WIDTH * VRAM_HEIGHT]>,
    pub gpustat: u32,
    gpuread: u32,

    state: GpuState,
    cmd_buffer: Vec<u32>,
    cmd_words_needed: usize,

    texpage: u32,
    tw_mask_x: u32,
    tw_mask_y: u32,
    tw_off_x: u32,
    tw_off_y: u32,
    flip_x: bool,
    flip_y: bool,

    area_x1: i32,
    area_y1: i32,
    area_x2: i32,
    area_y2: i32,
    draw_offset_x: i32,
    draw_offset_y: i32,

    set_mask: bool,
    check_mask: bool,

    pub disp_x: usize,
    pub disp_y: usize,
    pub disp_w: usize,
    pub disp_h: usize,
}

impl Gpu {
    pub fn new() -> Self {
        let vram: Box<[u16; VRAM_WIDTH * VRAM_HEIGHT]> = vec![0u16; VRAM_WIDTH * VRAM_HEIGHT]
            .into_boxed_slice()
            .try_into()
            .unwrap();

        let mut gpu = Self {
            vram,
            gpustat: 0x1C80_2000,
            gpuread: 0,
            state: GpuState::Ready,
            cmd_buffer: Vec::with_capacity(16),
            cmd_words_needed: 0,
            texpage: 0,
            tw_mask_x: 0,
            tw_mask_y: 0,
            tw_off_x: 0,
            tw_off_y: 0,
            flip_x: false,
            flip_y: false,
            area_x1: 0,
            area_y1: 0,
            area_x2: VRAM_WIDTH as i32 - 1,
            area_y2: VRAM_HEIGHT as i32 - 1,
            draw_offset_x: 0,
            draw_offset_y: 0,
            set_mask: false,
            check_mask: false,
            disp_x: 0,
            disp_y: 0,
            disp_w: 320,
            disp_h: 240,
        };
        gpu.reset_draw_state();
        gpu
    }

    fn reset_draw_state(&mut self) {
        self.texpage = 0;
        self.tw_mask_x = 0;
        self.tw_mask_y = 0;
        self.tw_off_x = 0;
        self.tw_off_y = 0;
        self.flip_x = false;
        self.flip_y = false;
        self.area_x1 = 0;
        self.area_y1 = 0;
        self.area_x2 = VRAM_WIDTH as i32 - 1;
        self.area_y2 = VRAM_HEIGHT as i32 - 1;
        self.draw_offset_x = 0;
        self.draw_offset_y = 0;
        self.set_mask = false;
        self.check_mask = false;
    }

    pub fn vblank(&mut self) {
        if self.gpustat & (1 << 19) != 0 {
            self.gpustat ^= 1 << 31;
        } else {
            self.gpustat &= !(1 << 31);
        }
    }


    pub fn read_gpustat(&self) -> u32 {
        let mut s = self.gpustat & !(1 << 25);
        let req = match (s >> 29) & 3 {
            0 => 0,
            1 => 1,
            2 => (s >> 28) & 1,
            _ => (s >> 27) & 1,
        };
        s |= req << 25;
        s
    }

    pub fn read_gp0(&mut self) -> u32 {
        if let GpuState::ImageStore(mut t) = self.state {
            let mut word = 0u32;
            for i in 0..2 {
                if t.cur_y >= t.h {
                    break;
                }
                let x = (t.x + t.cur_x) & (VRAM_WIDTH - 1);
                let y = (t.y + t.cur_y) & (VRAM_HEIGHT - 1);
                word |= (self.vram[y * VRAM_WIDTH + x] as u32) << (16 * i);
                t.cur_x += 1;
                if t.cur_x >= t.w {
                    t.cur_x = 0;
                    t.cur_y += 1;
                }
            }
            self.gpuread = word;
            self.state = if t.cur_y >= t.h {
                GpuState::Ready
            } else {
                GpuState::ImageStore(t)
            };
            return word;
        }
        self.gpuread
    }


    pub fn write_gp1(&mut self, val: u32) {
        let opcode = (val >> 24) & 0xFF;
        match opcode {
            0x00 => {
                self.gpustat = 0x1C80_2000;
                self.reset_draw_state();
                self.cmd_buffer.clear();
                self.state = GpuState::Ready;
            }
            0x01 => {
                self.cmd_buffer.clear();
                self.state = GpuState::Ready;
            }
            0x02 => {
                self.gpustat &= !(1 << 24);
            }
            0x03 => {
                if (val & 1) != 0 {
                    self.gpustat |= 1 << 23;
                } else {
                    self.gpustat &= !(1 << 23);
                }
            }
            0x04 => {
                let dma_dir = val & 0x03;
                self.gpustat = (self.gpustat & !(0x03 << 29)) | (dma_dir << 29);
            }
            0x05 => {
                self.disp_x = (val & 0x3FF) as usize;
                self.disp_y = ((val >> 10) & 0x1FF) as usize;
            }
            0x08 => {
                self.gpustat &= !((0x3F << 17) | (1 << 16) | (1 << 14));
                self.gpustat |= ((val & 0x3F) << 17) | (((val >> 6) & 1) << 16) | (((val >> 7) & 1) << 14);

                self.disp_w = if val & 0x40 != 0 {
                    368
                } else {
                    match val & 3 {
                        0 => 256,
                        1 => 320,
                        2 => 512,
                        _ => 640,
                    }
                };
                self.disp_h = if (val & 4) != 0 { 480 } else { 240 };
            }
            0x10 => {
                self.gpuread = match val & 0xF {
                    2 => {
                        self.tw_mask_x
                            | (self.tw_mask_y << 5)
                            | (self.tw_off_x << 10)
                            | (self.tw_off_y << 15)
                    }
                    3 => (self.area_x1 as u32) | ((self.area_y1 as u32) << 10),
                    4 => (self.area_x2 as u32) | ((self.area_y2 as u32) << 10),
                    5 => {
                        ((self.draw_offset_x as u32) & 0x7FF)
                            | (((self.draw_offset_y as u32) & 0x7FF) << 11)
                    }
                    7 => 2,
                    _ => self.gpuread,
                };
            }
            _ => {}
        }
    }

    fn gp0_words_needed(op: u32) -> usize {
        match op {
            0x02 => 3,
            0x20..=0x3F => {
                let quad = op & 0x08 != 0;
                let textured = op & 0x04 != 0;
                let gouraud = op & 0x10 != 0;
                let n = if quad { 4 } else { 3 };
                n + if textured { n } else { 0 } + if gouraud { n } else { 1 }
            }
            0x40..=0x5F => {
                if op & 0x10 != 0 {
                    4
                } else {
                    3
                }
            }
            0x60..=0x7F => {
                let textured = op & 0x04 != 0;
                let variable = (op >> 3) & 3 == 0;
                2 + if textured { 1 } else { 0 } + if variable { 1 } else { 0 }
            }
            0x80 => 4,
            0xA0 | 0xC0 => 3,
            _ => 1,
        }
    }

    pub fn write_gp0(&mut self, val: u32) {
        if let GpuState::ImageLoad(mut t) = self.state {
            for px in [(val & 0xFFFF) as u16, (val >> 16) as u16] {
                if t.cur_y >= t.h {
                    break;
                }
                let x = (t.x + t.cur_x) & (VRAM_WIDTH - 1);
                let y = (t.y + t.cur_y) & (VRAM_HEIGHT - 1);
                let idx = y * VRAM_WIDTH + x;
                if !(self.check_mask && self.vram[idx] & 0x8000 != 0) {
                    self.vram[idx] = px | if self.set_mask { 0x8000 } else { 0 };
                }
                t.cur_x += 1;
                if t.cur_x >= t.w {
                    t.cur_x = 0;
                    t.cur_y += 1;
                }
            }
            self.state = if t.cur_y >= t.h {
                GpuState::Ready
            } else {
                GpuState::ImageLoad(t)
            };
            return;
        }

        if self.cmd_buffer.is_empty() {
            self.cmd_words_needed = Self::gp0_words_needed(val >> 24);
        }
        self.cmd_buffer.push(val);

        let op = self.cmd_buffer[0] >> 24;
        let is_polyline = (0x40..=0x5F).contains(&op) && (op & 0x08) != 0;

        if is_polyline {
            let n = self.cmd_buffer.len();
            let shaded = op & 0x10 != 0;
            let terminator = (val & 0xF000_F000) == 0x5000_5000;
            let can_terminate = if shaded { n >= 5 && n % 2 == 1 } else { n >= 4 };

            if can_terminate && terminator {
                self.cmd_buffer.pop();
                self.execute_gp0_cmd();
            } else if n >= 4096 {
                self.execute_gp0_cmd();
            }
            return;
        }

        if self.cmd_buffer.len() >= self.cmd_words_needed {
            self.execute_gp0_cmd();
        }
    }

    fn execute_gp0_cmd(&mut self) {
        let mut buf = std::mem::take(&mut self.cmd_buffer);
        let op = buf[0] >> 24;

        match op {
            0x02 => self.cmd_fill(&buf),
            0x20..=0x3F => self.cmd_polygon(op, &buf),
            0x40..=0x5F => self.cmd_line(op, &buf),
            0x60..=0x7F => self.cmd_rect(op, &buf),
            0x80 => self.cmd_vram_copy(&buf),
            0xA0 => self.cmd_image_load(&buf),
            0xC0 => self.cmd_image_store(&buf),
            0x1F => {
                self.gpustat |= 1 << 24;
            }
            0xE1 => {
                let v = buf[0];
                self.texpage = v & 0xFFFF;
                self.flip_x = v & (1 << 12) != 0;
                self.flip_y = v & (1 << 13) != 0;
                self.gpustat =
                    (self.gpustat & !0x87FF) | (v & 0x7FF) | (((v >> 11) & 1) << 15);
            }
            0xE2 => {
                let v = buf[0];
                self.tw_mask_x = v & 0x1F;
                self.tw_mask_y = (v >> 5) & 0x1F;
                self.tw_off_x = (v >> 10) & 0x1F;
                self.tw_off_y = (v >> 15) & 0x1F;
            }
            0xE3 => {
                self.area_x1 = (buf[0] & 0x3FF) as i32;
                self.area_y1 = ((buf[0] >> 10) & 0x1FF) as i32;
            }
            0xE4 => {
                self.area_x2 = (buf[0] & 0x3FF) as i32;
                self.area_y2 = ((buf[0] >> 10) & 0x1FF) as i32;
            }
            0xE5 => {
                self.draw_offset_x = sext11(buf[0]);
                self.draw_offset_y = sext11(buf[0] >> 11);
            }
            0xE6 => {
                self.set_mask = buf[0] & 1 != 0;
                self.check_mask = buf[0] & 2 != 0;
                self.gpustat = (self.gpustat & !(3 << 11)) | ((buf[0] & 3) << 11);
            }
            _ => {}
        }

        buf.clear();
        self.cmd_buffer = buf;
    }


    fn cmd_fill(&mut self, buf: &[u32]) {
        let color = pack555(
            (buf[0] & 0xFF) as i32,
            ((buf[0] >> 8) & 0xFF) as i32,
            ((buf[0] >> 16) & 0xFF) as i32,
        );
        let x = (buf[1] & 0x3F0) as usize;
        let y = ((buf[1] >> 16) & 0x1FF) as usize;
        let w = (((buf[2] & 0x3FF) + 0xF) & !0xF) as usize;
        let h = ((buf[2] >> 16) & 0x1FF) as usize;

        for row in 0..h {
            for col in 0..w {
                let px = (x + col) & (VRAM_WIDTH - 1);
                let py = (y + row) & (VRAM_HEIGHT - 1);
                self.vram[py * VRAM_WIDTH + px] = color;
            }
        }
    }

    fn transfer_from(buf: &[u32]) -> Transfer {
        let x = (buf[1] & 0x3FF) as usize;
        let y = ((buf[1] >> 16) & 0x1FF) as usize;
        let w = (((buf[2] & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1) as usize;
        let h = ((((buf[2] >> 16) & 0xFFFF).wrapping_sub(1) & 0x1FF) + 1) as usize;
        Transfer {
            x,
            y,
            w,
            h,
            cur_x: 0,
            cur_y: 0,
        }
    }

    fn cmd_image_load(&mut self, buf: &[u32]) {
        self.state = GpuState::ImageLoad(Self::transfer_from(buf));
    }

    fn cmd_image_store(&mut self, buf: &[u32]) {
        self.state = GpuState::ImageStore(Self::transfer_from(buf));
    }

    fn cmd_vram_copy(&mut self, buf: &[u32]) {
        let sx = (buf[1] & 0x3FF) as usize;
        let sy = ((buf[1] >> 16) & 0x1FF) as usize;
        let dx = (buf[2] & 0x3FF) as usize;
        let dy = ((buf[2] >> 16) & 0x1FF) as usize;
        let w = (((buf[3] & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1) as usize;
        let h = ((((buf[3] >> 16) & 0xFFFF).wrapping_sub(1) & 0x1FF) + 1) as usize;

        let mut tmp: Vec<u16> = Vec::with_capacity(w * h);
        for row in 0..h {
            for col in 0..w {
                let x = (sx + col) & (VRAM_WIDTH - 1);
                let y = (sy + row) & (VRAM_HEIGHT - 1);
                tmp.push(self.vram[y * VRAM_WIDTH + x]);
            }
        }
        for row in 0..h {
            for col in 0..w {
                let x = (dx + col) & (VRAM_WIDTH - 1);
                let y = (dy + row) & (VRAM_HEIGHT - 1);
                let idx = y * VRAM_WIDTH + x;
                if self.check_mask && self.vram[idx] & 0x8000 != 0 {
                    continue;
                }
                self.vram[idx] = tmp[row * w + col] | if self.set_mask { 0x8000 } else { 0 };
            }
        }
    }


    fn make_vertex(&self, xy: u32, color: u32, u: i32, v: i32) -> Vertex {
        Vertex {
            x: sext11(xy) + self.draw_offset_x,
            y: sext11(xy >> 16) + self.draw_offset_y,
            r: (color & 0xFF) as i32,
            g: ((color >> 8) & 0xFF) as i32,
            b: ((color >> 16) & 0xFF) as i32,
            u,
            v,
        }
    }

    fn cmd_polygon(&mut self, op: u32, buf: &[u32]) {
        let gouraud = op & 0x10 != 0;
        let quad = op & 0x08 != 0;
        let textured = op & 0x04 != 0;
        let semi = op & 0x02 != 0;
        let raw = op & 0x01 != 0;
        let n = if quad { 4 } else { 3 };

        let mut verts = [Vertex::default(); 4];
        let mut idx = 0usize;
        let mut clut = 0u32;
        let mut tpage = 0u32;

        for i in 0..n {
            let color = if i == 0 || gouraud {
                let c = buf[idx];
                idx += 1;
                c
            } else {
                buf[0]
            };
            let xy = buf[idx];
            idx += 1;

            let (mut u, mut v) = (0i32, 0i32);
            if textured {
                let uv = buf[idx];
                idx += 1;
                u = (uv & 0xFF) as i32;
                v = ((uv >> 8) & 0xFF) as i32;
                if i == 0 {
                    clut = uv >> 16;
                } else if i == 1 {
                    tpage = uv >> 16;
                }
            }
            verts[i] = self.make_vertex(xy, color, u, v);
        }

        if textured {
            self.texpage = (self.texpage & !0x1FF) | (tpage & 0x1FF);
            self.gpustat = (self.gpustat & !0x1FF) | (tpage & 0x1FF);
        }

        let params = DrawParams {
            textured,
            raw,
            semi,
            clut,
        };

        self.draw_triangle(verts[0], verts[1], verts[2], params);
        if quad {
            self.draw_triangle(verts[1], verts[2], verts[3], params);
        }
    }

    fn cmd_rect(&mut self, op: u32, buf: &[u32]) {
        let textured = op & 0x04 != 0;
        let semi = op & 0x02 != 0;
        let raw = op & 0x01 != 0;
        let size = (op >> 3) & 3;

        let color = buf[0];
        let xy = buf[1];
        let mut idx = 2;

        let (mut u0, mut v0, mut clut) = (0i32, 0i32, 0u32);
        if textured {
            let w = buf[idx];
            idx += 1;
            u0 = (w & 0xFF) as i32;
            v0 = ((w >> 8) & 0xFF) as i32;
            clut = w >> 16;
        }

        let (w, h) = match size {
            0 => {
                let s = buf[idx];
                ((s & 0x3FF) as i32, ((s >> 16) & 0x1FF) as i32)
            }
            1 => (1, 1),
            2 => (8, 8),
            _ => (16, 16),
        };

        let x0 = sext11(xy) + self.draw_offset_x;
        let y0 = sext11(xy >> 16) + self.draw_offset_y;

        let cr = (color & 0xFF) as i32;
        let cg = ((color >> 8) & 0xFF) as i32;
        let cb = ((color >> 16) & 0xFF) as i32;
        let flat = pack555(cr, cg, cb);

        for row in 0..h {
            for col in 0..w {
                let (c, s) = if textured {
                    let tu = if self.flip_x { u0 - col } else { u0 + col };
                    let tv = if self.flip_y { v0 - row } else { v0 + row };
                    match self.sample_texture(tu, tv, clut) {
                        None => continue,
                        Some(t) => {
                            let out = if raw { t } else { modulate(t, cr, cg, cb) };
                            (out, semi && (t & 0x8000) != 0)
                        }
                    }
                } else {
                    (flat, semi)
                };
                self.put_pixel(x0 + col, y0 + row, c, s);
            }
        }
    }

    fn cmd_line(&mut self, op: u32, buf: &[u32]) {
        let shaded = op & 0x10 != 0;
        let semi = op & 0x02 != 0;

        let mut pts: Vec<Vertex> = Vec::new();
        let mut color = buf[0];
        let mut idx = 1;
        loop {
            if idx >= buf.len() {
                break;
            }
            let xy = buf[idx];
            idx += 1;
            pts.push(self.make_vertex(xy, color, 0, 0));
            if shaded {
                if idx >= buf.len() {
                    break;
                }
                color = buf[idx];
                idx += 1;
            }
        }

        for pair in pts.windows(2) {
            self.draw_line(pair[0], pair[1], semi);
        }
    }

    fn draw_line(&mut self, a: Vertex, b: Vertex, semi: bool) {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        if dx.abs() > 1023 || dy.abs() > 511 {
            return;
        }
        let n = dx.abs().max(dy.abs());
        let d = n.max(1);
        for i in 0..=n {
            let x = a.x + dx * i / d;
            let y = a.y + dy * i / d;
            let lerp = |p: i32, q: i32| p + (q - p) * i / d;
            let c = pack555(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b));
            self.put_pixel(x, y, c, semi);
        }
    }

    fn draw_triangle(&mut self, a: Vertex, mut b: Vertex, mut c: Vertex, p: DrawParams) {
        let mut area = edge(a.x, a.y, b.x, b.y, c.x, c.y);
        if area == 0 {
            return;
        }
        if area < 0 {
            std::mem::swap(&mut b, &mut c);
            area = -area;
        }

        let min_x = a.x.min(b.x).min(c.x);
        let max_x = a.x.max(b.x).max(c.x);
        let min_y = a.y.min(b.y).min(c.y);
        let max_y = a.y.max(b.y).max(c.y);

        if max_x - min_x > 1023 || max_y - min_y > 511 {
            return;
        }

        let x0 = min_x.max(self.area_x1);
        let x1 = max_x.min(self.area_x2);
        let y0 = min_y.max(self.area_y1);
        let y1 = max_y.min(self.area_y2);
        if x0 > x1 || y0 > y1 {
            return;
        }

        let bias = |s: &Vertex, t: &Vertex| -> i64 {
            let dx = t.x - s.x;
            let dy = t.y - s.y;
            if dy < 0 || (dy == 0 && dx > 0) {
                0
            } else {
                -1
            }
        };
        let bias0 = bias(&b, &c);
        let bias1 = bias(&c, &a);
        let bias2 = bias(&a, &b);

        for y in y0..=y1 {
            for x in x0..=x1 {
                let w0 = edge(b.x, b.y, c.x, c.y, x, y);
                let w1 = edge(c.x, c.y, a.x, a.y, x, y);
                let w2 = edge(a.x, a.y, b.x, b.y, x, y);
                if w0 + bias0 < 0 || w1 + bias1 < 0 || w2 + bias2 < 0 {
                    continue;
                }

                let lerp = |va: i32, vb: i32, vc: i32| -> i32 {
                    ((w0 * va as i64 + w1 * vb as i64 + w2 * vc as i64) / area) as i32
                };

                let cr = lerp(a.r, b.r, c.r);
                let cg = lerp(a.g, b.g, c.g);
                let cb = lerp(a.b, b.b, c.b);

                let (color, semi) = if p.textured {
                    let tu = lerp(a.u, b.u, c.u);
                    let tv = lerp(a.v, b.v, c.v);
                    match self.sample_texture(tu, tv, p.clut) {
                        None => continue,
                        Some(t) => {
                            let out = if p.raw { t } else { modulate(t, cr, cg, cb) };
                            (out, p.semi && (t & 0x8000) != 0)
                        }
                    }
                } else {
                    (pack555(cr, cg, cb), p.semi)
                };

                self.put_pixel(x, y, color, semi);
            }
        }
    }


    fn sample_texture(&self, u: i32, v: i32, clut: u32) -> Option<u16> {
        let tp = self.texpage;
        let base_x = ((tp & 0xF) * 64) as usize;
        let base_y = (((tp >> 4) & 1) * 256) as usize;
        let depth = (tp >> 7) & 3;

        let mut u = (u & 0xFF) as u32;
        let mut v = (v & 0xFF) as u32;
        u = (u & !(self.tw_mask_x << 3)) | ((self.tw_off_x & self.tw_mask_x) << 3);
        v = (v & !(self.tw_mask_y << 3)) | ((self.tw_off_y & self.tw_mask_y) << 3);
        let (u, v) = (u as usize, v as usize);

        let clut_x = ((clut & 0x3F) * 16) as usize;
        let clut_y = ((clut >> 6) & 0x1FF) as usize;

        let texel = match depth {
            0 => {
                let word = self.vram_get(base_x + u / 4, base_y + v);
                let index = ((word >> ((u % 4) * 4)) & 0xF) as usize;
                self.vram_get(clut_x + index, clut_y)
            }
            1 => {
                let word = self.vram_get(base_x + u / 2, base_y + v);
                let index = ((word >> ((u % 2) * 8)) & 0xFF) as usize;
                self.vram_get(clut_x + index, clut_y)
            }
            _ => {
                self.vram_get(base_x + u, base_y + v)
            }
        };

        if texel == 0 {
            None
        } else {
            Some(texel)
        }
    }

    fn put_pixel(&mut self, x: i32, y: i32, color: u16, semi: bool) {
        if x < self.area_x1 || x > self.area_x2 || y < self.area_y1 || y > self.area_y2 {
            return;
        }
        let idx = (y as usize) * VRAM_WIDTH + (x as usize);
        let back = self.vram[idx];

        if self.check_mask && back & 0x8000 != 0 {
            return;
        }

        let mut out = if semi {
            blend(back, color, (self.texpage >> 5) & 3)
        } else {
            color
        };
        if self.set_mask {
            out |= 0x8000;
        }
        self.vram[idx] = out;
    }

    fn vram_get(&self, x: usize, y: usize) -> u16 {
        self.vram[(y & (VRAM_HEIGHT - 1)) * VRAM_WIDTH + (x & (VRAM_WIDTH - 1))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(g: &mut Gpu, words: &[u32]) {
        for &w in words {
            g.write_gp0(w);
        }
    }

    fn px(g: &Gpu, x: usize, y: usize) -> u16 {
        g.vram[y * VRAM_WIDTH + x]
    }

    #[test]
    fn fill_rect() {
        let mut g = Gpu::new();
        send(&mut g, &[0x0200_00FF, 0x0010_0020, 0x0008_0010]);
        assert_eq!(px(&g, 0x20, 0x10), 0x001F);
        assert_eq!(px(&g, 0x20 + 15, 0x10 + 7), 0x001F);
        assert_eq!(px(&g, 0x20 + 16, 0x10), 0);
        assert_eq!(px(&g, 0x20, 0x10 + 8), 0);
    }

    #[test]
    fn flat_triangle() {
        let mut g = Gpu::new();
        send(&mut g, &[0x2000_00FF, 10 | (10 << 16), 50 | (10 << 16), 10 | (50 << 16)]);
        assert_eq!(px(&g, 11, 11), 0x001F);
        assert_eq!(px(&g, 60, 60), 0);
    }

    #[test]
    fn image_load_and_textured_quad_scales_with_uv() {
        let mut g = Gpu::new();
        send(&mut g, &[0xA000_0000, 0, 1 | (1 << 16), 0x0000_0021]);
        send(&mut g, &[0xA000_0000, 100 << 16, 3 | (1 << 16), 0x001F_0000, 0x0000_03E0]);
        send(&mut g, &[0xE100_0000]);
        let clut = 100u32 << 6;
        send(
            &mut g,
            &[
                0x2D80_8080,
                100 | (100 << 16),
                (clut << 16),
                104 | (100 << 16),
                2,
                100 | (102 << 16),
                0x0100,
                104 | (102 << 16),
                0x0102,
            ],
        );
        assert_eq!(px(&g, 100, 100), 0x001F);
        assert_eq!(px(&g, 101, 100), 0x001F);
        assert_eq!(px(&g, 102, 100), 0x03E0);
        assert_eq!(px(&g, 103, 100), 0x03E0);
        assert_eq!(px(&g, 104, 100), 0);
        assert_eq!(px(&g, 100, 102), 0);
    }

    #[test]
    fn gp1_display_mode_sets_gpustat_bits() {
        let mut g = Gpu::new();
        g.write_gp1(0x0800_0001);
        assert_eq!((g.gpustat >> 17) & 3, 1);
        assert_eq!(g.disp_w, 320);
        g.write_gp1(0x0800_0044);
        assert_eq!((g.gpustat >> 16) & 1, 1);
        assert_eq!((g.gpustat >> 19) & 1, 1);
        assert_eq!(g.disp_w, 368);
        assert_eq!(g.disp_h, 480);
    }

    #[test]
    fn vram_store_reads_back() {
        let mut g = Gpu::new();
        send(&mut g, &[0xA000_0000, 0, 2 | (1 << 16), 0x2222_1111]);
        send(&mut g, &[0xC000_0000, 0, 2 | (1 << 16)]);
        assert_eq!(g.read_gp0(), 0x2222_1111);
    }
}

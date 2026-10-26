pub const RAM_SIZE: usize = 2 * 1024 * 1024; // 2 MB
const RAM_MASK: usize = RAM_SIZE - 1;

pub struct Ram {
    pub data: Box<[u8; RAM_SIZE]>,
}

impl Ram {
    pub fn new() -> Self {
        let data: Box<[u8; RAM_SIZE]> = vec![0u8; RAM_SIZE]
            .into_boxed_slice()
            .try_into()
            .unwrap();
        Self { data }
    }

    #[inline]
    fn idx(offset: u32, i: usize) -> usize {
        (offset as usize + i) & RAM_MASK
    }

    pub fn read8(&self, offset: u32) -> u8 {
        self.data[Self::idx(offset, 0)]
    }

    pub fn read16(&self, offset: u32) -> u16 {
        u16::from_le_bytes([
            self.data[Self::idx(offset, 0)],
            self.data[Self::idx(offset, 1)],
        ])
    }

    pub fn read32(&self, offset: u32) -> u32 {
        u32::from_le_bytes([
            self.data[Self::idx(offset, 0)],
            self.data[Self::idx(offset, 1)],
            self.data[Self::idx(offset, 2)],
            self.data[Self::idx(offset, 3)],
        ])
    }

    pub fn write8(&mut self, offset: u32, val: u8) {
        self.data[Self::idx(offset, 0)] = val;
    }

    pub fn write16(&mut self, offset: u32, val: u16) {
        for (i, b) in val.to_le_bytes().iter().enumerate() {
            self.data[Self::idx(offset, i)] = *b;
        }
    }

    pub fn write32(&mut self, offset: u32, val: u32) {
        for (i, b) in val.to_le_bytes().iter().enumerate() {
            self.data[Self::idx(offset, i)] = *b;
        }
    }
}

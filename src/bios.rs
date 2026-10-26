use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const BIOS_SIZE: usize = 512 * 1024; // 512 KB

pub struct Bios {
    data: Box<[u8; BIOS_SIZE]>,
}

impl Bios {
    pub fn new<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let mut data = Box::new([0u8; BIOS_SIZE]);
        file.read_exact(data.as_mut_slice())?;
        Ok(Self { data })
    }

    pub fn read8(&self, offset: u32) -> u8 {
        self.data[offset as usize]
    }

    pub fn read16(&self, offset: u32) -> u16 {
        let o = offset as usize;
        u16::from_le_bytes([self.data[o], self.data[o + 1]])
    }

    pub fn read32(&self, offset: u32) -> u32 {
        let o = offset as usize;
        u32::from_le_bytes([
            self.data[o],
            self.data[o + 1],
            self.data[o + 2],
            self.data[o + 3],
        ])
    }
}

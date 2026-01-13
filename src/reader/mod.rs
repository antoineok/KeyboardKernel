use alloc::vec::Vec;

#[derive(Copy, Clone)]
pub struct Reader {
    base: *const u8,
    pos: usize,
    len: usize,
}

pub enum Endian {
    Big,
    Little,
}

impl Reader {
    /// Create a new reader for an FDT located at the given physical `addr`
    pub fn new(addr: u64, len: usize) -> Self {
        Self {
            base: addr as *const u8,
            pos: 0,
            len,
        }
    }

    pub fn from_vec<T>(vec: Vec<T>) -> Self {
        let len = vec.len() * core::mem::size_of::<T>();
        let base = vec.as_ptr() as *const u8;
        Self { base, pos: 0, len }
    }

    // Core reads
    pub unsafe fn read_u8(&mut self) -> u8 {
        let val = self.peek_u8();
        self.skip(1);
        val
    }

    pub unsafe fn read_u16(&mut self, endian: Endian) -> u16 {
        let val = self.peek_u16(endian);
        self.skip(2);
        val
    }

    pub unsafe fn read_u32(&mut self, endian: Endian) -> u32 {
        let val = self.peek_u32(endian);
        self.skip(4);
        val
    }

    pub unsafe fn read_u64(&mut self, endian: Endian) -> u64 {
        let val = self.peek_u64(endian);
        self.skip(8);
        val
    }

    pub unsafe fn read_i8(&mut self) -> i8 {
        (self.read_u8()) as i8
    }

    pub unsafe fn read_i16(&mut self, endian: Endian) -> i16 {
        (self.read_u16(endian)) as i16
    }

    pub unsafe fn read_i32(&mut self, endian: Endian) -> i32 {
        (self.read_u32(endian)) as i32
    }

    pub unsafe fn read_i64(&mut self, endian: Endian) -> i64 {
        (self.read_u64(endian)) as i64
    }

    // Peek reads (no cursor movement)
    pub unsafe fn peek_u8(&self) -> u8 {
        if self.remaining() == 0 {
            panic!("Out of bounds");
        }
        let val = self.base.offset(self.pos as isize).read();
        val
    }

    pub unsafe fn peek_u16(&self, endian: Endian) -> u16 {
        if self.remaining() < 2 {
            panic!("Out of bounds");
        }
        let b1 = self.base.offset(self.pos as isize).read();
        let b2 = self.base.offset((self.pos + 1) as isize).read();
        match endian {
            Endian::Big => u16::from_be_bytes([b1, b2]),
            Endian::Little => u16::from_le_bytes([b1, b2]),
        }
    }

    pub unsafe fn peek_u32(&self, endian: Endian) -> u32 {
        if self.remaining() < 4 {
            panic!("Out of bounds");
        }
        let b1 = self.base.offset(self.pos as isize).read();
        let b2 = self.base.offset((self.pos + 1) as isize).read();
        let b3 = self.base.offset((self.pos + 2) as isize).read();
        let b4 = self.base.offset((self.pos + 3) as isize).read();
        match endian {
            Endian::Big => u32::from_be_bytes([b1, b2, b3, b4]),
            Endian::Little => u32::from_le_bytes([b1, b2, b3, b4]),
        }
    }

    pub unsafe fn peek_u64(&self, endian: Endian) -> u64 {
        if self.remaining() < 8 {
            panic!("Out of bounds");
        }
        let b1 = self.base.offset(self.pos as isize).read();
        let b2 = self.base.offset((self.pos + 1) as isize).read();
        let b3 = self.base.offset((self.pos + 2) as isize).read();
        let b4 = self.base.offset((self.pos + 3) as isize).read();
        let b5 = self.base.offset((self.pos + 4) as isize).read();
        let b6 = self.base.offset((self.pos + 5) as isize).read();
        let b7 = self.base.offset((self.pos + 6) as isize).read();
        let b8 = self.base.offset((self.pos + 7) as isize).read();
        match endian {
            Endian::Big => u64::from_be_bytes([b1, b2, b3, b4, b5, b6, b7, b8]),
            Endian::Little => u64::from_le_bytes([b1, b2, b3, b4, b5, b6, b7, b8]),
        }
    }

    pub unsafe fn peek_i8(&self) -> i8 {
        (self.peek_u8()) as i8
    }

    pub unsafe fn peek_i16(&self, endian: Endian) -> i16 {
        (self.peek_u16(endian)) as i16
    }

    pub unsafe fn peek_i32(&self, endian: Endian) -> i32 {
        (self.peek_u32(endian)) as i32
    }

    pub unsafe fn peek_i64(&self, endian: Endian) -> i64 {
        (self.peek_u64(endian)) as i64
    }

    // Buffer operations
    pub unsafe fn read_bytes(&mut self, buffer: &mut [u8]) -> usize {
        let read = self.peek_bytes(buffer);
        self.skip(read);
        read
    }

    pub unsafe fn read_bytes_len(&mut self, size: usize) -> &mut [u8] {
        let buffer_size = core::cmp::min(size, self.remaining());
        if buffer_size == 0 {
            return &mut [];
        }
        let ptr = self.base.offset(self.pos as isize) as *mut u8;
        let slice = core::slice::from_raw_parts_mut(ptr, buffer_size);
        self.skip(buffer_size);
        slice
    }

    pub unsafe fn peek_bytes(&self, buffer: &mut [u8]) -> usize {
        let buffer_size = core::cmp::min(buffer.len(), self.remaining());
        if buffer_size == 0 {
            return 0;
        }
        self.base
            .offset(self.pos as isize)
            .copy_to(buffer.as_mut_ptr(), buffer_size);
        buffer_size
    }

    pub unsafe fn peek_bytes_len(&mut self, size: usize) -> &mut [u8] {
        let buffer_size = core::cmp::min(size, self.remaining());
        if buffer_size == 0 {
            return &mut [];
        }
        let ptr = self.base.offset(self.pos as isize) as *mut u8;
        let slice = core::slice::from_raw_parts_mut(ptr, buffer_size);
        slice
    }

    pub unsafe fn read_cstring(&mut self) -> &'static str {
        let start = self.position();

        // Find the null terminator
        while self.remaining() > 0 && self.peek_u8() != 0 {
            self.skip(1);
        }

        if self.remaining() == 0 {
            panic!("Null-terminated string not found");
        }

        let len = self.position() - start;
        let bytes = self.base.offset(start as isize) as *const u8;
        self.skip(1); // Skip the null terminator

        let slice = core::slice::from_raw_parts(bytes, len);
        core::str::from_utf8(slice).unwrap_or("")
    }

    // Navigation
    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub fn rewind(&mut self, n: usize) {
        self.pos -= n;
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn seek_start(&mut self) {
        self.seek(0);
    }

    pub fn seek_end(&mut self) {
        self.seek(self.len);
    }

    // State queries
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.len.saturating_sub(self.pos)
    }

    pub fn length(&self) -> usize {
        self.len
    }

    pub fn address(&self) -> u64 {
        self.base as u64
    }

    // Alignment
    pub fn align(&mut self, boundary: usize) {
        assert!(boundary > 0, "boundary must not be zero");

        // how many bytes to skip to reach the next aligned boundary
        let misalignment = self.pos % boundary;
        let skip = if misalignment == 0 {
            0
        } else {
            boundary - misalignment
        };

        let new_pos = self.pos + skip;

        if new_pos > self.len {
            panic!(
                "aligning to boundary {} would overflow: pos={}, len={}",
                boundary, new_pos, self.len
            );
        }

        self.skip(skip);
    }
}

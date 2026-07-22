//! Explicit little-endian wire encoding helpers.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireReadError {
    UnexpectedEnd,
    TrailingData,
}

pub struct WireReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> WireReader<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    pub const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn finish(self) -> Result<(), WireReadError> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(WireReadError::TrailingData)
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, WireReadError> {
        let bytes = self.read_bytes(1)?;
        Ok(bytes[0])
    }

    pub fn read_u16(&mut self) -> Result<u16, WireReadError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32, WireReadError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_u64(&mut self) -> Result<u64, WireReadError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], WireReadError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(WireReadError::UnexpectedEnd)?;

        if end > self.bytes.len() {
            return Err(WireReadError::UnexpectedEnd);
        }

        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireWriteError {
    BufferTooSmall,
}

pub struct WireWriter<'a> {
    bytes: &'a mut [u8],
    length: usize,
}

impl<'a> WireWriter<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, length: 0 }
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub const fn capacity(&self) -> usize {
        self.bytes.len()
    }

    pub const fn remaining(&self) -> usize {
        self.capacity() - self.length
    }

    pub fn as_written(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub fn remaining_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.length..]
    }

    pub fn advance(&mut self, length: usize) -> Result<(), WireWriteError> {
        let end = self
            .length
            .checked_add(length)
            .ok_or(WireWriteError::BufferTooSmall)?;

        if end > self.bytes.len() {
            return Err(WireWriteError::BufferTooSmall);
        }

        self.length = end;
        Ok(())
    }

    pub fn write_u8(&mut self, value: u8) -> Result<(), WireWriteError> {
        self.write_bytes(&[value])
    }

    pub fn write_u16(&mut self, value: u16) -> Result<(), WireWriteError> {
        self.write_bytes(&value.to_le_bytes())
    }

    pub fn write_u32(&mut self, value: u32) -> Result<(), WireWriteError> {
        self.write_bytes(&value.to_le_bytes())
    }

    pub fn write_u64(&mut self, value: u64) -> Result<(), WireWriteError> {
        self.write_bytes(&value.to_le_bytes())
    }

    pub fn write_bytes(&mut self, value: &[u8]) -> Result<(), WireWriteError> {
        let end = self
            .length
            .checked_add(value.len())
            .ok_or(WireWriteError::BufferTooSmall)?;

        if end > self.bytes.len() {
            return Err(WireWriteError::BufferTooSmall);
        }

        self.bytes[self.length..end].copy_from_slice(value);
        self.length = end;
        Ok(())
    }
}

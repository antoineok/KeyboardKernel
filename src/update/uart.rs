extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::{
    drivers::uart::UART,
    update::{crc32::crc32, kernel::KernelUpdater, UpdateError},
};

const FRAME_MAGIC: [u8; 4] = *b"UPD1";
const REPLY_MAGIC: [u8; 4] = *b"URP1";

const MAX_CHUNK_SIZE: usize = 4096;
const UPDATE_TRIGGER: &[u8] = b":update\r";

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum UartUpdateCommand {
    Start = 0x01,
    Chunk = 0x02,
    End = 0x03,
    Abort = 0x04,
    Commit = 0x05,
}

impl UartUpdateCommand {
    fn from_u8(v: u8) -> Result<Self, UpdateError> {
        match v {
            0x01 => Ok(Self::Start),
            0x02 => Ok(Self::Chunk),
            0x03 => Ok(Self::End),
            0x04 => Ok(Self::Abort),
            0x05 => Ok(Self::Commit),
            _ => Err(UpdateError::BadCommand),
        }
    }
}

pub struct UartUpdatePacket {
    pub cmd: UartUpdateCommand,
    pub seq: u32,
    pub payload: Vec<u8>,
}

pub struct UartUpdateSession {
    active: bool,
    expected_size: usize,
    received: usize,
    next_seq: u32,
    buffer: Vec<u8>,
}

impl UartUpdateSession {
    pub const fn new() -> Self {
        Self {
            active: false,
            expected_size: 0,
            received: 0,
            next_seq: 0,
            buffer: Vec::new(),
        }
    }

    pub fn handle_packet(
        &mut self,
        packet: UartUpdatePacket,
    ) -> Result<UartUpdateReply, UpdateError> {
        match packet.cmd {
            UartUpdateCommand::Start => {
                let expected_size = parse_u32_payload(&packet.payload)? as usize;
                self.start(expected_size)?;
                Ok(UartUpdateReply::Ready)
            }

            UartUpdateCommand::Chunk => {
                self.push_chunk(packet.seq, &packet.payload)?;
                Ok(UartUpdateReply::Ack(packet.seq))
            }

            UartUpdateCommand::End => {
                write_reply(UartUpdateReply::EndReceived);
                self.finish()?;
                Ok(UartUpdateReply::StagedOk)
            }

            UartUpdateCommand::Abort => {
                self.abort();
                Ok(UartUpdateReply::Aborted)
            }

            UartUpdateCommand::Commit => {
                write_reply(UartUpdateReply::CommitOk);
                KernelUpdater::commit_and_reboot();
            }
        }
    }

    fn start(&mut self, expected_size: usize) -> Result<(), UpdateError> {
        if expected_size < 4096 {
            return Err(UpdateError::InvalidImage);
        }

        self.buffer.clear();

        self.buffer
            .try_reserve_exact(expected_size)
            .map_err(|_| UpdateError::Alloc)?;

        self.active = true;
        self.expected_size = expected_size;
        self.received = 0;
        self.next_seq = 0;

        Ok(())
    }

    fn push_chunk(&mut self, seq: u32, data: &[u8]) -> Result<(), UpdateError> {
        if !self.active {
            return Err(UpdateError::NoActiveSession);
        }

        if seq != self.next_seq {
            return Err(UpdateError::BadSequence);
        }

        if data.len() > MAX_CHUNK_SIZE {
            return Err(UpdateError::BadPacket);
        }

        if self.received + data.len() > self.expected_size {
            return Err(UpdateError::TooMuchData);
        }

        self.buffer.extend_from_slice(data);
        self.received += data.len();
        self.next_seq += 1;

        Ok(())
    }

    fn finish(&mut self) -> Result<(), UpdateError> {
        if !self.active {
            return Err(UpdateError::NoActiveSession);
        }

        if self.received != self.expected_size {
            return Err(UpdateError::VerifyFailed);
        }

        KernelUpdater::stage(&self.buffer)?;
        let digest = crate::update::sha256::sha256(&self.buffer);
        KernelUpdater::verify_staged(self.expected_size, &digest)?;

        self.active = false;

        Ok(())
    }

    fn abort(&mut self) {
        self.active = false;
        self.expected_size = 0;
        self.received = 0;
        self.next_seq = 0;
        self.buffer.clear();
    }
}

#[derive(Debug, Copy, Clone)]
pub enum UartUpdateReply {
    Ready,
    Ack(u32),
    Nack(u32),
    EndReceived,
    StagedOk,
    CommitOk,
    Aborted,
    Error(UpdateReplyError),
}

#[repr(u8)]
#[derive(Debug, Copy, Clone)]
pub enum UpdateReplyError {
    Generic = 0x01,
    BadPacket = 0x02,
    BadSequence = 0x03,
    CrcFailed = 0x04,
    VerifyFailed = 0x05,
}

impl From<&UpdateError> for UpdateReplyError {
    fn from(e: &UpdateError) -> Self {
        match e {
            UpdateError::BadPacket => Self::BadPacket,
            UpdateError::BadSequence => Self::BadSequence,
            UpdateError::CrcFailed => Self::CrcFailed,
            UpdateError::VerifyFailed => Self::VerifyFailed,
            UpdateError::InvalidImage => Self::VerifyFailed,
            _ => Self::Generic,
        }
    }
}

pub struct UartUpdateService {
    trigger_pos: usize,
    binary_mode: bool,
    session: UartUpdateSession,
}

impl UartUpdateService {
    pub const fn new() -> Self {
        Self {
            trigger_pos: 0,
            binary_mode: false,
            session: UartUpdateSession::new(),
        }
    }

    pub fn is_binary_mode(&self) -> bool {
        self.binary_mode
    }

    pub fn poll(&mut self) {
        if !self.binary_mode {
            self.poll_trigger();
            return;
        }

        match try_read_packet_blocking_if_available() {
            Ok(Some(packet)) => {
                let seq = packet.seq;

                match self.session.handle_packet(packet) {
                    Ok(reply) => write_reply(reply),
                    Err(e) => {
                        let code = UpdateReplyError::from(&e);
                        write_reply(UartUpdateReply::Error(code));

                        if matches!(e, UpdateError::BadSequence) {
                            write_reply(UartUpdateReply::Nack(seq));
                        }
                    }
                }
            }

            Ok(None) => {}

            Err(e) => {
                let code = UpdateReplyError::from(&e);
                write_reply(UartUpdateReply::Error(code));
            }
        }
    }

    fn poll_trigger(&mut self) {
        let uart = UART.get();

        while let Some(byte) = uart.try_read_byte() {
            if byte == UPDATE_TRIGGER[self.trigger_pos] {
                self.trigger_pos += 1;

                if self.trigger_pos == UPDATE_TRIGGER.len() {
                    self.trigger_pos = 0;
                    self.binary_mode = true;

                    // Once the binary updater owns UART TX, suppress normal
                    // logs from every CPU so they cannot corrupt the protocol.
                    crate::drivers::uart::disable_logging();
                    uart.write_text("UPDATE MODE\n");
                    return;
                }
            } else {
                self.trigger_pos = if byte == UPDATE_TRIGGER[0] { 1 } else { 0 };
            }
        }
    }
}

pub struct UartUpdateServiceStatic(UnsafeCell<UartUpdateService>);

unsafe impl Sync for UartUpdateServiceStatic {}

pub static UART_UPDATE: UartUpdateServiceStatic =
    UartUpdateServiceStatic(UnsafeCell::new(UartUpdateService::new()));

impl UartUpdateServiceStatic {
    pub fn get_mut(&self) -> &mut UartUpdateService {
        unsafe { &mut *self.0.get() }
    }

    pub fn get(&self) -> &UartUpdateService {
        unsafe { &*self.0.get() }
    }
}

fn try_read_packet_blocking_if_available() -> Result<Option<UartUpdatePacket>, UpdateError> {
    let uart = UART.get();

    let first = match uart.try_read_byte() {
        Some(b) => b,
        None => return Ok(None),
    };

    read_packet_blocking_with_first(first).map(Some)
}

fn read_packet_blocking_with_first(first: u8) -> Result<UartUpdatePacket, UpdateError> {
    let uart = UART.get();

    let mut window = [0u8; 4];
    window[3] = first;

    loop {
        if window == FRAME_MAGIC {
            break;
        }

        window[0] = window[1];
        window[1] = window[2];
        window[2] = window[3];
        window[3] = uart.read_byte();
    }

    let cmd = UartUpdateCommand::from_u8(uart.read_byte())?;
    let seq = read_u32_le_uart();
    let len = read_u32_le_uart();
    let expected_crc = read_u32_le_uart();

    if len as usize > MAX_CHUNK_SIZE {
        return Err(UpdateError::BadPacket);
    }

    let mut payload = Vec::new();

    payload
        .try_reserve_exact(len as usize)
        .map_err(|_| UpdateError::Alloc)?;

    for _ in 0..len {
        payload.push(uart.read_byte());
    }

    let actual_crc = crc32(&payload);

    if actual_crc != expected_crc {
        return Err(UpdateError::CrcFailed);
    }

    Ok(UartUpdatePacket { cmd, seq, payload })
}

fn read_u32_le_uart() -> u32 {
    let uart = UART.get();

    let b0 = uart.read_byte();
    let b1 = uart.read_byte();
    let b2 = uart.read_byte();
    let b3 = uart.read_byte();

    u32::from_le_bytes([b0, b1, b2, b3])
}

fn write_u32_le_uart(v: u32) {
    let uart = UART.get();

    for b in v.to_le_bytes() {
        uart.write_byte(b);
    }
}

pub fn write_reply(reply: UartUpdateReply) {
    let uart = UART.get();

    for b in REPLY_MAGIC {
        uart.write_byte(b);
    }

    match reply {
        UartUpdateReply::Ready => {
            uart.write_byte(0x10);
            write_u32_le_uart(0);
        }

        UartUpdateReply::Ack(seq) => {
            uart.write_byte(0x11);
            write_u32_le_uart(seq);
        }

        UartUpdateReply::Nack(seq) => {
            uart.write_byte(0x12);
            write_u32_le_uart(seq);
        }

        UartUpdateReply::StagedOk => {
            uart.write_byte(0x13);
            write_u32_le_uart(0);
        }

        UartUpdateReply::Aborted => {
            uart.write_byte(0x14);
            write_u32_le_uart(0);
        }

        UartUpdateReply::Error(e) => {
            uart.write_byte(0x15);
            write_u32_le_uart(e as u32);
        }

        UartUpdateReply::EndReceived => {
            uart.write_byte(0x16);
            write_u32_le_uart(0);
        }

        UartUpdateReply::CommitOk => {
            uart.write_byte(0x17);
            write_u32_le_uart(0);
        }
    }
}

fn parse_u32_payload(payload: &[u8]) -> Result<u32, UpdateError> {
    if payload.len() != 4 {
        return Err(UpdateError::BadPacket);
    }

    Ok(u32::from_le_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

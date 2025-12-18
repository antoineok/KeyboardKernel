use core::cell::UnsafeCell;

use crate::{drivers::address::Address, fdt::aliases::AliasEntry};

const MBOX_RESPONSE: u32 = 0x80000000;
const MBOX_FULL: u32 = 0x80000000;
const MBOX_EMPTY: u32 = 0x40000000;
pub const MBOX_REQUEST: u32 = 0;

pub const MBOX_CH_POWER: u8 = 0;
pub const MBOX_CH_FB: u8 = 1;
pub const MBOX_CH_VUART: u8 = 2;
pub const MBOX_CH_VCHIQ: u8 = 3;
pub const MBOX_CH_LEDS: u8 = 4;
pub const MBOX_CH_BTNS: u8 = 5;
pub const MBOX_CH_TOUCH: u8 = 6;
pub const MBOX_CH_COUNT: u8 = 7;
pub const MBOX_CH_PROP: u8 = 8; // Request from ARM for response by VideoCore
pub const MBOX_TAG_SETPOWER: u32 = 0x28001;
pub const MBOX_TAG_SETCLKRATE: u32 = 0x38002;

pub const MBOX_TAG_SETPHYWH: u32 = 0x48003;
pub const MBOX_TAG_SETVIRTWH: u32 = 0x48004;
pub const MBOX_TAG_SETVIRTOFF: u32 = 0x48009;
pub const MBOX_TAG_SETDEPTH: u32 = 0x48005;
pub const MBOX_TAG_SETPXLORDR: u32 = 0x48006;
pub const MBOX_TAG_GETFB: u32 = 0x40001;
pub const MBOX_TAG_GETPITCH: u32 = 0x40008;

pub const MBOX_TAG_LAST: u32 = 0;

#[repr(C, align(16))]
pub struct MailboxBuffer {
    pub(crate) buffer: [u32; 36],
    mbox_read: Address,
    mbox_poll: Address,
    mbox_sender: Address,
    mbox_status: Address,
    mbox_config: Address,
    mbox_write: Address,
}

pub struct MailboxStatic(pub(crate) UnsafeCell<MailboxBuffer>);

unsafe impl Sync for MailboxStatic {}

unsafe impl Sync for MailboxBuffer {}

pub static MAILBOX: MailboxStatic = MailboxStatic(UnsafeCell::new(MailboxBuffer::new()));

impl MailboxStatic {
    pub fn get(&self) -> &mut MailboxBuffer {
        unsafe { &mut *self.0.get() }
    }
}

impl MailboxBuffer {
    pub const fn new() -> Self {
        MailboxBuffer {
            buffer: [0; 36],
            mbox_read: Address(0),
            mbox_poll: Address(0),
            mbox_sender: Address(4),
            mbox_status: Address(8),
            mbox_config: Address(0),
            mbox_write: Address(0),
        }
    }

    pub fn init(&mut self, mmio: Option<AliasEntry>) {
        let base_addr = match mmio {
            Some(entry) => entry.arm_phys,
            None => panic!("Mailbox MMIO entry not found"),
        };

        self.mbox_read.set_address(base_addr);
        self.mbox_poll.set_address(base_addr + 0x10);
        self.mbox_sender.set_address(base_addr + 0x14);
        self.mbox_status.set_address(base_addr + 0x18);
        self.mbox_config.set_address(base_addr + 0x1C);
        self.mbox_write.set_address(base_addr + 0x20);
    }

    pub fn as_mut_ptr(&mut self) -> *mut u32 {
        self.buffer.as_mut_ptr()
    }

    pub fn as_ptr(&self) -> *const u32 {
        self.buffer.as_ptr()
    }

    pub fn call(&mut self, channel: u8) -> bool {
        // Only upper 28 bits of the buffer address are sent; lower 4 bits are the channel
        let mbox_ptr = self.as_ptr() as usize as u32;
        let addr = (mbox_ptr & !0xF) | (channel as u32 & 0xF);

        // Wait until we can write
        while self.mbox_status.read() & MBOX_FULL != 0 {}

        // Write the address of our buffer to the mailbox with the channel appended
        self.mbox_write.write(addr);

        loop {
            // Is there a reply?
            while self.mbox_status.read() & MBOX_EMPTY != 0 {}

            // Is it a reply to our message?
            if self.mbox_read.read() == addr {
                // Is it successful?
                return unsafe { core::ptr::read_volatile(&self.buffer[1]) } == MBOX_RESPONSE;
            }
        }
    }
}

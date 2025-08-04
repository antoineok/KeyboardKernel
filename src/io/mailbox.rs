use core::cell::UnsafeCell;

use crate::io::mmio::MmioAddress;

use super::PERIPHERAL_BASE;

const VIDEOCORE_MBOX: MmioAddress = MmioAddress(PERIPHERAL_BASE + 0x0000B880);
const MBOX_READ: MmioAddress = VIDEOCORE_MBOX.offset(0);
const MBOX_POLL: MmioAddress = VIDEOCORE_MBOX.offset(0x10);
const MBOX_SENDER: MmioAddress = VIDEOCORE_MBOX.offset(0x14);
const MBOX_STATUS: MmioAddress = VIDEOCORE_MBOX.offset(0x18);
const MBOX_CONFIG: MmioAddress = VIDEOCORE_MBOX.offset(0x1C);
const MBOX_WRITE: MmioAddress = VIDEOCORE_MBOX.offset(0x20);

const MBOX_RESPONSE: u32 = 0x80000000;
const MBOX_FULL: u32 = 0x80000000;
const MBOX_EMPTY: u32 = 0x40000000;

#[repr(C, align(16))]
pub struct MailboxBuffer([u32; 36]);

pub struct MailboxStatic(UnsafeCell<MailboxBuffer>);

unsafe impl Sync for MailboxStatic {}

unsafe impl Sync for MailboxBuffer {}

pub static MAILBOX: MailboxStatic = MailboxStatic(UnsafeCell::new(MailboxBuffer::new()));

impl MailboxBuffer {
    pub const fn new() -> Self {
        MailboxBuffer([0; 36])
    }

    pub fn as_mut_ptr(&mut self) -> *mut u32 {
        self.0.as_mut_ptr()
    }

    pub fn as_ptr(&self) -> *const u32 {
        self.0.as_ptr()
    }

    pub fn call(&self, channel: u8) -> bool {
        // Only upper 28 bits of the buffer address are sent; lower 4 bits are the channel
        let mbox_ptr = self.as_ptr() as u32;
        let addr = (mbox_ptr & !0xF) | (channel as u32 & 0xF);

        // Wait until we can write
        while MBOX_STATUS.read() & MBOX_FULL != 0 {}

        // Write the address of our buffer to the mailbox with the channel appended
        MBOX_WRITE.write(addr);

        loop {
            // Is there a reply?
            while MBOX_STATUS.read() & MBOX_EMPTY != 0 {}

            // Is it a reply to our message?
            if MBOX_READ.read() == addr {
                // Is it successful?
                return (&self.0)[1] == MBOX_RESPONSE;
            }
        }
        false
    }
}

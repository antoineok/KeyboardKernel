#![allow(dead_code)]

use core::{cell::UnsafeCell, time::Duration};

use crate::{
    drivers::{
        address::Address,
        gpio::{GpioFunction, GpioPin},
    },
    fdt::AliasEntry,
    time::time_manager,
    uart_println,
};

const FIRMWARE: &[u8; 56508] = include_bytes!("../../data/BCM4345C0.hcd");

trait LoHi<T> {
    fn lo(self) -> T;
    fn hi(self) -> T;
}

impl LoHi<u8> for u16 {
    fn lo(self) -> u8 {
        (self & 0xFF) as u8
    }

    fn hi(self) -> u8 {
        ((self >> 8) & 0xFF) as u8
    }
}

impl LoHi<u16> for u32 {
    fn lo(self) -> u16 {
        (self & 0xFFFF) as u16
    }

    fn hi(self) -> u16 {
        ((self >> 16) & 0xFFFF) as u16
    }
}

#[repr(u8)]
enum Ogf {
    HostControl = 0x03,
    LeControl = 0x08,
    Vendor = 0x3f,
}

#[repr(u8)]
enum Command {
    SetBdAddr = 0x01,
    ResetChip = 0x03,
    AdvertData = 0x0008,
    SetAdvertParameters = 0x0006, // LE Set Advertising Parameters command OCF
    SetAdvertEnable = 0x000a,     // LE Set Advertising Enable command OCF
    SetScanEnable = 0x000c,       // LE Set Scan Enable command OCF
    SetScanParameters = 0x000b,   // LE Set Scan Parameters command OCF
    CreateConnection = 0x0d,      // LE Create Connection command OCF
    SetBaud = 0x18,
    LoadFirmware = 0x2e,
}

#[repr(u8)]
enum HciPacketType {
    Command = 0x01,
    Acl = 0x02,
    Event = 0x04,
}

#[repr(u8)]
pub enum HciEventCode {
    CommandComplete = 0x0e,
    ConnectComplete = 0x0f,
}

#[repr(u8)]
enum LeScanType {
    Active = 0x01,
}

#[repr(u8)]
enum LeAdvType {
    NonConnInd = 0x03,
}

impl Ogf {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x03 => Some(Ogf::HostControl),
            0x08 => Some(Ogf::LeControl),
            0x3f => Some(Ogf::Vendor),
            _ => None,
        }
    }
}
impl Command {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Command::SetBdAddr),
            0x03 => Some(Command::ResetChip),
            0x18 => Some(Command::SetBaud),
            0x2e => Some(Command::LoadFirmware),
            _ => None,
        }
    }
}
impl HciPacketType {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(HciPacketType::Command),
            0x02 => Some(HciPacketType::Acl),
            0x04 => Some(HciPacketType::Event),
            _ => None,
        }
    }
}
impl HciEventCode {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x0e => Some(HciEventCode::CommandComplete),
            0x0f => Some(HciEventCode::ConnectComplete),
            _ => None,
        }
    }
}
impl LeScanType {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(LeScanType::Active),
            _ => None,
        }
    }
}
impl LeAdvType {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x03 => Some(LeAdvType::NonConnInd),
            _ => None,
        }
    }
}

pub struct Bluetooth {
    uart0_dr: Address,
    uart0_fr: Address,
    uart0_ibrd: Address,
    uart0_fbrd: Address,
    uart0_lcrh: Address,
    uart0_cr: Address,
    uart0_ifls: Address,
    uart0_imsc: Address,
    uart0_ris: Address,
    uart0_mis: Address,
    uart0_icr: Address,
}
pub struct BluetoothStatic(UnsafeCell<Bluetooth>);

unsafe impl Sync for BluetoothStatic {}

unsafe impl Sync for Bluetooth {}

pub static BLUETOOTH: BluetoothStatic = BluetoothStatic(UnsafeCell::new(Bluetooth::new()));

impl BluetoothStatic {
    pub fn get(&self) -> &mut Bluetooth {
        unsafe { &mut *self.0.get() }
    }
}

impl Bluetooth {
    pub const fn new() -> Self {
        Bluetooth {
            uart0_dr: Address(0),
            uart0_fr: Address(0),
            uart0_ibrd: Address(0),
            uart0_fbrd: Address(0),
            uart0_lcrh: Address(0),
            uart0_cr: Address(0),
            uart0_ifls: Address(0),
            uart0_imsc: Address(0),
            uart0_ris: Address(0),
            uart0_mis: Address(0),
            uart0_icr: Address(0),
        }
    }

    pub fn init(&mut self, mmio_entry: Option<&AliasEntry>) {
        let base_addr = mmio_entry
            .unwrap_or_else(|| panic!("UART0 MMIO entry not found in device tree"))
            .arm_phys;

        self.uart0_dr.set_address(base_addr);
        self.uart0_fr = self.uart0_dr.offset(0x18);
        self.uart0_ibrd = self.uart0_dr.offset(0x24);
        self.uart0_fbrd = self.uart0_dr.offset(0x28);
        self.uart0_lcrh = self.uart0_dr.offset(0x2C);
        self.uart0_cr = self.uart0_dr.offset(0x30);
        self.uart0_ifls = self.uart0_dr.offset(0x34);
        self.uart0_imsc = self.uart0_dr.offset(0x38);
        self.uart0_ris = self.uart0_dr.offset(0x3C);
        self.uart0_mis = self.uart0_dr.offset(0x40);
        self.uart0_icr = self.uart0_dr.offset(0x44);

        GpioPin::new(30).set_function(GpioFunction::Alt3);
        GpioPin::new(31).set_function(GpioFunction::Alt3);
        GpioPin::new(32).set_function(GpioFunction::Alt3);
        GpioPin::new(33).set_function(GpioFunction::Alt3);

        self.flush_rx();

        self.uart0_imsc.write(0x0);
        self.uart0_icr.write(0x7FF);
        self.uart0_ibrd.write(0x1A);
        self.uart0_fbrd.write(0x03);
        self.uart0_ifls.write(0x08);
        self.uart0_lcrh.write(0x70); // 8 data bits,
        self.uart0_cr.write(0xB01);
        self.uart0_imsc.write(0x430);

        time_manager().spin_for(Duration::from_millis(100));
    }

    pub fn is_read_byte_ready(&self) -> bool {
        self.uart0_fr.read() & 0x10 == 0
    }

    pub fn is_write_byte_ready(&self) -> bool {
        self.uart0_fr.read() & 0x20 == 0
    }

    pub fn read_byte(&self) -> u8 {
        self.uart0_dr.read().lo().lo()
    }

    fn wait_read_byte(&self) -> u8 {
        while !self.is_read_byte_ready() {}

        self.read_byte()
    }

    fn write_byte(&self, val: u8) {
        self.uart0_dr.write(val as u32);
    }

    pub fn wait_write_byte(&self, val: u8) {
        while !self.is_write_byte_ready() {}

        self.write_byte(val);
    }

    fn flush_rx(&self) {
        while self.is_read_byte_ready() {
            self.read_byte();
        }
    }

    fn hci_command_bytes(&self, opcodebytes: &[u8; 2], data: &[u8]) -> u8 {
        let mut c = 0;

        self.wait_write_byte(HciPacketType::Command.as_u8());
        self.wait_write_byte(opcodebytes[0]);
        self.wait_write_byte(opcodebytes[1]);
        self.wait_write_byte(data.len() as u8);

        while c < data.len() {
            self.wait_write_byte(data[c]);
            c += 1;
        }

        if self.wait_read_byte() != HciPacketType::Event.as_u8() {
            return 1;
        }

        let code = self.wait_read_byte();
        if code == HciEventCode::ConnectComplete.as_u8() {
            if self.wait_read_byte() != 4 {
                return 2;
            }
            let err = self.wait_read_byte();
            if err != 0 {
                uart_println!("Saw HCI COMMAND STATUS with error code: {:x}", err);
                return 12;
            }

            if self.wait_read_byte() == 0 {
                return 3;
            }
            if self.wait_read_byte() != opcodebytes[0] {
                return 4;
            }
            if self.wait_read_byte() != opcodebytes[1] {
                return 5;
            }
        } else if code == HciEventCode::CommandComplete.as_u8() {
            if self.wait_read_byte() != 4 {
                return 6;
            }
            if self.wait_read_byte() == 0 {
                return 7;
            }
            if self.wait_read_byte() != opcodebytes[0] {
                return 8;
            }
            if self.wait_read_byte() != opcodebytes[1] {
                return 9;
            }
            if self.wait_read_byte() != 0 {
                return 10;
            }
        } else {
            return 11;
        }

        0
    }

    fn hci_command(&self, ogf: Ogf, ocf: Command, data: &[u8]) -> u8 {
        let opcode = ((ogf.as_u8() as u16) << 10) | ocf.as_u8() as u16;
        let opcodebytes = [opcode.lo(), opcode.hi()];
        self.hci_command_bytes(&opcodebytes, data)
    }

    pub fn reset(&self) {
        if self.hci_command(Ogf::HostControl, Command::ResetChip, &[]) != 0 {
            uart_println!("Failed to reset Bluetooth chip");
        }
    }

    pub fn load_firmware(&self) {
        if self.hci_command(Ogf::Vendor, Command::LoadFirmware, &[]) != 0 {
            uart_println!("Failed to load Bluetooth firmware");
        }

        let mut c: usize = 0;
        let size = FIRMWARE.len();
        while c < size {
            // Defensive: check at least 3 bytes remain
            if c + 3 > size {
                uart_println!("Firmware load: unexpected EOF");
                break;
            }
            let opcodebytes = [FIRMWARE[c], FIRMWARE[c + 1]];
            let length = FIRMWARE[c + 2] as usize;
            c += 3;

            // Defensive: check if enough bytes remain for data
            if c + length > size {
                uart_println!("Firmware load: data segment truncated");
                break;
            }
            let data = &FIRMWARE[c..c + length];

            if self.hci_command_bytes(&opcodebytes, data) != 0 {
                uart_println!("Firmware data load failed");
                break;
            }

            c += length;
        }
        time_manager().spin_for(Duration::from_millis(100));
        uart_println!("Bluetooth firmware loaded successfully");
    }

    pub fn set_baud(&self) {
        let data: [u8; 6] = [0x00, 0x00, 0x00, 0xc2, 0x01, 0x00]; // little endian, 115200
        if self.hci_command(Ogf::Vendor, Command::SetBaud, &data) != 0 {
            uart_println!("Failed to set Bluetooth baud rate");
        }
    }

    pub fn set_bdaddr(&self) {
        let data: [u8; 6] = [0xee, 0xff, 0xc0, 0xee, 0xff, 0xc0]; // reversed
        if self.hci_command(Ogf::HostControl, Command::SetBdAddr, &data) != 0 {
            uart_println!("Failed to set Bluetooth BD_ADDR");
        }
    }

    pub fn get_bdaddr(&self) -> [u8; 6] {
        self.write_byte(HciPacketType::Command.as_u8());
        self.write_byte(0x09);
        self.write_byte(0x10);
        self.write_byte(0x00);

        if self.read_byte() != HciPacketType::Event.as_u8() {
            return [0; 6];
        }
        if self.read_byte() != HciEventCode::CommandComplete.as_u8() {
            return [0; 6];
        }
        if self.read_byte() != 0x0a {
            return [0; 6];
        }
        if self.read_byte() != 0x01 {
            return [0; 6];
        }
        if self.read_byte() != 0x09 {
            return [0; 6];
        }
        if self.read_byte() != 0x10 {
            return [0; 6];
        }
        if self.read_byte() != 0x00 {
            return [0; 6];
        }

        let mut bdaddr = [0; 6];
        for i in 0..6 {
            bdaddr[i] = self.read_byte();
        }
        uart_println!(
            "Bluetooth BD_ADDR: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            bdaddr[5],
            bdaddr[4],
            bdaddr[3],
            bdaddr[2],
            bdaddr[1],
            bdaddr[0]
        );
        bdaddr
    }

    pub fn send_acl_subscirbe(&self, handle: u32) {
        self.write_byte(HciPacketType::Acl.as_u8());

        self.write_byte(handle.lo().lo());
        self.write_byte(handle.lo().hi());

        let len: u32 = 0x0009;
        self.write_byte(len.lo().lo());
        self.write_byte(len.lo().hi());

        let data_len: u32 = 0x0005;
        self.write_byte(data_len.lo().lo());
        self.write_byte(data_len.lo().hi());

        let channel: u32 = 0x0004;
        self.write_byte(channel.lo().lo());
        self.write_byte(channel.lo().hi());

        let command: [u8; 5] = [0x12, 0x2b, 0x00, 0x01, 0x00];

        for &byte in command.iter().take(data_len as usize) {
            self.write_byte(byte);
        }

        uart_println!("Sent ACL subscribe command with handle: {:x}", handle);
    }

    pub fn set_le_event_mask(&self, mask: u8) {
        let data: [u8; 8] = [
            mask, 0x00, 0x00, 0x00, // LE event mask
            0x00, 0x00, 0x00, 0x00, // reserved
        ];
        if self.hci_command(
            Ogf::LeControl,
            Command::SetBdAddr, // Not the correct command, but it is the same number internally. Set Event Mask command
            &data,
        ) != 0
        {
            uart_println!("Failed to set LE event mask");
        }
    }

    fn set_le_scan_enable(&self, state: u8, duplicate: u8) {
        let data: [u8; 2] = [state, duplicate];
        if self.hci_command(
            Ogf::LeControl,
            Command::SetScanEnable, // LE Set Scan Enable command
            &data,
        ) != 0
        {
            uart_println!("Failed to set LE scan enable");
        }
    }

    fn set_le_scan_parameters(
        &self,
        scan_type: LeScanType,
        interval: u16,
        window: u16,
        own_address_type: u8,
        filter_policy: u8,
    ) {
        let data: [u8; 7] = [
            scan_type.as_u8(), // Scan type
            interval.lo(),
            interval.hi(), // Scan interval
            window.lo(),
            window.hi(),      // Scan window
            own_address_type, // Own address type
            filter_policy,    // Filter policy
        ];
        if self.hci_command(
            Ogf::LeControl,
            Command::SetScanParameters, // LE Set Scan Parameters command
            &data,
        ) != 0
        {
            uart_println!("Failed to set LE scan parameters");
        }
    }

    pub fn set_le_advertenable(&self, state: u8) {
        let data: [u8; 1] = [state];
        if self.hci_command(
            Ogf::LeControl,
            Command::SetAdvertEnable, // LE Set Advertising Enable command
            &data,
        ) != 0
        {
            uart_println!("Failed to set LE advertising enable");
        }
    }

    fn set_le_advert_parameters(
        &self,
        adv_type: LeAdvType,
        interval_min: u16,
        interval_max: u16,
        own_address_type: u8,
        filter_policy: u8,
    ) {
        let mut data: [u8; 15] = [0; 15];
        data[0] = adv_type.as_u8(); // Advertising type
        data[1] = interval_min.lo(); // Advertising interval min (low byte)
        data[2] = interval_min.hi(); // Advertising interval min (high byte)
        data[3] = interval_max.lo(); // Advertising interval max (low byte)
        data[4] = interval_max.hi(); // Advertising interval max (high byte)
        data[5] = own_address_type; // Own address type
        data[13] = 0x07;
        data[14] = filter_policy; // Filter policy

        if self.hci_command(
            Ogf::LeControl,
            Command::SetAdvertParameters, // LE Set Advertising Parameters command
            &data,
        ) != 0
        {
            uart_println!("Failed to set LE advertising parameters");
        }
    }

    fn set_le_advert_data(&self) {
        let data: [u8; 32] = [
            0x19, 0x02, 0x01, 0x06, 0x03, 0x03, 0xAA, 0xFE, 0x11, 0x16, 0xAA, 0xFE, 0x10, 0x00,
            0x03, 0x69, 0x73, 0x6f, 0x6d, 0x65, 0x74, 0x69, 0x6d, 0x2e, 0x65, 0x73, 0, 0, 0, 0, 0,
            0, // End of data
               // Padding to fill the rest of the data length
        ];
        if self.hci_command(
            Ogf::LeControl,
            Command::AdvertData, // LE Set Advertising Data command
            &data,
        ) != 0
        {
            uart_println!("Failed to set LE advertising data");
        }
    }

    pub fn stop_scaning(&self) {
        self.set_le_scan_enable(0, 0);
        uart_println!("Bluetooth scanning stopped");
    }

    pub fn start_active_scaning(&self) {
        let ble_scan_interval: f32 = 60.0; // every 60ms
        let ble_scan_window: f32 = 60.0;
        let ble_scan_divisor: f32 = 0.625;

        let p = (ble_scan_interval / ble_scan_divisor) as u16;
        let q = (ble_scan_window / ble_scan_divisor) as u16;

        self.set_le_scan_parameters(LeScanType::Active, p, q, 0, 0);
        self.set_le_scan_enable(1, 0);
        uart_println!("Bluetooth scanning started");
    }

    pub fn start_active_advertizing(&self) {
        let advert_min_freq: f32 = 100.0; // every 60ms
        let advert_max_freq: f32 = 100.0;
        let ble_granularity: f32 = 0.625;

        let min_interval = (advert_min_freq / ble_granularity) as u16;
        let max_interval = (advert_max_freq / ble_granularity) as u16;

        self.set_le_advert_parameters(
            LeAdvType::NonConnInd,
            min_interval,
            max_interval,
            0, // Own address type
            0, // Filter policy
        );
        self.set_le_advert_data();
        self.set_le_advertenable(1);
        uart_println!("Bluetooth advertising started");
    }

    pub fn connect(&self, addr: [u8; 6]) {
        let ble_scan_interval: f32 = 60.0; // every 60ms
        let ble_scan_window: f32 = 60.0;
        let ble_scan_divisor: f32 = 0.625;
        let advert_min_freq: f32 = 100.0; // every 60ms
        let advert_max_freq: f32 = 100.0;
        let ble_granularity: f32 = 0.625;

        let min_interval = (advert_min_freq / ble_granularity) as u16;
        let max_interval = (advert_max_freq / ble_granularity) as u16;
        let p = (ble_scan_interval / ble_scan_divisor) as u16;
        let q = (ble_scan_window / ble_scan_divisor) as u16;

        let mut command = [0; 25];
        command[0] = p.lo();
        command[1] = q.lo();
        command[6] = addr[5];
        command[7] = addr[4];
        command[8] = addr[3];
        command[9] = addr[2];
        command[10] = addr[1];
        command[11] = addr[0];
        command[13] = min_interval.lo();
        command[15] = max_interval.lo();
        command[19] = 0x2a;
        command[20] = 0x00; // Own address type

        if self.hci_command(
            Ogf::LeControl,
            Command::CreateConnection, // LE Create Connection command OCF
            &command,
        ) != 0
        {
            uart_println!("Failed to initiate Bluetooth connection");
            return;
        }

        uart_println!("Bluetooth connection initiated");
    }
}

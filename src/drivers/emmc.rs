//! eMMC2 (BCM2711) driver - PIO read/write single block.
//!
//! - Register map is enum-style (no duplicates).
//! - Uses polling via INTERRUPT (W1C).
//! - Init sequence: CMD0, CMD1 loop, CMD2, CMD3(host assigns RCA), CMD7, CMD16, raise clock.
//! - Read: CMD17 + READ_RDY + DATA_DONE.
//! - Write: CMD24 + WRITE_RDY + DATA_DONE + DAT_INHIBIT release.
//!
//! Notes:
//! - This driver assumes sector (LBA) addressing (works on your setup as proven by reads).
//! - No external crates.
//! - Debug helpers are kept (can gate later).

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::time::Duration;

use super::address::Address;
use crate::{info, warn};

pub const EMMC_BLOCK_SIZE: usize = 512;

//--------------------------------------------------------------------------------------------------
// Register map (enum-style, NO duplicates)
//--------------------------------------------------------------------------------------------------
#[repr(usize)]
#[derive(Copy, Clone, Debug)]
pub enum EmmcReg {
    Arg2 = 0x00,
    BlkSizeCnt = 0x04,
    Arg1 = 0x08,
    CmdTm = 0x0C,
    Resp0 = 0x10,
    Resp1 = 0x14,
    Resp2 = 0x18,
    Resp3 = 0x1C,
    Data = 0x20,
    Status = 0x24,
    Control0 = 0x28,
    Control1 = 0x2C,
    Interrupt = 0x30,
    IrptMask = 0x34,
    IrptEn = 0x38,
    Control2 = 0x3C,
}

impl EmmcReg {
    #[inline(always)]
    pub const fn off(self) -> usize {
        self as usize
    }
}

//--------------------------------------------------------------------------------------------------
// Commands / responses / flags (enum-style)
//--------------------------------------------------------------------------------------------------
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum EmmcCommand {
    GoIdleState = 0,       // CMD0
    SendOpCond = 1,        // CMD1 (eMMC/MMC)
    AllSendCid = 2,        // CMD2
    SendRelativeAddr = 3,  // CMD3 (MMC: host assigns RCA)
    Switch = 6,            // CMD6 (MMC SWITCH)
    SelectCard = 7,        // CMD7
    SendExtCsd = 8,        // CMD8 (MMC SEND_EXT_CSD)
    StopTransmission = 12, // CMD12
    SendStatus = 13,       // CMD13
    SetBlockLen = 16,      // CMD16
    ReadSingleBlock = 17,  // CMD17
    WriteSingleBlock = 24, // CMD24
}

#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum CmdRespType {
    None = 0b00,
    R136 = 0b01,
    R48 = 0b10,
    R48Busy = 0b11,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum CmdFlag {
    IsData = 1 << 21,
    IxChkEn = 1 << 20,
    CrcChkEn = 1 << 19,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum XferFlag {
    // CMDTM transfer-mode low bits (TM_*).
    BlkCntEn = 1 << 1,    // TM_BLKCNT_EN
    AutoCmd12 = 1 << 2,   // TM_AUTO_CMD_EN (unused here)
    DataDirRead = 1 << 4, // TM_DAT_DIR (1 = card->host)
    MultiBlock = 1 << 5,  // TM_MULTI_BLOCK (0 for CMD17/CMD24)
}

#[inline(always)]
fn cmdtm_encode(cmd: EmmcCommand, resp: CmdRespType, flags: u32) -> u32 {
    // CMD_INDEX is bits 29:24; resp type bits 17:16.
    ((cmd as u32) << 24) | ((resp as u32) << 16) | flags
}

//--------------------------------------------------------------------------------------------------
// STATUS bits
//--------------------------------------------------------------------------------------------------
const STATUS_CMD_INHIBIT: u32 = 1 << 0;
const STATUS_DAT_INHIBIT: u32 = 1 << 1;

// MMC R1 card-status bits.
const R1_ERROR_MASK: u32 = 0xFFF9_A000 | (1 << 7);
const R1_READY_FOR_DATA: u32 = 1 << 8;
const R1_CURRENT_STATE_MASK: u32 = 0xF << 9;
const R1_CURRENT_STATE_SHIFT: u32 = 9;
const R1_STATE_TRAN: u32 = 4;

// eMMC EXT_CSD fields used for durable write barriers.
const EXT_CSD_FLUSH_CACHE: u8 = 32;
const EXT_CSD_CACHE_CTRL: usize = 33;
const EXT_CSD_REV: usize = 192;
const EXT_CSD_CACHE_SIZE: usize = 249;
const MMC_SWITCH_MODE_WRITE_BYTE: u32 = 0x03;
const EXT_CSD_CMD_SET_NORMAL: u32 = 1;

//--------------------------------------------------------------------------------------------------
// INTERRUPT bits (W1C)
//--------------------------------------------------------------------------------------------------
const IRQ_CMD_DONE: u32 = 1 << 0;
const IRQ_DATA_DONE: u32 = 1 << 1;
const IRQ_WRITE_RDY: u32 = 1 << 4;
const IRQ_READ_RDY: u32 = 1 << 5;
const IRQ_ERR: u32 = 1 << 15;

//--------------------------------------------------------------------------------------------------
// CONTROL1 clock/reset bits
//--------------------------------------------------------------------------------------------------
const C1_CLK_INT_EN: u32 = 1 << 0;
const C1_CLK_STABLE: u32 = 1 << 1;
const C1_CLK_EN: u32 = 1 << 2;

const C1_SRST_HC: u32 = 1 << 24;
const C1_SRST_CMD: u32 = 1 << 25;
const C1_SRST_DAT: u32 = 1 << 26;

// CONTROL1 divider field mask/encode (10-bit split)
const C1_DIV_MASK: u32 = (0xFF << 8) | (0x3 << 6);

#[inline(always)]
fn c1_div_encode(div10: u16) -> u32 {
    let lo = ((div10 as u32) & 0x00FF) << 8;
    let hi = ((((div10 as u32) & 0x0300) >> 8) & 0x3) << 6;
    lo | hi
}

//--------------------------------------------------------------------------------------------------
// Errors
//--------------------------------------------------------------------------------------------------
#[derive(Debug)]
pub enum EmmcError {
    NotInitialized,
    Timeout(&'static str),
    IrqError(u32),
    InhibitStuck(u32),
    CardStatus(u32),
}

//--------------------------------------------------------------------------------------------------
// Driver
//--------------------------------------------------------------------------------------------------
pub struct Emmc {
    initialized: bool,
    rca: u16,
    base: Address,
    ext_csd_rev: u8,
    cache_size_kib: u32,
    cache_enabled: bool,
}

impl Emmc {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            rca: 0,
            base: Address(0),
            ext_csd_rev: 0,
            cache_size_kib: 0,
            cache_enabled: false,
        }
    }

    #[inline(always)]
    fn reg(&self, r: EmmcReg) -> Address {
        self.base.offset(r.off())
    }

    #[inline(always)]
    fn barrier_io() {
        // Peripheral boundary barriers.
        unsafe { core::arch::asm!("dsb sy", "isb", options(nostack, nomem)) }
    }

    fn wait_while<F: Fn() -> bool>(
        cond: F,
        mut limit: u32,
        what: &'static str,
    ) -> Result<(), EmmcError> {
        while cond() {
            limit = limit.wrapping_sub(1);
            if limit == 0 {
                return Err(EmmcError::Timeout(what));
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    fn clear_interrupts(&self, mask: u32) {
        // INTERRUPT is W1C.
        self.reg(EmmcReg::Interrupt).write(mask);
    }

    fn clear_all_interrupts(&self) {
        self.clear_interrupts(0xFFFF_FFFF);
    }

    // Split inhibit waits so R1b/busy handling is correct.
    fn wait_cmd_inhibit_clear(&self) -> Result<(), EmmcError> {
        let st = self.reg(EmmcReg::Status);
        let stc = st;
        Self::wait_while(
            || (stc.read() & STATUS_CMD_INHIBIT) != 0,
            2_000_000,
            "CMD_INHIBIT",
        )?;
        Ok(())
    }

    fn wait_dat_inhibit_clear(&self) -> Result<(), EmmcError> {
        let st = self.reg(EmmcReg::Status);
        let stc = st;
        Self::wait_while(
            || (stc.read() & STATUS_DAT_INHIBIT) != 0,
            2_000_000,
            "DAT_INHIBIT",
        )?;
        Ok(())
    }

    fn reset_host(&self) -> Result<(), EmmcError> {
        let c1 = self.reg(EmmcReg::Control1);

        // Full reset
        c1.write(c1.read() | C1_SRST_HC);
        let c1c = c1;
        Self::wait_while(|| (c1c.read() & C1_SRST_HC) != 0, 2_000_000, "SRST_HC")?;

        // CMD/DAT reset
        c1.write(c1.read() | (C1_SRST_CMD | C1_SRST_DAT));
        let c1c2 = c1;
        Self::wait_while(
            || (c1c2.read() & (C1_SRST_CMD | C1_SRST_DAT)) != 0,
            2_000_000,
            "SRST_CMD/DAT",
        )?;

        Ok(())
    }

    fn set_clock_divided(&self, div10: u16) -> Result<(), EmmcError> {
        let c1 = self.reg(EmmcReg::Control1);

        // disable SD clock + internal clock
        let mut v = c1.read();
        v &= !C1_CLK_EN;
        v &= !C1_CLK_INT_EN;
        c1.write(v);

        // program divider
        v &= !C1_DIV_MASK;
        v |= c1_div_encode(div10);

        // set timeout exponent (keep what we had: 0xE)
        v |= 0xE << 16;

        c1.write(v);

        // enable internal clock
        v |= C1_CLK_INT_EN;
        c1.write(v);

        // wait stable
        let c1c = c1;
        Self::wait_while(
            || (c1c.read() & C1_CLK_STABLE) == 0,
            2_000_000,
            "CLK_STABLE",
        )?;

        // enable SD clock
        c1.write(c1.read() | C1_CLK_EN);

        Ok(())
    }

    fn enable_bus_power_3v3(&self) {
        // SDHCI-style: bit8 = power on, bits 11:9 = voltage select (0b111 commonly 3.3V)
        let c0 = self.reg(EmmcReg::Control0);
        let mut v = c0.read();

        v |= 1 << 8;
        v &= !(0x7 << 9);
        v |= 0x7 << 9;

        c0.write(v);
        crate::time::time_manager().spin_for(Duration::from_millis(2));
    }

    // send_cmd: deterministic wait for CMD_DONE/ERR; proper R1b busy handling.
    fn send_cmd(
        &self,
        cmd: EmmcCommand,
        arg: u32,
        resp: CmdRespType,
        flags: u32,
    ) -> Result<(), EmmcError> {
        // Always require CMD line free before issuing
        self.wait_cmd_inhibit_clear()
            .map_err(|_| EmmcError::InhibitStuck(self.reg(EmmcReg::Status).read()))?;

        // If this is a data command, also require DAT free before issuing
        if (flags & (CmdFlag::IsData as u32)) != 0 {
            self.wait_dat_inhibit_clear()
                .map_err(|_| EmmcError::InhibitStuck(self.reg(EmmcReg::Status).read()))?;
        }

        self.clear_all_interrupts();

        self.reg(EmmcReg::Arg1).write(arg);
        self.reg(EmmcReg::CmdTm)
            .write(cmdtm_encode(cmd, resp, flags));
        Self::barrier_io();

        let intr = self.reg(EmmcReg::Interrupt);
        let intrc = intr;

        Self::wait_while(
            || (intrc.read() & (IRQ_CMD_DONE | IRQ_ERR)) == 0,
            4_000_000,
            "CMD_DONE",
        )?;

        let v = intr.read();

        // Ack what we saw
        self.clear_interrupts(v);

        // Error?
        if (v & IRQ_ERR) != 0 || (v & 0xFFFF_0000) != 0 {
            return Err(EmmcError::IrqError(v));
        }

        // R1b: wait for DAT line to become free (busy release)
        if matches!(resp, CmdRespType::R48Busy) {
            self.wait_dat_inhibit_clear()
                .map_err(|_| EmmcError::InhibitStuck(self.reg(EmmcReg::Status).read()))?;
        }

        Ok(())
    }

    fn read_ext_csd(&self, out: &mut [u8; EMMC_BLOCK_SIZE]) -> Result<(), EmmcError> {
        self.wait_cmd_inhibit_clear()
            .map_err(|_| EmmcError::InhibitStuck(self.reg(EmmcReg::Status).read()))?;
        self.wait_dat_inhibit_clear()
            .map_err(|_| EmmcError::InhibitStuck(self.reg(EmmcReg::Status).read()))?;

        self.reg(EmmcReg::BlkSizeCnt)
            .write(((1u32) << 16) | (EMMC_BLOCK_SIZE as u32));
        self.clear_all_interrupts();
        self.reg(EmmcReg::Arg1).write(0);

        let cmdtm = cmdtm_encode(
            EmmcCommand::SendExtCsd,
            CmdRespType::R48,
            (CmdFlag::IsData as u32)
                | (CmdFlag::CrcChkEn as u32)
                | (CmdFlag::IxChkEn as u32)
                | (XferFlag::BlkCntEn as u32)
                | (XferFlag::DataDirRead as u32),
        );
        self.reg(EmmcReg::CmdTm).write(cmdtm);
        Self::barrier_io();

        let intr = self.reg(EmmcReg::Interrupt);
        let intrc = intr;
        Self::wait_while(
            || (intrc.read() & (IRQ_READ_RDY | IRQ_ERR)) == 0,
            4_000_000,
            "EXT_CSD_READ_RDY",
        )?;

        let v = intr.read();
        if (v & IRQ_ERR) != 0 || (v & 0xFFFF_0000) != 0 {
            self.clear_interrupts(v);
            return Err(EmmcError::IrqError(v));
        }
        self.clear_interrupts(v);

        for i in 0..128 {
            let word = self.reg(EmmcReg::Data).read();
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }

        let intrc2 = intr;
        Self::wait_while(
            || (intrc2.read() & (IRQ_DATA_DONE | IRQ_ERR)) == 0,
            4_000_000,
            "EXT_CSD_DATA_DONE",
        )?;

        let v2 = intr.read();
        if (v2 & IRQ_ERR) != 0 || (v2 & 0xFFFF_0000) != 0 {
            self.clear_interrupts(v2);
            return Err(EmmcError::IrqError(v2));
        }
        self.clear_interrupts(v2);
        Ok(())
    }

    fn card_status(&self) -> Result<u32, EmmcError> {
        self.send_cmd(
            EmmcCommand::SendStatus,
            (self.rca as u32) << 16,
            CmdRespType::R48,
            (CmdFlag::CrcChkEn as u32) | (CmdFlag::IxChkEn as u32),
        )?;
        Ok(self.reg(EmmcReg::Resp0).read())
    }

    fn wait_ready_for_data(&self) -> Result<u32, EmmcError> {
        for _ in 0..10_000 {
            let status = self.card_status()?;
            if (status & R1_ERROR_MASK) != 0 {
                return Err(EmmcError::CardStatus(status));
            }

            let state = (status & R1_CURRENT_STATE_MASK) >> R1_CURRENT_STATE_SHIFT;
            if (status & R1_READY_FOR_DATA) != 0 && state == R1_STATE_TRAN {
                return Ok(status);
            }

            crate::time::time_manager().spin_for(Duration::from_millis(1));
        }

        Err(EmmcError::Timeout("READY_FOR_DATA"))
    }

    fn switch_ext_csd_byte(&self, index: u8, value: u8) -> Result<(), EmmcError> {
        let arg = (MMC_SWITCH_MODE_WRITE_BYTE << 24)
            | ((index as u32) << 16)
            | ((value as u32) << 8)
            | EXT_CSD_CMD_SET_NORMAL;

        self.send_cmd(
            EmmcCommand::Switch,
            arg,
            CmdRespType::R48Busy,
            (CmdFlag::CrcChkEn as u32) | (CmdFlag::IxChkEn as u32),
        )?;

        let status = self.reg(EmmcReg::Resp0).read();
        if (status & R1_ERROR_MASK) != 0 {
            return Err(EmmcError::CardStatus(status));
        }
        Ok(())
    }

    /// Make all completed writes durable before a filesystem integrity boundary.
    pub fn flush(&self) -> Result<(), EmmcError> {
        if !self.initialized {
            return Err(EmmcError::NotInitialized);
        }

        // First ensure any outstanding programming operation has completed.
        self.wait_ready_for_data()?;

        // eMMC devices with CACHE_CTRL enabled require FLUSH_CACHE to push the
        // volatile device cache to non-volatile storage.
        if self.cache_enabled && self.cache_size_kib != 0 {
            self.switch_ext_csd_byte(EXT_CSD_FLUSH_CACHE, 1)?;
        }

        // CMD6 uses R1b, but finish with an explicit card-state check as the
        // durability boundary consumed by the block/filesystem layers.
        self.wait_ready_for_data()?;
        Self::barrier_io();
        Ok(())
    }

    //----------------------------------------------------------------------------------------------
    // Public init
    //----------------------------------------------------------------------------------------------
    pub fn init(&mut self, mmio_entry: Option<crate::fdt::AliasEntry>) {
        let base_addr = mmio_entry
            .unwrap_or_else(|| panic!("EMMC MMIO entry not found in device tree"))
            .arm_phys;

        self.base.set_address(base_addr);

        // Debug helpers
        let dump_regs = |tag: &str, this: &Emmc| {
            let c0 = this.reg(EmmcReg::Control0).read();
            let c1 = this.reg(EmmcReg::Control1).read();
            let st = this.reg(EmmcReg::Status).read();
            let ir = this.reg(EmmcReg::Interrupt).read();
            let mk = this.reg(EmmcReg::IrptMask).read();
            let en = this.reg(EmmcReg::IrptEn).read();
            let r0 = this.reg(EmmcReg::Resp0).read();
            info!(
                "[{}] CONTROL0=0x{:08x} CONTROL1=0x{:08x} STATUS=0x{:08x} INTERRUPT=0x{:08x} IRPT_MASK=0x{:08x} IRPT_EN=0x{:08x} RESP0=0x{:08x}",
                tag, c0, c1, st, ir, mk, en, r0
            );
        };

        let dump_irq = |tag: &str, v: u32| {
            if v == 0 {
                info!("[{}] IRQ=0x00000000", tag);
                return;
            }
            info!("[{}] IRQ=0x{:08x}", tag, v);

            if (v & IRQ_CMD_DONE) != 0 {
                info!("  - CMD_DONE");
            }
            if (v & IRQ_DATA_DONE) != 0 {
                info!("  - DATA_DONE");
            }
            if (v & IRQ_WRITE_RDY) != 0 {
                info!("  - WRITE_RDY");
            }
            if (v & IRQ_READ_RDY) != 0 {
                info!("  - READ_RDY");
            }
            if (v & IRQ_ERR) != 0 {
                info!("  - ERR (summary)");
            }

            if (v & (1 << 16)) != 0 {
                info!("  - CTO_ERR (command timeout)");
            }
            if (v & (1 << 17)) != 0 {
                info!("  - CCRC_ERR (command CRC error)");
            }
            if (v & (1 << 18)) != 0 {
                info!("  - CEND_ERR (command end bit error)");
            }
            if (v & (1 << 19)) != 0 {
                info!("  - CBAD_ERR (command index error)");
            }
            if (v & (1 << 20)) != 0 {
                info!("  - DTO_ERR (data timeout)");
            }
            if (v & (1 << 21)) != 0 {
                info!("  - DCRC_ERR (data CRC error)");
            }
            if (v & (1 << 22)) != 0 {
                info!("  - DEND_ERR (data end bit error)");
            }
            if (v & (1 << 24)) != 0 {
                info!("  - ACMD_ERR (Auto CMD error)");
            }
        };

        // 1) Reset host controller
        if let Err(e) = self.reset_host() {
            warn!("reset_host failed: {:?}", e);
            dump_regs("RESET_FAIL", self);
            return;
        }

        // 2) Interrupt setup for polling
        const IRPT_MASK_KNOWN: u32 = 0x017f_7137;
        self.reg(EmmcReg::IrptMask).write(IRPT_MASK_KNOWN);
        self.reg(EmmcReg::IrptEn).write(0);
        self.clear_all_interrupts();

        // 3) Identification clock
        if let Err(e) = self.set_clock_divided(250) {
            warn!("set_clock_divided(250) failed: {:?}", e);
            dump_regs("CLOCK_IDENT_FAIL", self);
            return;
        }

        // 4) Bus power
        self.enable_bus_power_3v3();

        // 5) CMD0
        if let Err(e) = self.send_cmd(EmmcCommand::GoIdleState, 0, CmdRespType::None, 0) {
            match e {
                EmmcError::IrqError(v) => {
                    warn!("CMD0 failed: IrqError(0x{:08x})", v);
                    dump_irq("CMD0_IRQ", v);
                }
                _ => warn!("CMD0 failed: {:?}", e),
            }
            dump_regs("CMD0_FAIL", self);
            return;
        }

        // 6) CMD1 loop (OCR)
        let mut ocr: u32 = 0;
        let mut ready = false;
        let cmd1_arg: u32 = 0x40FF_8000;

        for attempt in 0..1000u32 {
            let r = self.send_cmd(EmmcCommand::SendOpCond, cmd1_arg, CmdRespType::R48, 0);
            if let Err(e) = r {
                match e {
                    EmmcError::IrqError(v) => {
                        warn!("CMD1 failed on attempt {}: IrqError(0x{:08x})", attempt, v);
                        dump_irq("CMD1_IRQ", v);
                    }
                    _ => warn!("CMD1 failed on attempt {}: {:?}", attempt, e),
                }
                dump_regs("CMD1_FAIL", self);
                return;
            }

            ocr = self.reg(EmmcReg::Resp0).read();
            if (ocr & 0x8000_0000) != 0 {
                ready = true;
                break;
            }
            crate::time::time_manager().spin_for(Duration::from_millis(1));
        }

        if !ready {
            warn!("CMD1 did not become ready (OCR=0x{:08x})", ocr);
            dump_regs("CMD1_NOT_READY", self);
            return;
        }

        // 7) CMD2
        if let Err(e) = self.send_cmd(
            EmmcCommand::AllSendCid,
            0,
            CmdRespType::R136,
            CmdFlag::CrcChkEn as u32,
        ) {
            match e {
                EmmcError::IrqError(v) => {
                    warn!("CMD2 failed: IrqError(0x{:08x})", v);
                    dump_irq("CMD2_IRQ", v);
                }
                _ => warn!("CMD2 failed: {:?}", e),
            }
            dump_regs("CMD2_FAIL", self);
            return;
        }

        // 8) CMD3 (host assigns RCA)
        let rca: u16 = 0x0001;
        if let Err(e) = self.send_cmd(
            EmmcCommand::SendRelativeAddr,
            (rca as u32) << 16,
            CmdRespType::R48,
            (CmdFlag::CrcChkEn as u32) | (CmdFlag::IxChkEn as u32),
        ) {
            match e {
                EmmcError::IrqError(v) => {
                    warn!("CMD3 failed: IrqError(0x{:08x})", v);
                    dump_irq("CMD3_IRQ", v);
                }
                _ => warn!("CMD3 failed: {:?}", e),
            }
            dump_regs("CMD3_FAIL", self);
            return;
        }

        // store RCA for CMD7
        self.rca = rca;
        crate::time::time_manager().spin_for(Duration::from_millis(1));

        // 9) CMD7
        if let Err(e) = self.send_cmd(
            EmmcCommand::SelectCard,
            (self.rca as u32) << 16,
            CmdRespType::R48Busy,
            (CmdFlag::CrcChkEn as u32) | (CmdFlag::IxChkEn as u32),
        ) {
            match e {
                EmmcError::IrqError(v) => {
                    warn!("CMD7 failed: IrqError(0x{:08x})", v);
                    dump_irq("CMD7_IRQ", v);
                }
                _ => warn!("CMD7 failed: {:?}", e),
            }
            dump_regs("CMD7_FAIL", self);
            return;
        }

        // 10) CMD16
        if let Err(e) = self.send_cmd(
            EmmcCommand::SetBlockLen,
            EMMC_BLOCK_SIZE as u32,
            CmdRespType::R48,
            (CmdFlag::CrcChkEn as u32) | (CmdFlag::IxChkEn as u32),
        ) {
            match e {
                EmmcError::IrqError(v) => {
                    warn!("CMD16 failed: IrqError(0x{:08x})", v);
                    dump_irq("CMD16_IRQ", v);
                }
                _ => warn!("CMD16 failed: {:?}", e),
            }
            dump_regs("CMD16_FAIL", self);
            return;
        }

        // Read EXT_CSD while the card is selected so write-cache durability
        // can be handled correctly by BlockDevice::flush().
        let mut ext_csd = [0u8; EMMC_BLOCK_SIZE];
        if let Err(e) = self.read_ext_csd(&mut ext_csd) {
            warn!("CMD8 SEND_EXT_CSD failed: {:?}", e);
            dump_regs("EXT_CSD_FAIL", self);
            return;
        }

        self.ext_csd_rev = ext_csd[EXT_CSD_REV];
        self.cache_size_kib = if self.ext_csd_rev >= 6 {
            u32::from_le_bytes([
                ext_csd[EXT_CSD_CACHE_SIZE],
                ext_csd[EXT_CSD_CACHE_SIZE + 1],
                ext_csd[EXT_CSD_CACHE_SIZE + 2],
                ext_csd[EXT_CSD_CACHE_SIZE + 3],
            ])
        } else {
            0
        };
        self.cache_enabled = self.cache_size_kib != 0 && (ext_csd[EXT_CSD_CACHE_CTRL] & 1) != 0;
        info!(
            "eMMC EXT_CSD: rev={}, cache={} KiB, cache_enabled={}",
            self.ext_csd_rev, self.cache_size_kib, self.cache_enabled
        );

        // 11) Data clock
        if let Err(e) = self.set_clock_divided(4) {
            warn!("data clock setup failed: {:?}", e);
            dump_regs("DATA_CLOCK_FAIL", self);
            return;
        }

        self.initialized = true;
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    //----------------------------------------------------------------------------------------------
    // Read a single 512-byte block (PIO) - CMD17
    //----------------------------------------------------------------------------------------------
    pub fn read_block(&self, lba: u32, out: &mut [u8; EMMC_BLOCK_SIZE]) -> Result<(), EmmcError> {
        if !self.initialized {
            return Err(EmmcError::NotInitialized);
        }

        // BLKSIZECNT: 1 block, 512 bytes
        self.reg(EmmcReg::BlkSizeCnt)
            .write(((1u32) << 16) | (EMMC_BLOCK_SIZE as u32));

        self.clear_all_interrupts();

        // ARG1 then CMDTM
        self.reg(EmmcReg::Arg1).write(lba);

        let cmdtm = cmdtm_encode(
            EmmcCommand::ReadSingleBlock,
            CmdRespType::R48,
            (CmdFlag::IsData as u32)
                | (CmdFlag::CrcChkEn as u32)
                | (CmdFlag::IxChkEn as u32)
                | (XferFlag::BlkCntEn as u32)
                | (XferFlag::DataDirRead as u32),
        );

        self.reg(EmmcReg::CmdTm).write(cmdtm);
        Self::barrier_io();

        // Wait READ_RDY or ERR
        let intr = self.reg(EmmcReg::Interrupt);
        let intrc = intr;

        Self::wait_while(
            || (intrc.read() & (IRQ_READ_RDY | IRQ_ERR)) == 0,
            4_000_000,
            "READ_RDY",
        )?;

        let v = intr.read();
        if (v & IRQ_ERR) != 0 || (v & 0xFFFF_0000) != 0 {
            self.clear_interrupts(v);
            return Err(EmmcError::IrqError(v));
        }

        // Ack READ_RDY + anything else seen
        self.clear_interrupts(v);

        // Drain 512 bytes
        for i in 0..128 {
            let w = self.reg(EmmcReg::Data).read();
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }

        // Wait DATA_DONE or ERR
        let intrc2 = intr;
        Self::wait_while(
            || (intrc2.read() & (IRQ_DATA_DONE | IRQ_ERR)) == 0,
            4_000_000,
            "DATA_DONE",
        )?;

        let v2 = intr.read();
        if (v2 & IRQ_ERR) != 0 || (v2 & 0xFFFF_0000) != 0 {
            self.clear_interrupts(v2);
            return Err(EmmcError::IrqError(v2));
        }

        self.clear_interrupts(v2);

        Ok(())
    }

    //----------------------------------------------------------------------------------------------
    // Write a single 512-byte block (PIO) - CMD24
    //----------------------------------------------------------------------------------------------
    pub fn write_block(&self, lba: u32, data: &[u8; EMMC_BLOCK_SIZE]) -> Result<(), EmmcError> {
        if !self.initialized {
            return Err(EmmcError::NotInitialized);
        }

        // BLKSIZECNT: 1 block, 512 bytes
        self.reg(EmmcReg::BlkSizeCnt)
            .write(((1u32) << 16) | (EMMC_BLOCK_SIZE as u32));

        self.clear_all_interrupts();

        // ARG1 then CMDTM
        self.reg(EmmcReg::Arg1).write(lba);

        let cmdtm = cmdtm_encode(
            EmmcCommand::WriteSingleBlock,
            CmdRespType::R48,
            (CmdFlag::IsData as u32)
                | (CmdFlag::CrcChkEn as u32)
                | (CmdFlag::IxChkEn as u32)
                | (XferFlag::BlkCntEn as u32), // write => no DataDirRead
        );

        self.reg(EmmcReg::CmdTm).write(cmdtm);
        Self::barrier_io();

        // Wait WRITE_RDY or ERR
        let intr = self.reg(EmmcReg::Interrupt);
        let intrc = intr;

        Self::wait_while(
            || (intrc.read() & (IRQ_WRITE_RDY | IRQ_ERR)) == 0,
            4_000_000,
            "WRITE_RDY",
        )?;

        let v = intr.read();
        if (v & IRQ_ERR) != 0 || (v & 0xFFFF_0000) != 0 {
            self.clear_interrupts(v);
            return Err(EmmcError::IrqError(v));
        }

        // Ack WRITE_RDY (+ anything else observed)
        self.clear_interrupts(v);

        // Push 512 bytes = 128 words
        for i in 0..128 {
            let b0 = data[i * 4] as u32;
            let b1 = (data[i * 4 + 1] as u32) << 8;
            let b2 = (data[i * 4 + 2] as u32) << 16;
            let b3 = (data[i * 4 + 3] as u32) << 24;
            let w = b0 | b1 | b2 | b3;
            self.reg(EmmcReg::Data).write(w);
        }

        Self::barrier_io();

        // Wait DATA_DONE or ERR
        let intrc2 = intr;
        Self::wait_while(
            || (intrc2.read() & (IRQ_DATA_DONE | IRQ_ERR)) == 0,
            4_000_000,
            "DATA_DONE",
        )?;

        let v2 = intr.read();
        if (v2 & IRQ_ERR) != 0 || (v2 & 0xFFFF_0000) != 0 {
            self.clear_interrupts(v2);
            return Err(EmmcError::IrqError(v2));
        }

        // Ack DATA_DONE (+ any other)
        self.clear_interrupts(v2);

        // Ensure DAT line released after the write (makes writes deterministic).
        self.wait_dat_inhibit_clear()
            .map_err(|_| EmmcError::InhibitStuck(self.reg(EmmcReg::Status).read()))?;

        Ok(())
    }
}

//--------------------------------------------------------------------------------------------------
// Static wrapper (your pattern)
//--------------------------------------------------------------------------------------------------
pub struct EmmcStatic(UnsafeCell<Emmc>);
unsafe impl Sync for EmmcStatic {}

pub static EMMC: EmmcStatic = EmmcStatic(UnsafeCell::new(Emmc::new()));

impl EmmcStatic {
    pub fn get_mut(&self) -> &mut Emmc {
        unsafe { &mut *self.0.get() }
    }
    pub fn get(&self) -> &Emmc {
        unsafe { &*self.0.get() }
    }
}

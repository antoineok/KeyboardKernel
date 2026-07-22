use core::{
    hint::spin_loop,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{compiler_fence, Ordering},
    time::Duration,
};

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use crate::{arch::time::uptime, fdt::AliasEntry};

use super::registers::*;

/// Raw hardware capability registers discovered from the DWC2 core.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Dwc2Hardware {
    pub snpsid: u32,
    pub hwcfg1: u32,
    pub hwcfg2: u32,
    pub hwcfg3: u32,
    pub hwcfg4: u32,
}

impl Dwc2Hardware {
    #[inline(always)]
    pub const fn revision(self) -> u16 {
        self.snpsid as u16
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dwc2Error {
    MissingAlias,
    RegisterWindowTooSmall {
        actual: u64,
        required: u64,
    },
    InvalidCoreId(u32),
    NotInitialized,
    Ep0PacketTooLarge(usize),
    InvalidEndpoint(usize),
    EndpointBusy(usize),
    PacketTooLarge {
        endpoint: usize,
        actual: usize,
        maximum: usize,
    },
    Timeout {
        register: usize,
        mask: u32,
        expected_set: bool,
    },
}

/// Low-level Synopsys DWC2 controller.
///
/// This type deliberately contains no USB descriptors or class-specific state.
/// It only controls registers, FIFOs, endpoint hardware and interrupt state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbSpeed {
    High,
    Full,
    Low,
}

impl UsbSpeed {
    pub const fn management_packet_size(self) -> usize {
        match self {
            Self::High => 512,
            Self::Full | Self::Low => 64,
        }
    }
}

pub struct Dwc2 {
    base: u64,
    register_window_size: u64,
    hardware: Dwc2Hardware,
    initialized: bool,
}

impl Dwc2 {
    pub const fn new() -> Self {
        Self {
            base: 0,
            register_window_size: 0,
            hardware: Dwc2Hardware {
                snpsid: 0,
                hwcfg1: 0,
                hwcfg2: 0,
                hwcfg3: 0,
                hwcfg4: 0,
            },
            initialized: false,
        }
    }

    /// Maps and probes the controller, then leaves every USB interrupt masked.
    ///
    /// This is safe to call during early hardware initialization. It does not
    /// allocate memory, configure endpoints or connect the device to the host.
    pub fn init(&mut self, mmio_entry: Option<AliasEntry>) -> Result<(), Dwc2Error> {
        let entry = mmio_entry.ok_or(Dwc2Error::MissingAlias)?;

        if entry.size < MIN_REGISTER_WINDOW_SIZE {
            return Err(Dwc2Error::RegisterWindowTooSmall {
                actual: entry.size,
                required: MIN_REGISTER_WINDOW_SIZE,
            });
        }

        self.base = entry.arm_phys;
        self.register_window_size = entry.size;

        let snpsid = self.read_raw(GSNPSID);
        if snpsid & GSNPSID_OTG_MASK != GSNPSID_OTG_VALUE {
            self.base = 0;
            self.register_window_size = 0;
            return Err(Dwc2Error::InvalidCoreId(snpsid));
        }

        self.hardware = Dwc2Hardware {
            snpsid,
            hwcfg1: self.read_raw(GHWCFG1),
            hwcfg2: self.read_raw(GHWCFG2),
            hwcfg3: self.read_raw(GHWCFG3),
            hwcfg4: self.read_raw(GHWCFG4),
        };
        self.initialized = true;

        // Prevent the core from raising CPU interrupts before the kernel's USB
        // interrupt path and endpoint-zero state machine exist.
        self.disable_global_interrupts()?;
        self.write_raw(GINTMSK, 0);
        self.clear_pending_interrupts()?;

        // If the boot firmware/ID pin already selected device mode, remain
        // electrically disconnected until the higher-level USB stack is ready.
        if self.is_device_mode()? {
            self.disconnect()?;
        }

        crate::info!(
            "DWC2 initialized: base={:#x}, size={:#x}, snpsid={:#010x}",
            self.base,
            self.register_window_size,
            self.hardware.snpsid
        );
        crate::info!(
            "DWC2 capabilities: hwcfg1={:#010x}, hwcfg2={:#010x}, hwcfg3={:#010x}, hwcfg4={:#010x}",
            self.hardware.hwcfg1,
            self.hardware.hwcfg2,
            self.hardware.hwcfg3,
            self.hardware.hwcfg4
        );

        Ok(())
    }

    #[inline(always)]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    #[inline(always)]
    pub const fn base(&self) -> Option<u64> {
        if self.initialized {
            Some(self.base)
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn register_window_size(&self) -> Option<u64> {
        if self.initialized {
            Some(self.register_window_size)
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn hardware(&self) -> Option<Dwc2Hardware> {
        if self.initialized {
            Some(self.hardware)
        } else {
            None
        }
    }

    /// Prepares the controller for a future device-mode USB stack.
    ///
    /// Call this later, once the kernel is ready to build endpoint state. The
    /// controller remains soft-disconnected when this method returns.
    pub fn prepare_device_mode(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.disable_global_interrupts()?;
        self.write_raw(GINTMSK, 0);

        // Disconnect first when the controller is already in peripheral mode.
        if self.is_device_mode()? {
            self.disconnect()?;
        }

        self.core_soft_reset()?;
        self.force_device_mode()?;
        self.disconnect()?;
        self.clear_pending_interrupts()?;

        Ok(())
    }

    /// Performs a DWC2 core soft reset.
    pub fn core_soft_reset(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.wait_for_bits(
            GRSTCTL,
            GRSTCTL_AHBIDLE,
            true,
            Duration::from_millis(100),
        )?;

        // GRSTCTL contains command/status bits. Do not replay a value read
        // from it; issue only the core-soft-reset command.
        self.write_raw(GRSTCTL, GRSTCTL_CSFTRST);
        self.device_barrier();

        self.wait_for_bits(
            GRSTCTL,
            GRSTCTL_CSFTRST,
            false,
            Duration::from_millis(100),
        )?;

        // The core requires a few PHY clocks after CSFTRST deasserts before
        // accessing additional registers. Three microseconds is deliberately
        // conservative for a high/full-speed PHY clock.
        self.delay(Duration::from_micros(3));

        Ok(())
    }

    /// Forces the OTG controller into peripheral/device mode.
    pub fn force_device_mode(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.modify_raw(GUSBCFG, |mut value| {
            value &= !GUSBCFG_FORCEHOSTMODE;
            value |= GUSBCFG_FORCEDEVMODE;
            value
        });

        // Mode forcing is not instantaneous. Wait for the current-mode bit,
        // then retain the standard DWC2 settling interval.
        self.wait_for_bits(
            GINTSTS,
            GINT_CURMODE_HOST,
            false,
            Duration::from_millis(100),
        )?;
        self.delay(Duration::from_millis(25));

        Ok(())
    }

    #[inline]
    pub fn is_device_mode(&self) -> Result<bool, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GINTSTS) & GINT_CURMODE_HOST == 0)
    }

    /// Enables the device pull-up by clearing DCTL.SFTDISCON.
    ///
    /// Do not call this until endpoint zero and the descriptor state machine
    /// are completely ready to answer the host.
    pub fn connect(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(DCTL, |value| value & !DCTL_SFTDISCON);
        self.device_barrier();
        Ok(())
    }

    /// Disables the device pull-up by setting DCTL.SFTDISCON.
    pub fn disconnect(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(DCTL, |value| value | DCTL_SFTDISCON);
        self.device_barrier();
        Ok(())
    }

    pub fn enable_global_interrupts(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(GAHBCFG, |value| value | GAHBCFG_GLBL_INTR_EN);
        self.device_barrier();
        Ok(())
    }

    pub fn disable_global_interrupts(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(GAHBCFG, |value| value & !GAHBCFG_GLBL_INTR_EN);
        self.device_barrier();
        Ok(())
    }

    pub fn set_interrupt_mask(&mut self, mask: u32) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(GINTMSK, mask);
        self.device_barrier();
        Ok(())
    }

    pub fn interrupt_mask(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GINTMSK))
    }

    pub fn interrupt_status(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GINTSTS))
    }

    /// Acknowledges write-one-to-clear global interrupt bits.
    pub fn acknowledge_interrupts(&mut self, bits: u32) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(GINTSTS, bits);
        self.device_barrier();
        Ok(())
    }

    pub fn clear_pending_interrupts(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        let pending = self.read_raw(GINTSTS);

        // Bit zero reports the current host/device mode and is read-only.
        self.write_raw(GINTSTS, pending & !GINT_CURMODE_HOST);
        self.device_barrier();

        Ok(())
    }

    /// Configures the device-global registers and endpoint zero for a
    /// polling-only USB device stack. The device remains soft-disconnected.
    pub fn initialize_device_controller(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.disconnect()?;

        // Start in programmed-I/O mode. Keep the controller's global
        // interrupt gate open even though the CPU still polls GINTSTS with
        // DAIF masked.
        self.modify_raw(GAHBCFG, |value| {
            (value & !GAHBCFG_DMA_EN) | GAHBCFG_GLBL_INTR_EN
        });

        // Disable HNP/SRP and use the maximum turnaround calibration.
        self.modify_raw(GUSBCFG, |mut value| {
            value &= !(GUSBCFG_TOUTCAL_MASK | GUSBCFG_SRPCAP | GUSBCFG_HNPCAP);
            value |= GUSBCFG_TOUTCAL_7;
            value
        });

        self.modify_raw(DCFG, |mut value| {
            value &= !(DCFG_DEVSPD_MASK | DCFG_DEVADDR_MASK);
            value |= DCFG_DEVSPD_HIGH;
            value
        });

        self.configure_device_fifos()?;

        self.write_raw(DIEPMSK, DIEPINT_XFRC | DIEPINT_TOC);
        self.write_raw(DOEPMSK, DOEPINT_XFRC | DOEPINT_STUP | DOEPINT_B2BSTUP);
        self.write_raw(DAINTMSK, DAINT_IN_EP0 | DAINT_OUT_EP0);

        // These masks affect the interrupt summary bits even though global
        // CPU interrupt output remains disabled and the stack polls GINTSTS.
        self.write_raw(
            GINTMSK,
            GINT_USBRST
                | GINT_ENUMDONE
                | GINT_RXFLVL
                | GINT_IEPINT
                | GINT_OEPINT
                | GINT_USBSUSP
                | GINT_WKUPINT
                | GINT_DISCONNINT,
        );

        self.write_raw(diepint(0), u32::MAX);
        self.write_raw(doepint(0), u32::MAX);
        self.clear_pending_interrupts()?;
        self.configure_ep0_max_packet(64)?;

        // Required DWC2 device-programming pulse.
        self.modify_raw(DCTL, |value| value | DCTL_PWRONPRGDONE);
        self.device_barrier();
        self.delay(Duration::from_micros(10));
        self.modify_raw(DCTL, |value| value & !DCTL_PWRONPRGDONE);

        // Clear global endpoint NAK state left by reset.
        self.modify_raw(DCTL, |value| value | DCTL_CGOUTNAK | DCTL_CGNPINNAK);
        self.device_barrier();

        Ok(())
    }

    /// Restores endpoint-zero state after a USB bus reset.
    pub fn handle_usb_reset(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.modify_raw(DCFG, |value| value & !DCFG_DEVADDR_MASK);
        self.modify_raw(DCTL, |value| value & !DCTL_RMTWKUPSIG);

        self.write_raw(DIEPMSK, DIEPINT_XFRC | DIEPINT_TOC);
        self.write_raw(DOEPMSK, DOEPINT_XFRC | DOEPINT_STUP | DOEPINT_B2BSTUP);
        self.write_raw(DAINTMSK, DAINT_IN_EP0 | DAINT_OUT_EP0);
        for endpoint in 0..=3 {
            self.write_raw(diepint(endpoint), u32::MAX);
            self.write_raw(doepint(endpoint), u32::MAX);
        }
        self.deconfigure_non_control_endpoints()?;

        self.flush_tx_fifo_all()?;
        self.flush_rx_fifo()?;
        self.configure_ep0_max_packet(64)?;

        self.modify_raw(DCTL, |value| value | DCTL_CGOUTNAK | DCTL_CGNPINNAK);
        self.device_barrier();

        self.arm_ep0_setup()?;

        Ok(())
    }

    /// Applies endpoint-zero sizing after the host has established bus speed.
    pub fn handle_enumeration_done(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        let packet_size = if self.enumerated_speed()? == UsbSpeed::Low {
            8
        } else {
            64
        };
        self.configure_ep0_max_packet(packet_size)
    }

    pub fn enumerated_speed(&self) -> Result<UsbSpeed, Dwc2Error> {
        self.ensure_initialized()?;
        let speed = (self.read_raw(DSTS) & DSTS_ENUMSPD_MASK) >> DSTS_ENUMSPD_SHIFT;
        Ok(match speed {
            DSTS_ENUMSPD_HIGH => UsbSpeed::High,
            DSTS_ENUMSPD_LOW_6 => UsbSpeed::Low,
            DSTS_ENUMSPD_FULL_30_60 | DSTS_ENUMSPD_FULL_48 => UsbSpeed::Full,
            _ => UsbSpeed::Full,
        })
    }

    /// Arms endpoint zero OUT to receive one eight-byte setup packet.
    pub fn arm_ep0_setup(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.write_raw(
            doeptsiz(0),
            8 | DEPTSIZ0_PKTCNT_ONE | DOEPTSIZ0_SUPCNT_ONE,
        );

        self.modify_raw(doepctl(0), |mut value| {
            value &= !(DEPCTL0_MPS_MASK | DEPCTL_STALL);
            value |=
                DEPCTL0_MPS_64 | DEPCTL_USBACTEP | DEPCTL_CNAK | DEPCTL_EPENA;
            value
        });
        self.device_barrier();

        Ok(())
    }

    /// Arms the zero-length OUT status stage of a control-read transfer.
    pub fn arm_ep0_status_out(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(doeptsiz(0), DEPTSIZ0_PKTCNT_ONE);
        self.modify_raw(doepctl(0), |mut value| {
            value &= !(DEPCTL0_MPS_MASK | DEPCTL_STALL);
            value |= DEPCTL0_MPS_64 | DEPCTL_USBACTEP | DEPCTL_CNAK | DEPCTL_EPENA;
            value
        });
        self.device_barrier();
        Ok(())
    }

    /// Starts one endpoint-zero IN packet. Enumeration descriptors in the
    /// current stack are deliberately limited to a single 64-byte packet.
    pub fn start_ep0_in(&mut self, data: &[u8]) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        if data.len() > 64 {
            return Err(Dwc2Error::Ep0PacketTooLarge(data.len()));
        }

        self.wait_for_ep0_tx_space((data.len() + 3) / 4)?;
        self.write_raw(
            dieptsiz(0),
            (data.len() as u32 & DEPTSIZ0_XFERSIZE_MASK) | DEPTSIZ0_PKTCNT_ONE,
        );

        self.modify_raw(diepctl(0), |mut value| {
            value &= !(DEPCTL0_MPS_MASK | DEPCTL_STALL | DEPCTL_TXFNUM_MASK);
            value |= DEPCTL0_MPS_64 | DEPCTL_USBACTEP | DEPCTL_CNAK | DEPCTL_EPENA;
            value
        });
        self.device_barrier();

        for chunk in data.chunks(4) {
            let mut word = 0u32;
            for (index, byte) in chunk.iter().enumerate() {
                word |= u32::from(*byte) << (index * 8);
            }
            self.write_raw(fifo(0), word);
        }
        self.device_barrier();

        Ok(())
    }

    #[inline]
    pub fn start_ep0_status_in(&mut self) -> Result<(), Dwc2Error> {
        self.start_ep0_in(&[])
    }

    pub fn set_device_address(&mut self, address: u8) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(DCFG, |mut value| {
            value &= !DCFG_DEVADDR_MASK;
            value |= (u32::from(address) << DCFG_DEVADDR_SHIFT) & DCFG_DEVADDR_MASK;
            value
        });
        self.device_barrier();
        Ok(())
    }

    pub fn stall_ep0(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(diepctl(0), |value| value | DEPCTL_STALL | DEPCTL_USBACTEP);
        self.modify_raw(doepctl(0), |value| value | DEPCTL_STALL | DEPCTL_USBACTEP);
        self.device_barrier();
        Ok(())
    }

    pub fn clear_ep0_stall(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        // Do not force DATA0 on endpoint zero. The SETUP transaction resets
        // the control-transfer toggle and the core must send the data/status
        // PID selected by that state machine.
        self.modify_raw(diepctl(0), |value| {
            (value & !DEPCTL_STALL) | DEPCTL_USBACTEP
        });
        self.modify_raw(doepctl(0), |value| {
            (value & !DEPCTL_STALL) | DEPCTL_USBACTEP
        });
        self.device_barrier();
        Ok(())
    }

    #[inline]
    pub fn pop_rx_status(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GRXSTSP))
    }

    #[inline]
    pub fn read_fifo_word(&self, endpoint: usize) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(fifo(endpoint)))
    }

    pub fn discard_fifo_bytes(
        &self,
        _endpoint: usize,
        byte_count: usize,
    ) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        for _ in 0..byte_count.div_ceil(4) {
            let _ = self.read_raw(fifo(0));
        }
        Ok(())
    }

    #[inline]
    pub fn ep0_in_interrupts(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(diepint(0)))
    }

    #[inline]
    pub fn ep0_out_interrupts(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(doepint(0)))
    }

    pub fn acknowledge_ep0_in_interrupts(&mut self, bits: u32) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(diepint(0), bits);
        self.device_barrier();
        Ok(())
    }

    pub fn acknowledge_ep0_out_interrupts(&mut self, bits: u32) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(doepint(0), bits);
        self.device_barrier();
        Ok(())
    }

    /// Configures the non-control endpoints used by the composite device.
    pub fn configure_composite_endpoints(
        &mut self,
        keyboard_endpoint: usize,
        management_endpoint: usize,
        extra_hid_endpoint: usize,
    ) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        let management_packet_size = self.enumerated_speed()?.management_packet_size();

        self.configure_in_endpoint(
            keyboard_endpoint,
            8,
            DEPCTL_EPTYPE_INTERRUPT,
            keyboard_endpoint,
        )?;
        self.configure_in_endpoint(
            management_endpoint,
            management_packet_size,
            DEPCTL_EPTYPE_BULK,
            management_endpoint,
        )?;
        self.configure_in_endpoint(
            extra_hid_endpoint,
            3,
            DEPCTL_EPTYPE_INTERRUPT,
            extra_hid_endpoint,
        )?;
        self.configure_out_endpoint(
            management_endpoint,
            management_packet_size,
            DEPCTL_EPTYPE_BULK,
        )?;

        self.write_raw(
            DAINTMSK,
            DAINT_IN_EP0
                | DAINT_OUT_EP0
                | daint_in(keyboard_endpoint)
                | daint_in(management_endpoint)
                | daint_in(extra_hid_endpoint)
                | daint_out(management_endpoint),
        );
        self.device_barrier();
        self.arm_out_endpoint(management_endpoint, management_packet_size)?;
        Ok(())
    }

    pub fn deconfigure_non_control_endpoints(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        for endpoint in 1..=3 {
            self.modify_raw(diepctl(endpoint), |value| {
                (value | DEPCTL_SNAK) & !DEPCTL_USBACTEP
            });
            self.modify_raw(doepctl(endpoint), |value| {
                (value | DEPCTL_SNAK) & !DEPCTL_USBACTEP
            });
            self.write_raw(diepint(endpoint), u32::MAX);
            self.write_raw(doepint(endpoint), u32::MAX);
        }
        self.write_raw(DAINTMSK, DAINT_IN_EP0 | DAINT_OUT_EP0);
        self.device_barrier();
        Ok(())
    }

    fn configure_in_endpoint(
        &mut self,
        endpoint: usize,
        max_packet_size: usize,
        endpoint_type: u32,
        fifo_number: usize,
    ) -> Result<(), Dwc2Error> {
        self.validate_non_control_endpoint(endpoint)?;
        self.write_raw(diepint(endpoint), u32::MAX);
        self.modify_raw(diepctl(endpoint), |mut value| {
            value &= !(DEPCTL_MPS_MASK
                | DEPCTL_EPTYPE_MASK
                | DEPCTL_TXFNUM_MASK
                | DEPCTL_STALL
                | DEPCTL_EPENA
                | DEPCTL_EPDIS);
            value |= (max_packet_size as u32 & DEPCTL_MPS_MASK)
                | endpoint_type
                | (((fifo_number as u32) << DEPCTL_TXFNUM_SHIFT) & DEPCTL_TXFNUM_MASK)
                | DEPCTL_USBACTEP
                | DEPCTL_SNAK
                | DEPCTL_SETD0PID;
            value
        });
        self.device_barrier();
        Ok(())
    }

    fn configure_out_endpoint(
        &mut self,
        endpoint: usize,
        max_packet_size: usize,
        endpoint_type: u32,
    ) -> Result<(), Dwc2Error> {
        self.validate_non_control_endpoint(endpoint)?;
        self.write_raw(doepint(endpoint), u32::MAX);
        self.modify_raw(doepctl(endpoint), |mut value| {
            value &= !(DEPCTL_MPS_MASK
                | DEPCTL_EPTYPE_MASK
                | DEPCTL_STALL
                | DEPCTL_EPENA
                | DEPCTL_EPDIS);
            value |= (max_packet_size as u32 & DEPCTL_MPS_MASK)
                | endpoint_type
                | DEPCTL_USBACTEP
                | DEPCTL_SNAK
                | DEPCTL_SETD0PID;
            value
        });
        self.device_barrier();
        Ok(())
    }

    pub fn start_in_transfer(
        &mut self,
        endpoint: usize,
        max_packet_size: usize,
        data: &[u8],
    ) -> Result<(), Dwc2Error> {
        self.validate_non_control_endpoint(endpoint)?;
        if data.len() > max_packet_size {
            return Err(Dwc2Error::PacketTooLarge {
                endpoint,
                actual: data.len(),
                maximum: max_packet_size,
            });
        }
        if self.read_raw(diepctl(endpoint)) & DEPCTL_EPENA != 0 {
            return Err(Dwc2Error::EndpointBusy(endpoint));
        }

        self.wait_for_tx_space(endpoint, data.len().div_ceil(4))?;
        self.write_raw(
            dieptsiz(endpoint),
            (data.len() as u32 & DEPTSIZ_XFERSIZE_MASK) | DEPTSIZ_PKTCNT_ONE,
        );
        self.modify_raw(diepctl(endpoint), |value| {
            (value & !(DEPCTL_STALL | DEPCTL_SNAK)) | DEPCTL_CNAK | DEPCTL_EPENA
        });
        self.device_barrier();

        for chunk in data.chunks(4) {
            let mut word = 0u32;
            for (index, byte) in chunk.iter().enumerate() {
                word |= u32::from(*byte) << (index * 8);
            }
            self.write_raw(fifo(endpoint), word);
        }
        self.device_barrier();
        Ok(())
    }

    pub fn arm_out_endpoint(
        &mut self,
        endpoint: usize,
        max_packet_size: usize,
    ) -> Result<(), Dwc2Error> {
        self.validate_non_control_endpoint(endpoint)?;
        if self.read_raw(doepctl(endpoint)) & DEPCTL_EPENA != 0 {
            return Err(Dwc2Error::EndpointBusy(endpoint));
        }
        self.write_raw(
            doeptsiz(endpoint),
            (max_packet_size as u32 & DEPTSIZ_XFERSIZE_MASK) | DEPTSIZ_PKTCNT_ONE,
        );
        self.modify_raw(doepctl(endpoint), |value| {
            (value & !(DEPCTL_STALL | DEPCTL_SNAK)) | DEPCTL_CNAK | DEPCTL_EPENA
        });
        self.device_barrier();
        Ok(())
    }

    /// Arms an endpoint-zero OUT data stage of up to 64 bytes.
    pub fn arm_ep0_out(&mut self, length: usize) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        if length > 64 {
            return Err(Dwc2Error::Ep0PacketTooLarge(length));
        }
        self.write_raw(
            doeptsiz(0),
            (length as u32 & DEPTSIZ0_XFERSIZE_MASK) | DEPTSIZ0_PKTCNT_ONE,
        );
        self.modify_raw(doepctl(0), |mut value| {
            value &= !(DEPCTL0_MPS_MASK | DEPCTL_STALL);
            value |= DEPCTL0_MPS_64 | DEPCTL_USBACTEP | DEPCTL_CNAK | DEPCTL_EPENA;
            value
        });
        self.device_barrier();
        Ok(())
    }

    #[inline]
    pub fn endpoint_in_interrupts(&self, endpoint: usize) -> Result<u32, Dwc2Error> {
        self.validate_endpoint(endpoint)?;
        Ok(self.read_raw(diepint(endpoint)))
    }

    #[inline]
    pub fn endpoint_out_interrupts(&self, endpoint: usize) -> Result<u32, Dwc2Error> {
        self.validate_endpoint(endpoint)?;
        Ok(self.read_raw(doepint(endpoint)))
    }

    pub fn acknowledge_endpoint_in_interrupts(
        &mut self,
        endpoint: usize,
        bits: u32,
    ) -> Result<(), Dwc2Error> {
        self.validate_endpoint(endpoint)?;
        self.write_raw(diepint(endpoint), bits);
        self.device_barrier();
        Ok(())
    }

    pub fn acknowledge_endpoint_out_interrupts(
        &mut self,
        endpoint: usize,
        bits: u32,
    ) -> Result<(), Dwc2Error> {
        self.validate_endpoint(endpoint)?;
        self.write_raw(doepint(endpoint), bits);
        self.device_barrier();
        Ok(())
    }

    pub fn endpoint_interrupt_summary(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(DAINT))
    }

    #[inline]
    pub fn is_suspended(&self) -> Result<bool, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(DSTS) & DSTS_SUSPSTS != 0)
    }

    fn wait_for_tx_space(
        &self,
        endpoint: usize,
        required_words: usize,
    ) -> Result<(), Dwc2Error> {
        let started = uptime();
        let timeout = Duration::from_millis(100);
        loop {
            let available = (self.read_raw(dtxfsts(endpoint)) & DTXFSTS_INEPTFSAV_MASK) as usize;
            if available >= required_words {
                return Ok(());
            }
            if uptime().saturating_sub(started) >= timeout {
                return Err(Dwc2Error::Timeout {
                    register: dtxfsts(endpoint),
                    mask: required_words as u32,
                    expected_set: true,
                });
            }
            spin_loop();
        }
    }

    fn validate_endpoint(&self, endpoint: usize) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        if endpoint <= 3 {
            Ok(())
        } else {
            Err(Dwc2Error::InvalidEndpoint(endpoint))
        }
    }

    fn validate_non_control_endpoint(&self, endpoint: usize) -> Result<(), Dwc2Error> {
        self.validate_endpoint(endpoint)?;
        if endpoint == 0 {
            Err(Dwc2Error::InvalidEndpoint(endpoint))
        } else {
            Ok(())
        }
    }

    fn configure_device_fifos(&mut self) -> Result<(), Dwc2Error> {
        // The BCM2711 core reports substantially more FIFO RAM than this
        // conservative layout consumes. Sizes are in 32-bit words.
        const RX_FIFO_WORDS: u32 = 512;
        const EP0_TX_FIFO_WORDS: u32 = 128;
        const KEYBOARD_TX_FIFO_WORDS: u32 = 32;
        const MANAGEMENT_TX_FIFO_WORDS: u32 = 256;
        const EXTRA_HID_TX_FIFO_WORDS: u32 = 32;

        let ep0_start = RX_FIFO_WORDS;
        let keyboard_start = ep0_start + EP0_TX_FIFO_WORDS;
        let management_start = keyboard_start + KEYBOARD_TX_FIFO_WORDS;
        let extra_hid_start = management_start + MANAGEMENT_TX_FIFO_WORDS;

        self.write_raw(GRXFSIZ, RX_FIFO_WORDS);
        self.write_raw(
            GNPTXFSIZ,
            (ep0_start & FIFOSIZE_STARTADDR_MASK)
                | ((EP0_TX_FIFO_WORDS << FIFOSIZE_DEPTH_SHIFT) & FIFOSIZE_DEPTH_MASK),
        );
        self.write_raw(
            dieptxf(1),
            (keyboard_start & FIFOSIZE_STARTADDR_MASK)
                | ((KEYBOARD_TX_FIFO_WORDS << FIFOSIZE_DEPTH_SHIFT) & FIFOSIZE_DEPTH_MASK),
        );
        self.write_raw(
            dieptxf(2),
            (management_start & FIFOSIZE_STARTADDR_MASK)
                | ((MANAGEMENT_TX_FIFO_WORDS << FIFOSIZE_DEPTH_SHIFT) & FIFOSIZE_DEPTH_MASK),
        );
        self.write_raw(
            dieptxf(3),
            (extra_hid_start & FIFOSIZE_STARTADDR_MASK)
                | ((EXTRA_HID_TX_FIFO_WORDS << FIFOSIZE_DEPTH_SHIFT) & FIFOSIZE_DEPTH_MASK),
        );
        self.device_barrier();

        self.flush_rx_fifo()?;
        self.flush_tx_fifo_all()?;
        Ok(())
    }

    fn flush_rx_fifo(&mut self) -> Result<(), Dwc2Error> {
        self.write_raw(GRSTCTL, GRSTCTL_RXFFLSH);
        self.device_barrier();
        self.wait_for_bits(
            GRSTCTL,
            GRSTCTL_RXFFLSH,
            false,
            Duration::from_millis(100),
        )
    }

    fn flush_tx_fifo_all(&mut self) -> Result<(), Dwc2Error> {
        self.write_raw(GRSTCTL, GRSTCTL_TXFFLSH | GRSTCTL_TXFNUM_ALL);
        self.device_barrier();
        self.wait_for_bits(
            GRSTCTL,
            GRSTCTL_TXFFLSH,
            false,
            Duration::from_millis(100),
        )
    }

    fn configure_ep0_max_packet(&mut self, packet_size: usize) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        let encoded = match packet_size {
            64 => DEPCTL0_MPS_64,
            32 => DEPCTL0_MPS_32,
            16 => DEPCTL0_MPS_16,
            _ => DEPCTL0_MPS_8,
        };

        self.modify_raw(diepctl(0), |value| {
            (value & !DEPCTL0_MPS_MASK) | encoded | DEPCTL_USBACTEP
        });
        self.modify_raw(doepctl(0), |value| {
            (value & !DEPCTL0_MPS_MASK) | encoded | DEPCTL_USBACTEP
        });
        self.device_barrier();
        Ok(())
    }

    fn wait_for_ep0_tx_space(&self, required_words: usize) -> Result<(), Dwc2Error> {
        let started = uptime();
        let timeout = Duration::from_millis(100);

        loop {
            let available = (self.read_raw(dtxfsts(0)) & DTXFSTS_INEPTFSAV_MASK) as usize;
            if available >= required_words {
                return Ok(());
            }

            if uptime().saturating_sub(started) >= timeout {
                return Err(Dwc2Error::Timeout {
                    register: dtxfsts(0),
                    mask: required_words as u32,
                    expected_set: true,
                });
            }

            spin_loop();
        }
    }

    #[inline(always)]
    fn ensure_initialized(&self) -> Result<(), Dwc2Error> {
        if self.initialized {
            Ok(())
        } else {
            Err(Dwc2Error::NotInitialized)
        }
    }

    fn wait_for_bits(
        &self,
        register: usize,
        mask: u32,
        expected_set: bool,
        timeout: Duration,
    ) -> Result<(), Dwc2Error> {
        let started = uptime();

        loop {
            let value = self.read_raw(register);
            let condition = value & mask == mask;

            if condition == expected_set {
                return Ok(());
            }

            if uptime().saturating_sub(started) >= timeout {
                return Err(Dwc2Error::Timeout {
                    register,
                    mask,
                    expected_set,
                });
            }

            spin_loop();
        }
    }

    fn delay(&self, duration: Duration) {
        let started = uptime();
        while uptime().saturating_sub(started) < duration {
            spin_loop();
        }
    }

    #[inline(always)]
    fn modify_raw(&mut self, offset: usize, update: impl FnOnce(u32) -> u32) {
        let value = self.read_raw(offset);
        self.write_raw(offset, update(value));
    }

    #[inline(always)]
    fn read_raw(&self, offset: usize) -> u32 {
        debug_assert!(offset + core::mem::size_of::<u32>() <= self.register_window_size as usize);

        let address = (self.base as usize).wrapping_add(offset);
        unsafe { read_volatile(address as *const u32) }
    }

    #[inline(always)]
    fn write_raw(&mut self, offset: usize, value: u32) {
        debug_assert!(offset + core::mem::size_of::<u32>() <= self.register_window_size as usize);

        let address = (self.base as usize).wrapping_add(offset);
        unsafe {
            write_volatile(address as *mut u32, value);
        }
    }

    #[inline(always)]
    fn device_barrier(&self) {
        compiler_fence(Ordering::SeqCst);

        #[cfg(target_arch = "aarch64")]
        unsafe {
            asm!("dsb sy", options(nostack, preserves_flags));
        }

        compiler_fence(Ordering::SeqCst);
    }
}

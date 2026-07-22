//! Synopsys DWC2 register offsets and bit definitions.
//!
//! Offsets are relative to the controller's translated ARM physical base.

/// The global/device register range used by the current driver.
pub const MIN_REGISTER_WINDOW_SIZE: u64 = 0x5000;

// Global registers.
pub const GOTGCTL: usize = 0x000;
pub const GOTGINT: usize = 0x004;
pub const GAHBCFG: usize = 0x008;
pub const GUSBCFG: usize = 0x00c;
pub const GRSTCTL: usize = 0x010;
pub const GINTSTS: usize = 0x014;
pub const GINTMSK: usize = 0x018;
pub const GRXSTSR: usize = 0x01c;
pub const GRXSTSP: usize = 0x020;
pub const GRXFSIZ: usize = 0x024;
pub const GNPTXFSIZ: usize = 0x028;
pub const GNPTXSTS: usize = 0x02c;
pub const GI2CCTL: usize = 0x030;
pub const GPVNDCTL: usize = 0x034;
pub const GGPIO: usize = 0x038;
pub const GUID: usize = 0x03c;
pub const GSNPSID: usize = 0x040;
pub const GHWCFG1: usize = 0x044;
pub const GHWCFG2: usize = 0x048;
pub const GHWCFG3: usize = 0x04c;
pub const GHWCFG4: usize = 0x050;
pub const GLPMCFG: usize = 0x054;
pub const GPWRDN: usize = 0x058;
pub const GDFIFOCFG: usize = 0x05c;
pub const GADPCTL: usize = 0x060;

// Host periodic TX FIFO sizing begins here. The device-mode driver does not
// currently configure these registers, but keeping the offsets here makes the
// global register map complete enough for diagnostics.
pub const HPTXFSIZ: usize = 0x100;
pub const DIEPTXF1: usize = 0x104;

/// Device IN endpoint TX FIFO size register for endpoint/FIFO `n`.
///
/// `n == 1` maps to DIEPTXF1. Endpoint zero uses GNPTXFSIZ.
#[inline(always)]
pub const fn dieptxf(n: usize) -> usize {
    DIEPTXF1 + (n - 1) * 4
}

// Device-global registers.
pub const DCFG: usize = 0x800;
pub const DCTL: usize = 0x804;
pub const DSTS: usize = 0x808;
pub const DIEPMSK: usize = 0x810;
pub const DOEPMSK: usize = 0x814;
pub const DAINT: usize = 0x818;
pub const DAINTMSK: usize = 0x81c;
pub const DTKNQR1: usize = 0x820;
pub const DTKNQR2: usize = 0x824;
pub const DVBUSDIS: usize = 0x828;
pub const DVBUSPULSE: usize = 0x82c;
pub const DTKNQR3: usize = 0x830;
pub const DTKNQR4: usize = 0x834;

// Device IN endpoint registers.
pub const DIEP_BASE: usize = 0x900;
pub const DIEP_STRIDE: usize = 0x20;

#[inline(always)]
pub const fn diepctl(endpoint: usize) -> usize {
    DIEP_BASE + endpoint * DIEP_STRIDE
}

#[inline(always)]
pub const fn diepint(endpoint: usize) -> usize {
    diepctl(endpoint) + 0x08
}

#[inline(always)]
pub const fn dieptsiz(endpoint: usize) -> usize {
    diepctl(endpoint) + 0x10
}

#[inline(always)]
pub const fn diepdma(endpoint: usize) -> usize {
    diepctl(endpoint) + 0x14
}

#[inline(always)]
pub const fn dtxfsts(endpoint: usize) -> usize {
    diepctl(endpoint) + 0x18
}

// Device OUT endpoint registers.
pub const DOEP_BASE: usize = 0xb00;
pub const DOEP_STRIDE: usize = 0x20;

#[inline(always)]
pub const fn doepctl(endpoint: usize) -> usize {
    DOEP_BASE + endpoint * DOEP_STRIDE
}

#[inline(always)]
pub const fn doepint(endpoint: usize) -> usize {
    doepctl(endpoint) + 0x08
}

#[inline(always)]
pub const fn doeptsiz(endpoint: usize) -> usize {
    doepctl(endpoint) + 0x10
}

#[inline(always)]
pub const fn doepdma(endpoint: usize) -> usize {
    doepctl(endpoint) + 0x14
}

// Endpoint FIFO windows.
pub const FIFO_BASE: usize = 0x1000;
pub const FIFO_STRIDE: usize = 0x1000;

#[inline(always)]
pub const fn fifo(endpoint: usize) -> usize {
    FIFO_BASE + endpoint * FIFO_STRIDE
}

// GAHBCFG.
pub const GAHBCFG_GLBL_INTR_EN: u32 = 1 << 0;

// GUSBCFG.
pub const GUSBCFG_TOUTCAL_MASK: u32 = 0x7;
pub const GUSBCFG_TOUTCAL_7: u32 = 0x7;
pub const GUSBCFG_SRPCAP: u32 = 1 << 8;
pub const GUSBCFG_HNPCAP: u32 = 1 << 9;
pub const GUSBCFG_FORCEHOSTMODE: u32 = 1 << 29;
pub const GUSBCFG_FORCEDEVMODE: u32 = 1 << 30;

// GRSTCTL.
pub const GRSTCTL_CSFTRST: u32 = 1 << 0;
pub const GRSTCTL_RXFFLSH: u32 = 1 << 4;
pub const GRSTCTL_TXFFLSH: u32 = 1 << 5;
pub const GRSTCTL_TXFNUM_SHIFT: u32 = 6;
pub const GRSTCTL_TXFNUM_MASK: u32 = 0x1f << GRSTCTL_TXFNUM_SHIFT;
pub const GRSTCTL_TXFNUM_ALL: u32 = 0x10 << GRSTCTL_TXFNUM_SHIFT;
pub const GRSTCTL_AHBIDLE: u32 = 1 << 31;

// GINTSTS / GINTMSK.
pub const GINT_CURMODE_HOST: u32 = 1 << 0;
pub const GINT_MODEMIS: u32 = 1 << 1;
pub const GINT_OTGINT: u32 = 1 << 2;
pub const GINT_SOF: u32 = 1 << 3;
pub const GINT_RXFLVL: u32 = 1 << 4;
pub const GINT_NPTXFEMP: u32 = 1 << 5;
pub const GINT_GINNAKEFF: u32 = 1 << 6;
pub const GINT_GOUTNAKEFF: u32 = 1 << 7;
pub const GINT_ULPI_CK_INT: u32 = 1 << 8;
pub const GINT_I2CINT: u32 = 1 << 9;
pub const GINT_ERLYSUSP: u32 = 1 << 10;
pub const GINT_USBSUSP: u32 = 1 << 11;
pub const GINT_USBRST: u32 = 1 << 12;
pub const GINT_ENUMDONE: u32 = 1 << 13;
pub const GINT_ISOOUTDROP: u32 = 1 << 14;
pub const GINT_EOPF: u32 = 1 << 15;
pub const GINT_EPMIS: u32 = 1 << 17;
pub const GINT_IEPINT: u32 = 1 << 18;
pub const GINT_OEPINT: u32 = 1 << 19;
pub const GINT_INCOMPLISOIN: u32 = 1 << 20;
pub const GINT_INCOMPLISOOUT: u32 = 1 << 21;
pub const GINT_FETSUSP: u32 = 1 << 22;
pub const GINT_RESETDET: u32 = 1 << 23;
pub const GINT_PRTINT: u32 = 1 << 24;
pub const GINT_HCHINT: u32 = 1 << 25;
pub const GINT_PTXFEMP: u32 = 1 << 26;
pub const GINT_LPMTRANRCVD: u32 = 1 << 27;
pub const GINT_CONIDSTSCHNG: u32 = 1 << 28;
pub const GINT_DISCONNINT: u32 = 1 << 29;
pub const GINT_SESSREQINT: u32 = 1 << 30;
pub const GINT_WKUPINT: u32 = 1 << 31;

// DCFG.
pub const DCFG_DEVSPD_MASK: u32 = 0b11;
pub const DCFG_DEVSPD_HIGH: u32 = 0;
pub const DCFG_DEVSPD_FULL_30_60: u32 = 1;
pub const DCFG_DEVSPD_LOW_6: u32 = 2;
pub const DCFG_DEVSPD_FULL_48: u32 = 3;
pub const DCFG_NZSTSOUTHSHK: u32 = 1 << 2;
pub const DCFG_DEVADDR_SHIFT: u32 = 4;
pub const DCFG_DEVADDR_MASK: u32 = 0x7f << DCFG_DEVADDR_SHIFT;

// DCTL.
pub const DCTL_RMTWKUPSIG: u32 = 1 << 0;
pub const DCTL_SFTDISCON: u32 = 1 << 1;
pub const DCTL_GNPINNAKSTS: u32 = 1 << 2;
pub const DCTL_GOUTNAKSTS: u32 = 1 << 3;
pub const DCTL_TSTCTL_SHIFT: u32 = 4;
pub const DCTL_TSTCTL_MASK: u32 = 0x7 << DCTL_TSTCTL_SHIFT;
pub const DCTL_SGOUTNAK: u32 = 1 << 9;
pub const DCTL_CGOUTNAK: u32 = 1 << 10;
pub const DCTL_PWRONPRGDONE: u32 = 1 << 11;
pub const DCTL_SGNPINNAK: u32 = 1 << 7;
pub const DCTL_CGNPINNAK: u32 = 1 << 8;

// Common endpoint-control bits.
pub const DEPCTL_MPS_MASK: u32 = 0x7ff;
pub const DEPCTL_USBACTEP: u32 = 1 << 15;
pub const DEPCTL_NAKSTS: u32 = 1 << 17;
pub const DEPCTL_EPTYPE_SHIFT: u32 = 18;
pub const DEPCTL_EPTYPE_MASK: u32 = 0b11 << DEPCTL_EPTYPE_SHIFT;
pub const DEPCTL_STALL: u32 = 1 << 21;
pub const DEPCTL_TXFNUM_SHIFT: u32 = 22;
pub const DEPCTL_TXFNUM_MASK: u32 = 0xf << DEPCTL_TXFNUM_SHIFT;
pub const DEPCTL_CNAK: u32 = 1 << 26;
pub const DEPCTL_SNAK: u32 = 1 << 27;
pub const DEPCTL_SETD0PID: u32 = 1 << 28;
pub const DEPCTL_SETD1PID: u32 = 1 << 29;
pub const DEPCTL_EPDIS: u32 = 1 << 30;
pub const DEPCTL_EPENA: u32 = 1 << 31;

// Synopsys DWC2 core identifier.
pub const GSNPSID_OTG_MASK: u32 = 0xffff_0000;
pub const GSNPSID_OTG_VALUE: u32 = 0x4f54_0000;

// Additional device-mode definitions used by endpoint zero enumeration.

// GAHBCFG.
pub const GAHBCFG_DMA_EN: u32 = 1 << 5;

// FIFO sizing fields (GRXFSIZ / GNPTXFSIZ / DIEPTXFn use 32-bit words).
pub const FIFOSIZE_STARTADDR_MASK: u32 = 0xffff;
pub const FIFOSIZE_DEPTH_SHIFT: u32 = 16;
pub const FIFOSIZE_DEPTH_MASK: u32 = 0xffff << FIFOSIZE_DEPTH_SHIFT;

// DSTS.
pub const DSTS_ENUMSPD_SHIFT: u32 = 1;
pub const DSTS_ENUMSPD_MASK: u32 = 0b11 << DSTS_ENUMSPD_SHIFT;
pub const DSTS_ENUMSPD_HIGH: u32 = 0;
pub const DSTS_ENUMSPD_FULL_30_60: u32 = 1;
pub const DSTS_ENUMSPD_LOW_6: u32 = 2;
pub const DSTS_ENUMSPD_FULL_48: u32 = 3;

// Endpoint-zero maximum packet size uses a special two-bit encoding.
pub const DEPCTL0_MPS_MASK: u32 = 0b11;
pub const DEPCTL0_MPS_64: u32 = 0;
pub const DEPCTL0_MPS_32: u32 = 1;
pub const DEPCTL0_MPS_16: u32 = 2;
pub const DEPCTL0_MPS_8: u32 = 3;

// Endpoint interrupt bits.
pub const DIEPINT_XFRC: u32 = 1 << 0;
pub const DIEPINT_EPDISD: u32 = 1 << 1;
pub const DIEPINT_TOC: u32 = 1 << 3;

pub const DOEPINT_XFRC: u32 = 1 << 0;
pub const DOEPINT_EPDISD: u32 = 1 << 1;
pub const DOEPINT_STUP: u32 = 1 << 3;

// Endpoint-zero transfer size fields.
pub const DEPTSIZ0_XFERSIZE_MASK: u32 = 0x7f;
pub const DEPTSIZ0_PKTCNT_SHIFT: u32 = 19;
pub const DEPTSIZ0_PKTCNT_ONE: u32 = 1 << DEPTSIZ0_PKTCNT_SHIFT;
pub const DOEPTSIZ0_SUPCNT_SHIFT: u32 = 29;
pub const DOEPTSIZ0_SUPCNT_ONE: u32 = 1 << DOEPTSIZ0_SUPCNT_SHIFT;
pub const DOEPTSIZ0_SUPCNT_THREE: u32 = 3 << DOEPTSIZ0_SUPCNT_SHIFT;

// DAINT/DAINTMSK endpoint-zero summary bits.
pub const DAINT_IN_EP0: u32 = 1 << 0;
pub const DAINT_OUT_EP0: u32 = 1 << 16;

// Receive status pop fields.
pub const GRXSTSP_EPNUM_MASK: u32 = 0x0f;
pub const GRXSTSP_BCNT_SHIFT: u32 = 4;
pub const GRXSTSP_BCNT_MASK: u32 = 0x7ff << GRXSTSP_BCNT_SHIFT;
pub const GRXSTSP_PKTSTS_SHIFT: u32 = 17;
pub const GRXSTSP_PKTSTS_MASK: u32 = 0x0f << GRXSTSP_PKTSTS_SHIFT;

pub const PKTSTS_OUT_DATA: u32 = 0x2;
pub const PKTSTS_OUT_COMPLETE: u32 = 0x3;
pub const PKTSTS_SETUP_COMPLETE: u32 = 0x4;
pub const PKTSTS_SETUP_DATA: u32 = 0x6;

// Endpoint-zero TX FIFO free-space field, in 32-bit words.
pub const DTXFSTS_INEPTFSAV_MASK: u32 = 0xffff;

// Non-control endpoint types.
pub const DEPCTL_EPTYPE_CONTROL: u32 = 0 << DEPCTL_EPTYPE_SHIFT;
pub const DEPCTL_EPTYPE_ISOCHRONOUS: u32 = 1 << DEPCTL_EPTYPE_SHIFT;
pub const DEPCTL_EPTYPE_BULK: u32 = 2 << DEPCTL_EPTYPE_SHIFT;
pub const DEPCTL_EPTYPE_INTERRUPT: u32 = 3 << DEPCTL_EPTYPE_SHIFT;

// Generic non-control endpoint transfer-size fields.
pub const DEPTSIZ_XFERSIZE_MASK: u32 = 0x7ffff;
pub const DEPTSIZ_PKTCNT_SHIFT: u32 = 19;
pub const DEPTSIZ_PKTCNT_MASK: u32 = 0x3ff << DEPTSIZ_PKTCNT_SHIFT;
pub const DEPTSIZ_PKTCNT_ONE: u32 = 1 << DEPTSIZ_PKTCNT_SHIFT;

// Additional endpoint interrupt bits.
pub const DIEPINT_TXFE: u32 = 1 << 7;
pub const DOEPINT_B2BSTUP: u32 = 1 << 6;

// DSTS.
pub const DSTS_SUSPSTS: u32 = 1 << 0;

#[inline(always)]
pub const fn daint_in(endpoint: usize) -> u32 {
    1u32 << endpoint
}

#[inline(always)]
pub const fn daint_out(endpoint: usize) -> u32 {
    1u32 << (16 + endpoint)
}

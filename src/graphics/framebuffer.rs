use core::cell::UnsafeCell;

use crate::{
    drivers::mailbox::{
        MAILBOX, MBOX_CH_PROP, MBOX_REQUEST, MBOX_TAG_GETFB, MBOX_TAG_GETPITCH, MBOX_TAG_LAST,
        MBOX_TAG_SETALPHAMODE, MBOX_TAG_SETDEPTH, MBOX_TAG_SETPHYWH, MBOX_TAG_SETPXLORDR,
        MBOX_TAG_SETVIRTOFF, MBOX_TAG_SETVIRTWH,
    },
    early_timeit, warn,
};

use crate::graphics::surface::{PixelFormat, SurfaceError, SurfaceInfo, SurfaceMut};

pub struct Framebuffer {
    width: u32,
    height: u32,
    pitch: u32,
    pixel_format: PixelFormat,

    /// Size of the allocation returned by MBOX_TAG_GETFB.
    buffer_size: usize,
    buffer: *mut u8,
}

pub struct FramebufferStatic(UnsafeCell<Framebuffer>);

unsafe impl Sync for Framebuffer {}
unsafe impl Sync for FramebufferStatic {}

pub static FRAMEBUFFER: FramebufferStatic = FramebufferStatic(UnsafeCell::new(Framebuffer::new()));

impl FramebufferStatic {
    pub fn get(&self) -> &mut Framebuffer {
        unsafe { &mut *self.0.get() }
    }
}

impl Framebuffer {
    pub const fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            pitch: 0,
            pixel_format: PixelFormat::None,
            buffer_size: 0,
            buffer: core::ptr::null_mut(),
        }
    }

    pub fn init(&mut self) {
        {
            let mbox = MAILBOX.get();
            let mbox_buf = &mut mbox.buffer;

            mbox_buf[0] = 39 * 4; // Length of message in bytes.
            mbox_buf[1] = MBOX_REQUEST;

            mbox_buf[2] = MBOX_TAG_SETPHYWH;
            mbox_buf[3] = 8;
            mbox_buf[4] = 0;
            mbox_buf[5] = 1366;
            mbox_buf[6] = 768;

            mbox_buf[7] = MBOX_TAG_SETVIRTWH;
            mbox_buf[8] = 8;
            mbox_buf[9] = 8;
            mbox_buf[10] = 1366;
            mbox_buf[11] = 768;

            mbox_buf[12] = MBOX_TAG_SETVIRTOFF;
            mbox_buf[13] = 8;
            mbox_buf[14] = 8;
            mbox_buf[15] = 0;
            mbox_buf[16] = 0;

            mbox_buf[17] = MBOX_TAG_SETDEPTH;
            mbox_buf[18] = 4;
            mbox_buf[19] = 4;
            mbox_buf[20] = 32;

            mbox_buf[21] = MBOX_TAG_SETPXLORDR;
            mbox_buf[22] = 4;
            mbox_buf[23] = 4;
            mbox_buf[24] = 1; // RGB.

            mbox_buf[25] = MBOX_TAG_GETFB;
            mbox_buf[26] = 8;
            mbox_buf[27] = 8;
            mbox_buf[28] = 4096; // Requested framebuffer alignment.
            mbox_buf[29] = 0;

            mbox_buf[30] = MBOX_TAG_GETPITCH;
            mbox_buf[31] = 4;
            mbox_buf[32] = 4;
            mbox_buf[33] = 0;

            mbox_buf[34] = MBOX_TAG_SETALPHAMODE;
            mbox_buf[35] = 4;
            mbox_buf[36] = 0;
            mbox_buf[37] = 2; // Alpha ignored

            mbox_buf[38] = MBOX_TAG_LAST;
        }

        let mbox = MAILBOX.get();
        let result = early_timeit!("Mailbox framebuffer call", { mbox.call(MBOX_CH_PROP) });

        if result && mbox.buffer[20] == 32 && mbox.buffer[28] != 0 {
            let pixel_format = match mbox.buffer[24] {
                1 => PixelFormat::Rgbx8888,
                0 => PixelFormat::Bgrx8888,
                value => {
                    warn!("Unsupported framebuffer pixel order: {}", value);
                    self.reset();
                    return;
                }
            };

            // Convert the VideoCore bus address to an ARM physical address.
            let buffer_address = mbox.buffer[28] & 0x3fff_ffff;
            let buffer_size = mbox.buffer[29] as usize;

            self.width = mbox.buffer[5];
            self.height = mbox.buffer[6];
            self.pitch = mbox.buffer[33];
            self.pixel_format = pixel_format;
            self.buffer_size = buffer_size;
            self.buffer = buffer_address as *mut u8;

            if let Err(error) = self.as_surface_mut() {
                warn!("Invalid framebuffer surface: {}", error);
                self.reset();
                return;
            }
        } else {
            warn!("No framebuffer available, continuing headless");
            self.reset();
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.width = 0;
        self.height = 0;
        self.pitch = 0;
        self.pixel_format = PixelFormat::None;
        self.buffer_size = 0;
        self.buffer = core::ptr::null_mut();
    }

    #[inline(always)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline(always)]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline(always)]
    pub fn pitch(&self) -> u32 {
        self.pitch
    }

    #[inline(always)]
    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    #[inline(always)]
    pub fn buffer(&self) -> *mut u8 {
        self.buffer
    }

    #[inline(always)]
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    #[inline(always)]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if self.buffer.is_null() || x >= self.width || y >= self.height {
            return;
        }

        let offset = y as usize * self.pitch as usize + x as usize * size_of::<u32>();
        let packed = self.pack_pixel(color);

        unsafe {
            self.buffer.add(offset).cast::<u32>().write(packed);
        }
    }

    pub fn draw_rect(&mut self, x1: u32, y1: u32, x2: u32, y2: u32, color: u32, fill: bool) {
        if self.buffer.is_null() {
            return;
        }

        let max_x = self.width.saturating_sub(1);
        let max_y = self.height.saturating_sub(1);

        let x1 = x1.min(max_x);
        let x2 = x2.min(max_x);
        let y1 = y1.min(max_y);
        let y2 = y2.min(max_y);

        for y in y1..=y2 {
            for x in x1..=x2 {
                if fill || x == x1 || x == x2 || y == y1 || y == y2 {
                    self.set_pixel(x, y, color);
                }
            }
        }
    }

    pub fn draw_line(&mut self, x1: u32, y1: u32, x2: u32, y2: u32, color: u32) {
        if self.buffer.is_null() {
            return;
        }

        let dx = (x2 as i32 - x1 as i32).abs();
        let dy = (y2 as i32 - y1 as i32).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut x = x1;
        let mut y = y1;

        loop {
            self.set_pixel(x, y, color);

            if x == x2 && y == y2 {
                break;
            }

            let err2 = err * 2;

            if err2 > -dy {
                err -= dy;
                x = (x as i32 + sx) as u32;
            }

            if err2 < dx {
                err += dx;
                y = (y as i32 + sy) as u32;
            }
        }
    }

    pub fn draw_circle(&mut self, x0: u32, y0: u32, radius: u32, color: u32, fill: bool) {
        if self.buffer.is_null() {
            return;
        }

        let mut x = radius as i32;
        let mut y = 0;
        let mut err = 0;

        while x >= y {
            if fill {
                self.draw_line(
                    (x0 as i32 - y) as u32,
                    (y0 as i32 + x) as u32,
                    (x0 as i32 + y) as u32,
                    (y0 as i32 + x) as u32,
                    color,
                );
                self.draw_line(
                    (x0 as i32 - x) as u32,
                    (y0 as i32 + y) as u32,
                    (x0 as i32 + x) as u32,
                    (y0 as i32 + y) as u32,
                    color,
                );
                self.draw_line(
                    (x0 as i32 - y) as u32,
                    (y0 as i32 - x) as u32,
                    (x0 as i32 + y) as u32,
                    (y0 as i32 - x) as u32,
                    color,
                );
                self.draw_line(
                    (x0 as i32 - x) as u32,
                    (y0 as i32 - y) as u32,
                    (x0 as i32 + x) as u32,
                    (y0 as i32 - y) as u32,
                    color,
                );
            }

            self.set_pixel((x0 as i32 - y) as u32, (y0 as i32 + x) as u32, color);
            self.set_pixel((x0 as i32 + y) as u32, (y0 as i32 + x) as u32, color);
            self.set_pixel((x0 as i32 - x) as u32, (y0 as i32 + y) as u32, color);
            self.set_pixel((x0 as i32 + x) as u32, (y0 as i32 + y) as u32, color);
            self.set_pixel((x0 as i32 - x) as u32, (y0 as i32 - y) as u32, color);
            self.set_pixel((x0 as i32 + x) as u32, (y0 as i32 - y) as u32, color);
            self.set_pixel((x0 as i32 - y) as u32, (y0 as i32 - x) as u32, color);
            self.set_pixel((x0 as i32 + y) as u32, (y0 as i32 - x) as u32, color);

            if err <= 0 {
                y += 1;
                err += 2 * y + 1;
            }

            if err > 0 {
                x -= 1;
                err -= 2 * x + 1;
            }
        }
    }

    /// Copy a rectangular ARGB8888 pixel buffer into the framebuffer.
    pub fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, pixels: &[u8]) {
        if self.buffer.is_null() {
            return;
        }

        let pitch = self.pitch as usize;

        for row in 0..h {
            let fb_y = y + row;

            if fb_y >= self.height {
                break;
            }

            let dst = unsafe {
                self.buffer
                    .add(fb_y as usize * pitch + x as usize * size_of::<u32>())
            };

            let src = &pixels[row as usize * w as usize * size_of::<u32>()..];

            match self.pixel_format() {
                PixelFormat::Rgbx8888 => unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr(),
                        dst,
                        w as usize * size_of::<u32>(),
                    );
                },
                PixelFormat::Bgrx8888 => {
                    for i in 0..w as usize {
                        let source_offset = i * size_of::<u32>();

                        unsafe {
                            *dst.add(source_offset) = src[source_offset + 2];
                            *dst.add(source_offset + 1) = src[source_offset + 1];
                            *dst.add(source_offset + 2) = src[source_offset];
                            *dst.add(source_offset + 3) = src[source_offset + 3];
                        }
                    }
                }
                _ => {
                    warn!(
                        "Unsupported pixel format for blit: {:?}",
                        self.pixel_format()
                    );
                    break;
                }
            }
        }
    }

    fn pack_pixel(&self, color: u32) -> u32 {
        // Input format: 0xRRGGBBAA.
        let [r, g, b, _a] = color.to_be_bytes();

        match self.pixel_format() {
            PixelFormat::Rgbx8888 => u32::from_le_bytes([r, g, b, 0xff]),
            PixelFormat::Bgrx8888 => u32::from_le_bytes([b, g, r, 0xff]),

            _ => {
                unreachable!("invalid framebuffer pixel format")
            }
        }
    }

    pub fn clear(&mut self, color: u32) {
        if self.buffer.is_null() {
            return;
        }

        let pixel = self.pack_pixel(color);
        let width = self.width as usize;
        let height = self.height as usize;
        let pitch = self.pitch as usize;
        let base = self.buffer;

        let bytes = pixel.to_ne_bytes();

        if bytes[0] == bytes[1] && bytes[1] == bytes[2] && bytes[2] == bytes[3] {
            self.clear_same(bytes[0]);
            return;
        }

        unsafe {
            for y in 0..height {
                let row = base.add(y * pitch).cast::<u32>();
                core::slice::from_raw_parts_mut(row, width).fill(pixel);
            }
        }
    }

    /// Clear the framebuffer with a color whose four bytes are identical.
    ///
    /// Examples: `0x00000000` and `0xffffffff`.
    fn clear_same(&mut self, color: u8) {
        if self.buffer.is_null() {
            return;
        }

        let len = self.pitch as usize * self.height as usize;

        unsafe {
            core::ptr::write_bytes(self.buffer, color, len);
        }
    }

    pub fn as_surface_mut(&mut self) -> Result<SurfaceMut<'_>, SurfaceError> {
        if self.buffer.is_null() {
            return Err(SurfaceError::NullBuffer);
        }

        let info = SurfaceInfo::new(
            self.width,
            self.height,
            self.pitch as usize,
            self.pixel_format,
        )?;

        // Safety:
        //
        // - `buffer` and `buffer_size` were returned together by the mailbox;
        // - the framebuffer allocation remains alive while the display is active;
        // - borrowing `&mut self` prevents another safe mutable surface view from
        //   being created through this Framebuffer simultaneously.
        let pixels = unsafe { core::slice::from_raw_parts_mut(self.buffer, self.buffer_size) };

        SurfaceMut::new(pixels, info)
    }
}

#[inline(always)]
const fn size_of<T>() -> usize {
    core::mem::size_of::<T>()
}

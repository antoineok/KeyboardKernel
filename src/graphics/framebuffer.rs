use core::cell::UnsafeCell;

use crate::{
    drivers::mailbox::{
        MAILBOX, MBOX_CH_PROP, MBOX_REQUEST, MBOX_TAG_GETFB, MBOX_TAG_GETPITCH, MBOX_TAG_LAST,
        MBOX_TAG_SETDEPTH, MBOX_TAG_SETPHYWH, MBOX_TAG_SETPXLORDR, MBOX_TAG_SETVIRTOFF,
        MBOX_TAG_SETVIRTWH,
    },
    uart_println,
};

pub struct Framebuffer {
    width: u32,
    height: u32,
    pitch: u32,
    is_rgb: bool,
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
        Framebuffer {
            width: 0,
            height: 0,
            pitch: 0,
            is_rgb: false,
            buffer: core::ptr::null_mut(),
        }
    }

    pub fn init(&mut self) {
        {
            let mbox = MAILBOX.get();
            let mbox_buf = &mut mbox.buffer;
            mbox_buf[0] = 35 * 4; // Length of message in bytes
            mbox_buf[1] = MBOX_REQUEST;

            mbox_buf[2] = MBOX_TAG_SETPHYWH; // Tag identifier
            mbox_buf[3] = 8; // Value size in bytes
            mbox_buf[4] = 0;
            mbox_buf[5] = 1366; // Value(width)
            mbox_buf[6] = 768; // Value(height)

            mbox_buf[7] = MBOX_TAG_SETVIRTWH;
            mbox_buf[8] = 8;
            mbox_buf[9] = 8;
            mbox_buf[10] = 1366;
            mbox_buf[11] = 768;

            mbox_buf[12] = MBOX_TAG_SETVIRTOFF;
            mbox_buf[13] = 8;
            mbox_buf[14] = 8;
            mbox_buf[15] = 0; // Value(x)
            mbox_buf[16] = 0; // Value(y)

            mbox_buf[17] = MBOX_TAG_SETDEPTH;
            mbox_buf[18] = 4;
            mbox_buf[19] = 4;
            mbox_buf[20] = 32; // Bits per pixel

            mbox_buf[21] = MBOX_TAG_SETPXLORDR;
            mbox_buf[22] = 4;
            mbox_buf[23] = 4;
            mbox_buf[24] = 1; // RGB

            mbox_buf[25] = MBOX_TAG_GETFB;
            mbox_buf[26] = 8;
            mbox_buf[27] = 8;
            mbox_buf[28] = 4096; // FrameBufferInfo.pointer
            mbox_buf[29] = 0; // FrameBufferInfo.size

            mbox_buf[30] = MBOX_TAG_GETPITCH;
            mbox_buf[31] = 4;
            mbox_buf[32] = 4;
            mbox_buf[33] = 0; // Bytes per line

            mbox_buf[34] = MBOX_TAG_LAST;
        }

        let mbox = MAILBOX.get();
        if mbox.call(MBOX_CH_PROP) && mbox.buffer[20] == 32 && mbox.buffer[28] != 0 {
            mbox.buffer[28] &= 0x3FFFFFFF; // Convert GPU address to ARM address
            self.width = mbox.buffer[5];
            self.height = mbox.buffer[6];
            self.pitch = mbox.buffer[33];
            self.is_rgb = mbox.buffer[24] == 1;
            self.buffer = mbox.buffer[28] as *mut u8;

            // Clear the framebuffer
            self.clear(0x000000); // Black
            uart_println!(
                "Framebuffer initialized: {}x{} with pitch {}",
                self.width,
                self.height,
                self.pitch
            );
            uart_println!("Framebuffer buffer address: {:p}", self.buffer);
            uart_println!("Framebuffer is RGB: {}", self.is_rgb);
        } else {
            panic!("Failed to initialize framebuffer");
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pitch(&self) -> u32 {
        self.pitch
    }

    pub fn is_rgb(&self) -> bool {
        self.is_rgb
    }

    pub fn buffer(&self) -> *mut u8 {
        self.buffer
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width() && y < self.height() {
            let offset = (y * self.pitch() + x * 4) as usize;
            unsafe {
                let buffer_slice = core::slice::from_raw_parts_mut(
                    self.buffer(),
                    (self.pitch() * self.height()) as usize,
                );
                if self.is_rgb() {
                    buffer_slice[offset] = ((color >> 16) & 0xFF) as u8; // Red
                    buffer_slice[offset + 1] = ((color >> 8) & 0xFF) as u8; // Green
                    buffer_slice[offset + 2] = (color & 0xFF) as u8; // Blue
                } else {
                    buffer_slice[offset] = (color & 0xFF) as u8; // Blue
                    buffer_slice[offset + 1] = ((color >> 8) & 0xFF) as u8; // Green
                    buffer_slice[offset + 2] = ((color >> 16) & 0xFF) as u8; // Red
                }
                buffer_slice[offset + 3] = ((color >> 24) & 0xFF) as u8; // Alpha
            }
        }
    }

    pub fn draw_rect(&mut self, x1: u32, y1: u32, x2: u32, y2: u32, color: u32, fill: bool) {
        // Clamp rectangle to framebuffer size
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

    /// Copy a rectangular pixel buffer into the framebuffer.
    ///
    /// - `x`, `y`: top-left corner
    /// - `w`, `h`: size
    /// - `pixels`: source buffer (ARGB8888 or RGB8888 depending on config)
    pub fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, pixels: &[u8]) {
        let pitch = self.pitch as usize;

        for row in 0..h {
            let fb_y = y + row;
            if fb_y >= self.height {
                break;
            }

            let dst = unsafe { self.buffer.add((fb_y as usize * pitch) + (x as usize * 4)) };
            let src = &pixels[(row as usize * w as usize * 4)..];

            if self.is_rgb {
                // Fast memcpy
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dst, (w as usize) * 4);
                }
            } else {
                // BGRA framebuffer: swap R/B
                for i in 0..w as usize {
                    let s = i * 4;
                    unsafe {
                        *dst.add(s) = src[s + 2]; // B
                        *dst.add(s + 1) = src[s + 1]; // G
                        *dst.add(s + 2) = src[s]; // R
                        *dst.add(s + 3) = src[s + 3]; // A
                    }
                }
            }
        }
    }

    pub fn clear(&mut self, color: u32) {
        self.draw_rect(0, 0, self.width, self.height, color, true);
    }
}

use crate::graphics::framebuffer::FRAMEBUFFER;

pub unsafe fn early_el1_init() {
    let framebuffer = FRAMEBUFFER.get();
    framebuffer.init();
}

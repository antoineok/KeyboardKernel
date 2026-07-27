pub mod init;

use alloc::vec::Vec;
use core::{hint::spin_loop, time::Duration};

use crate::arch::{
    cpu::{self, CpuId},
    exceptions::PrivilegeLevel,
};

use crate::graphics::{
    graphics_core_ready, BlendMode, GraphicsCoreProxy, GraphicsRequestResult, PixelFormat, Point,
    SourceRect, Surface, SurfaceInfo,
};
use crate::memory::{
    heap::KERNEL_HEAP, phys::PHYS_ALLOCATOR, MemoryRegionKind, MEMORY_MANAGER,
};
use crate::storage::fs::vfs::{FsId, VFS};
use crate::time::time_manager;
use crate::video::kwvideo::VideoTransitionCache;

use crate::{
    early_timeit,
    info,
    services::{KernelManagementBackend, KeyworkServices},
    transport::{usb_core_ready, KeyworkTransport, TransportState, UsbCoreProxy},
    usb::MANAGEMENT_FRAME_CAPACITY,
};

#[cfg(feature = "jtag")]
use crate::jtag::breakpoint;

#[no_mangle]
pub extern "C" fn el1_entry(device_tree_ptr: usize) -> ! {
    // We are now in EL1, with a basic stack (set by EL2 code).
    unsafe {
        early_timeit!("Kernel early EL1 init", {
            init::early_el1_init(device_tree_ptr)
        });

        crate::util::early_timing::flush_early_timings();
    }

    info!("DAIF = {:#x}", read_daif());

    #[cfg(feature = "jtag")]
    breakpoint();

    info!(
        "Current privilege level: {}",
        PrivilegeLevel::current().as_str()
    );
    info!("Current CPU: {}", crate::arch::cpu::current_id());

    bring_up_secondary_cpu(1);
    wait_for_graphics_core();
    bring_up_secondary_cpu(2);
    wait_for_usb_transport_core();

    info!("Timer test, spinning for 1 second");
    time_manager().spin_for(core::time::Duration::from_secs(1));
    info!("1 second elapsed");

    let mut usb_transport = UsbCoreProxy::new();
    let mut services = KeyworkServices::<MANAGEMENT_FRAME_CAPACITY>::new();
    let mut management_backend = KernelManagementBackend::new();

    let vfs = VFS.get();

    let mut video_cache = VideoTransitionCache::new();
    video_cache
        .load(vfs, 0, FsId::Rom, "output.kwv")
        .expect("failed to load output.kwv KWVIDEO test asset");

    let video = video_cache
        .get_mut(0)
        .expect("KWVIDEO test asset disappeared from transition cache");
    let metadata = video.metadata();
    let hevc = metadata.hevc;
    let fragment = metadata.first_fragment;
    let fragment_ms = fragment
        .duration_ticks
        .saturating_mul(1000)
        / metadata.timescale as u64;

    info!(
        "KWVIDEO loaded: name={}, brand={}, bytes={}, bootstrap={} KiB",
        video.stream_name(),
        video.file_type().major_brand(),
        video.file_len(),
        video.bootstrap().len() / 1024,
    );
    info!(
        "KWVIDEO HEVC: track={}, {}x{}, timescale={}, profile={}, level={}, chroma={}, bit_depth={}/{}, nal_length={} bytes, VPS/SPS/PPS={}/{}/{}",
        metadata.track_id,
        metadata.width,
        metadata.height,
        metadata.timescale,
        hevc.profile_idc,
        hevc.level_idc,
        hevc.chroma_format_idc,
        hevc.bit_depth_luma,
        hevc.bit_depth_chroma,
        hevc.nal_length_size,
        hevc.vps_count,
        hevc.sps_count,
        hevc.pps_count,
    );
    info!(
        "KWVIDEO first fragment: decode_time={}, samples={}, duration={} ticks (~{} ms), bytes={}, first_offset={}, first_size={}, sync={}",
        fragment.base_decode_time,
        fragment.sample_count,
        fragment.duration_ticks,
        fragment_ms,
        fragment.total_sample_bytes,
        fragment.first_sample_offset,
        fragment.first_sample_size,
        fragment.first_sample_sync,
    );

    const TEST_FRAGMENT_COUNT: usize = 3;
    let mut fragments = Vec::with_capacity(TEST_FRAGMENT_COUNT);
    fragments.push(
        video
            .cached_first_fragment()
            .expect("failed to materialize cached KWVIDEO fragment"),
    );
    for fragment_index in 1..TEST_FRAGMENT_COUNT {
        let next = video
            .read_next_fragment(vfs)
            .unwrap_or_else(|error| panic!("failed to stream KWVIDEO fragment {fragment_index}: {error}"))
            .unwrap_or_else(|| panic!("KWVIDEO ended before fragment {fragment_index}"));
        fragments.push(next);
    }

    let total_frames: usize = fragments.iter().map(|fragment| fragment.samples().len()).sum();
    let prefetched_bytes: usize = fragments
        .iter()
        .skip(1)
        .flat_map(|fragment| fragment.samples())
        .map(|sample| sample.size as usize)
        .sum();
    info!(
        "KWVIDEO 3-second test ready: fragments={}, frames={}, streamed_sample_bytes={} KiB",
        fragments.len(),
        total_frames,
        prefetched_bytes / 1024,
    );

    let mut graphics = GraphicsCoreProxy::new();
    let playback_started = time_manager().uptime();
    let mut presentation_ticks = 0u64;
    let mut late_frames = 0usize;
    let mut global_picture_index = 0usize;

    for (fragment_index, media_fragment) in fragments.iter().enumerate() {
        info!(
            "KWVIDEO fragment {}/{}: decode_time={}, samples={}, duration={} ticks (~{} ms)",
            fragment_index + 1,
            fragments.len(),
            media_fragment.base_decode_time(),
            media_fragment.samples().len(),
            media_fragment.duration_ticks(),
            media_fragment.duration_ticks().saturating_mul(1000) / metadata.timescale as u64,
        );

        for (picture_index, sample) in media_fragment.samples().iter().copied().enumerate() {
            let sample_bytes = media_fragment
                .sample_bytes(sample)
                .unwrap_or_else(|error| panic!("failed to locate KWVIDEO picture {global_picture_index}: {error}"));
            let picture = video
                .picture_syntax_bytes(sample_bytes)
                .unwrap_or_else(|error| panic!("failed to parse KWVIDEO picture {global_picture_index}: {error}"));
            let slice = picture.slice;
            if picture_index == 0 {
                assert!(
                    matches!(slice.nal_unit_type, 19 | 20) && slice.slice_type == 2,
                    "each kwv1 fragment must begin with an IDR I-slice",
                );
            } else {
                assert_eq!(slice.slice_type, 1, "kwv1 inter pictures must be P slices");
            }

            let raw_cabac_len = video
                .picture_cabac_payload_bytes(sample_bytes, &picture)
                .unwrap_or_else(|error| panic!("failed to locate KWVIDEO picture {global_picture_index} raw CABAC payload: {error}"))
                .len();
            let boundary_epb = video
                .picture_cabac_boundary_has_epb_bytes(sample_bytes, &picture)
                .unwrap_or_else(|error| panic!("failed to inspect KWVIDEO picture {global_picture_index} CABAC boundary: {error}"));
            let cabac_payload = video
                .picture_cabac_rbsp_bytes(sample_bytes, &picture)
                .unwrap_or_else(|error| panic!("failed to materialize KWVIDEO picture {global_picture_index} CABAC RBSP: {error}"));
            let emulation_bytes_removed = raw_cabac_len.saturating_sub(cabac_payload.len());
            let phase1 = crate::video::rpivid::RPIVID_WORKSPACE
                .submit_picture_phase1(&picture, &cabac_payload);
            if !phase1.complete() {
                info!(
                    "RPIVID phase1 failure: picture={}, fragment={}, nal_type={}, slice_type={}, poc_lsb={}, refs={}/{}, merge={}, cabac={} bytes RBSP, epb_removed={}, boundary_epb={}, commands={}, irq={}, CFSTATUS={:#010x}/{:#010x}, STATUS={:#010x}, BFSTATUS={:#010x}, intc={:#010x}",
                    global_picture_index,
                    fragment_index,
                    slice.nal_unit_type,
                    slice.slice_type,
                    slice.pic_order_cnt_lsb,
                    slice.num_ref_idx_l0_active,
                    slice.num_ref_idx_l1_active,
                    slice.max_num_merge_cand,
                    cabac_payload.len(),
                    emulation_bytes_removed,
                    boundary_epb,
                    phase1.command_count,
                    phase1.irq_received,
                    phase1.cfstatus,
                    phase1.cfnum,
                    phase1.status,
                    phase1.bfstatus,
                    phase1.interrupt_control,
                );
            }
            assert!(
                phase1.complete(),
                "RPIVID phase 1 failed for KWVIDEO picture {global_picture_index}",
            );

            let phase2 = crate::video::rpivid::RPIVID_WORKSPACE
                .submit_picture_phase2(&picture, picture_index);
            assert!(
                phase2.complete(),
                "RPIVID phase 2 failed for KWVIDEO picture {global_picture_index}",
            );

            let decoded = crate::video::rpivid::RPIVID_WORKSPACE
                .copy_frame_rgbx(&picture, picture_index);
            let (rgb_checksum, center_pixel) = rgbx_signature_and_center(&decoded);
            assert_eq!(
                decoded.width(),
                metadata.width.into(),
                "SPS crop width disagrees with MP4 metadata",
            );
            assert_eq!(
                decoded.height(),
                metadata.height.into(),
                "SPS crop height disagrees with MP4 metadata",
            );
            let decoded_info = SurfaceInfo::new(
                decoded.width(),
                decoded.height(),
                decoded.pitch(),
                PixelFormat::Rgbx8888,
            )
            .expect("failed to describe decoded RPIVID frame");
            let decoded_surface = Surface::new(decoded.pixels(), decoded_info)
                .expect("failed to expose decoded RPIVID frame as a surface");

            // Decode ahead, then present against one continuous media clock
            // spanning all three test fragments.
            let target_ns = ((u128::from(presentation_ticks) * 1_000_000_000u128)
                / u128::from(metadata.timescale)) as u64;
            let target = Duration::from_nanos(target_ns);
            loop {
                let elapsed = time_manager().uptime().saturating_sub(playback_started);
                if elapsed >= target {
                    if elapsed > target + Duration::from_millis(2) {
                        late_frames += 1;
                    }
                    break;
                }
                spin_loop();
            }

            let request_id = graphics
                .submit_image(
                    &decoded_surface,
                    SourceRect::whole(&decoded_surface),
                    Point::new(0, 0),
                    BlendMode::Opaque,
                    0x000000ff,
                )
                .expect("failed to submit decoded KWVIDEO frame to CPU1");
            wait_for_graphics_request(&mut graphics, request_id);

            info!(
                "KWVIDEO frame {}/{}: fragment={}, type={}, POC={}, sample={} bytes, CABAC={} bytes RBSP, epb_removed={}, boundary_epb={}, P1={}/{}, P2={}, yuv_checksum={:#018x}, rgb_checksum={:#018x}, center=#{:02x}{:02x}{:02x}",
                global_picture_index + 1,
                total_frames,
                fragment_index + 1,
                slice.slice_type,
                slice.pic_order_cnt_lsb,
                sample.size,
                cabac_payload.len(),
                emulation_bytes_removed,
                boundary_epb,
                phase1.cfstatus,
                phase1.cfnum,
                phase2.status2,
                phase2.output_checksum,
                rgb_checksum,
                center_pixel[0],
                center_pixel[1],
                center_pixel[2],
            );

            presentation_ticks = presentation_ticks
                .checked_add(sample.duration_ticks as u64)
                .expect("KWVIDEO presentation clock overflow");
            global_picture_index += 1;
        }
    }

    let playback_elapsed = time_manager().uptime().saturating_sub(playback_started);
    info!(
        "KWVIDEO 3-second test played: fragments={}, frames={}, media={} ticks (~{} ms), elapsed={} ms, late_frames={}",
        fragments.len(),
        global_picture_index,
        presentation_ticks,
        presentation_ticks.saturating_mul(1000) / metadata.timescale as u64,
        playback_elapsed.as_millis(),
        late_frames,
    );

    log_memory_summary();

    loop {
        let transition = services
            .update_transport_selection(usb_transport.state(), TransportState::Disconnected);
        if transition.changed() {
            info!(
                "Active host transport changed: {:?} -> {:?}",
                transition.previous, transition.current
            );
        }
        management_backend.set_active_transport(services.active_transport());
        services
            .poll_selected_transport(&mut usb_transport, &mut management_backend)
            .expect("USB service polling failed");
        management_backend.poll_pending_action();
    }
}

fn wait_for_graphics_core() {
    const GRAPHICS_CORE_BOOT_TIMEOUT: Duration = Duration::from_millis(100);

    let start = time_manager().uptime();
    while !graphics_core_ready() {
        if time_manager().uptime().saturating_sub(start) >= GRAPHICS_CORE_BOOT_TIMEOUT {
            panic!("timed out waiting for CPU1 graphics worker");
        }
        spin_loop();
    }

    info!("CPU1 graphics worker handshake complete");
}

fn wait_for_graphics_request(graphics: &mut GraphicsCoreProxy, request_id: u64) {
    const GRAPHICS_RENDER_TIMEOUT: Duration = Duration::from_secs(2);

    let start = time_manager().uptime();
    loop {
        if let Some(result) = graphics.request_result(request_id) {
            assert_eq!(
                result,
                GraphicsRequestResult::Complete,
                "CPU1 graphics request {request_id} failed: {result:?}"
            );
            return;
        }

        if time_manager().uptime().saturating_sub(start) >= GRAPHICS_RENDER_TIMEOUT {
            panic!("timed out waiting for CPU1 graphics request {request_id}");
        }

        spin_loop();
    }
}

fn wait_for_usb_transport_core() {
    const USB_CORE_BOOT_TIMEOUT: Duration = Duration::from_millis(100);

    let start = time_manager().uptime();
    while !usb_core_ready() {
        if time_manager().uptime().saturating_sub(start) >= USB_CORE_BOOT_TIMEOUT {
            panic!("timed out waiting for CPU2 USB transport worker");
        }
        spin_loop();
    }

    info!("CPU2 USB transport worker is ready");
}

fn bring_up_secondary_cpu(raw_id: u8) {
    const SECONDARY_BOOT_TIMEOUT: Duration = Duration::from_millis(100);

    let cpu = CpuId::new(raw_id).expect("BCM2711 secondary CPU ID must be valid");
    info!("Releasing {} for secondary EL1 bring-up", cpu);

    assert!(
        cpu::release_secondary(cpu),
        "refused to release secondary CPU"
    );

    let start = time_manager().uptime();

    loop {
        let status = cpu::secondary_status(cpu);

        if status.online {
            info!(
                "{} online: EL1={}, MMU={}, D-cache={}",
                cpu, status.at_el1, status.mmu_enabled, status.data_cache_enabled
            );

            assert!(status.at_el1, "{} did not enter EL1", cpu);
            assert!(status.mmu_enabled, "{} failed to enable its EL1 MMU", cpu);
            assert!(
                status.data_cache_enabled,
                "{} failed to enable its EL1 data cache",
                cpu
            );
            return;
        }

        if time_manager().uptime().saturating_sub(start) >= SECONDARY_BOOT_TIMEOUT {
            panic!("timed out waiting for {} secondary bring-up", cpu);
        }

        spin_loop();
    }
}

fn rgbx_signature_and_center(
    decoded: &crate::video::rpivid::RpividDecodedFrame,
) -> (u64, [u8; 4]) {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in decoded.pixels() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    let center_x = decoded.width() as usize / 2;
    let center_y = decoded.height() as usize / 2;
    let offset = center_y
        .checked_mul(decoded.pitch())
        .and_then(|row| row.checked_add(center_x * 4))
        .expect("decoded center-pixel offset overflow");
    let pixels = decoded.pixels();
    assert!(offset + 4 <= pixels.len(), "decoded center pixel is out of bounds");

    (
        hash,
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ],
    )
}

fn log_memory_summary() {
    const MIB: u64 = 1024 * 1024;

    let map = MEMORY_MANAGER.get().map();
    let tracked = map.total_bytes();
    let usable = map.bytes_of_kind(MemoryRegionKind::Usable);
    let reserved = map.bytes_of_kind(MemoryRegionKind::Reserved);
    let dma = map.bytes_of_kind(MemoryRegionKind::Dma);
    let no_map = map.bytes_of_kind(MemoryRegionKind::NoMap);

    info!(
        "Memory map: tracked={} MiB, usable={} MiB, reserved={} MiB, DMA={} MiB, no-map={} MiB",
        tracked / MIB,
        usable / MIB,
        reserved / MIB,
        dma / MIB,
        no_map / MIB,
    );

    let phys = PHYS_ALLOCATOR.get();
    if let (Some((arena_start, arena_end, arena_bytes)), Some(free_bytes)) =
        (phys.info(), phys.free_bytes())
    {
        info!(
            "Physical allocator: arena={:#018x}..{:#018x} ({} MiB), free={} MiB",
            arena_start,
            arena_end,
            arena_bytes / MIB,
            free_bytes / MIB,
        );
    }

    let heap = KERNEL_HEAP.get();
    if let (Some(capacity), Some(used), Some(free)) =
        (heap.capacity_bytes(), heap.used_bytes(), heap.free_bytes())
    {
        info!(
            "Kernel heap: capacity={} MiB, used={} KiB, free={} KiB",
            (capacity as u64) / MIB,
            used / 1024,
            free / 1024,
        );
    }
}

#[inline(always)]
pub fn read_daif() -> u64 {
    let value: u64;

    unsafe {
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}

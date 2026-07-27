pub mod init;

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
        .get(0)
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

    let samples = video
        .first_fragment_samples()
        .expect("failed to enumerate first KWVIDEO fragment samples");
    assert_eq!(
        samples.len(),
        fragment.sample_count as usize,
        "KWVIDEO fragment sample table changed after validation",
    );
    assert!(!samples.is_empty(), "KWVIDEO first fragment is empty");

    let mut graphics = GraphicsCoreProxy::new();
    let playback_started = time_manager().uptime();
    let mut presentation_ticks = 0u64;
    let mut late_frames = 0usize;

    for (picture_index, sample) in samples.iter().copied().enumerate() {
        let picture = video
            .picture_syntax(sample)
            .unwrap_or_else(|error| panic!("failed to parse KWVIDEO picture {picture_index}: {error}"));
        let slice = picture.slice;
        if picture_index == 0 {
            assert!(
                matches!(slice.nal_unit_type, 19 | 20) && slice.slice_type == 2,
                "first KWVIDEO fragment picture must be an IDR I-slice",
            );
        } else {
            assert_eq!(slice.slice_type, 1, "kwv1 inter pictures must be P slices");
        }

        let cabac_payload = video
            .picture_cabac_payload(sample, &picture)
            .unwrap_or_else(|error| panic!("failed to locate KWVIDEO picture {picture_index} CABAC payload: {error}"));
        let phase1 = crate::video::rpivid::RPIVID_WORKSPACE
            .submit_picture_phase1(&picture, cabac_payload);
        assert!(
            phase1.complete(),
            "RPIVID phase 1 failed for KWVIDEO picture {picture_index}",
        );

        let phase2 = crate::video::rpivid::RPIVID_WORKSPACE
            .submit_picture_phase2(&picture, picture_index);
        assert!(
            phase2.complete(),
            "RPIVID phase 2 failed for KWVIDEO picture {picture_index}",
        );

        let decoded = crate::video::rpivid::RPIVID_WORKSPACE
            .copy_frame_rgbx(&picture, picture_index);
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

        // Decode ahead, then present against the fragment's media clock. The
        // first picture is due immediately; each following picture is due at
        // the accumulated duration of the samples before it.
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
            "KWVIDEO frame {}/{}: type={}, POC={}, sample={} bytes, CABAC={} bytes, P1={}/{}, P2={}, checksum={:#018x}",
            picture_index + 1,
            samples.len(),
            slice.slice_type,
            slice.pic_order_cnt_lsb,
            sample.size,
            cabac_payload.len(),
            phase1.cfstatus,
            phase1.cfnum,
            phase2.status2,
            phase2.output_checksum,
        );

        presentation_ticks = presentation_ticks
            .checked_add(sample.duration_ticks as u64)
            .expect("KWVIDEO presentation clock overflow");
    }

    let playback_elapsed = time_manager().uptime().saturating_sub(playback_started);
    info!(
        "KWVIDEO first fragment played: frames={}, media={} ticks (~{} ms), elapsed={} ms, late_frames={}",
        samples.len(),
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

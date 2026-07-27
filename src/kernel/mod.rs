pub mod init;

use core::{hint::spin_loop, time::Duration};

use crate::arch::{
    cpu::{self, CpuId},
    exceptions::PrivilegeLevel,
};

use crate::assets::ImageAsset;
use crate::graphics::{
    graphics_core_ready, BlendMode, GraphicsCoreProxy, GraphicsRequestResult, Point, SourceRect,
};
use crate::memory::{
    dma::DmaBuffer, heap::KERNEL_HEAP, phys::PHYS_ALLOCATOR, MemoryRegionKind, MEMORY_MANAGER,
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

    let first_access_unit = video
        .first_access_unit_annex_b()
        .expect("failed to reconstruct first KWVIDEO HEVC access unit");
    let bitstream_window = crate::video::rpivid::RPIVID_WORKSPACE
        .stage_bitstream(first_access_unit.bytes());
    info!(
        "KWVIDEO first access unit staged: annex_b={} bytes, parameter_sets={}, sample_nals={}, vcl_nals={}, first_vcl_type={}, rpivid_base={:#010x}, rpivid_offset={}, rpivid_len={}",
        first_access_unit.bytes().len(),
        first_access_unit.parameter_set_count(),
        first_access_unit.sample_nal_count(),
        first_access_unit.vcl_nal_count(),
        first_access_unit.first_vcl_type(),
        bitstream_window.base_register_value(),
        bitstream_window.byte_offset(),
        bitstream_window.byte_len(),
    );

    let picture = video
        .first_picture_syntax()
        .expect("failed to parse first KWVIDEO HEVC picture syntax");
    let sps = picture.sps;
    let pps = picture.pps;
    let slice = picture.slice;
    info!(
        "KWVIDEO SPS: coded={}x{}, CTB=2^{} ({}x{} CTBs), min_cb=2^{}, transform=2^{}..2^{}, depth inter/intra={}/{}, amp={}, sao={}, pcm={}, scaling_list={}, tmvp={}, strong_intra_smoothing={}",
        sps.width,
        sps.height,
        sps.log2_ctb_size,
        picture.ctb_width,
        picture.ctb_height,
        sps.log2_min_luma_coding_block_size,
        sps.log2_min_luma_transform_block_size,
        sps.log2_max_luma_transform_block_size,
        sps.max_transform_hierarchy_depth_inter,
        sps.max_transform_hierarchy_depth_intra,
        sps.amp_enabled,
        sps.sao_enabled,
        sps.pcm_enabled,
        sps.scaling_list_enabled,
        sps.temporal_mvp_enabled,
        sps.strong_intra_smoothing_enabled,
    );
    info!(
        "KWVIDEO PPS: id={}, init_qp={}, cb/cr={}/{}, signhide={}, transform_skip={}, cu_qp_delta={} depth={}, constrained_intra={}, tiles={} {}x{}, wpp={}, loop_tile={}, loop_slice={}, deblock_disabled={}",
        pps.pps_id,
        pps.init_qp_minus26 + 26,
        pps.pps_cb_qp_offset,
        pps.pps_cr_qp_offset,
        pps.sign_data_hiding_enabled,
        pps.transform_skip_enabled,
        pps.cu_qp_delta_enabled,
        pps.diff_cu_qp_delta_depth,
        pps.constrained_intra_pred,
        pps.tiles_enabled,
        pps.tile_columns,
        pps.tile_rows,
        pps.entropy_coding_sync_enabled,
        pps.loop_filter_across_tiles_enabled,
        pps.loop_filter_across_slices_enabled,
        pps.pps_deblocking_filter_disabled,
    );
    info!(
        "KWVIDEO IDR slice: nal_type={}, first={}, slice_type={}, qp_delta={}, qp={}, sao={}/{}, chroma_qp={}/{}, deblock_disabled={}, beta/tc={}/{}, loop_slice={}, entry_points={}, entry_bits={}, header_rbsp_bits={}, data_byte_offset={}",
        slice.nal_unit_type,
        slice.first_slice_segment_in_pic,
        slice.slice_type,
        slice.slice_qp_delta,
        pps.init_qp_minus26 + 26 + slice.slice_qp_delta,
        slice.sao_luma,
        slice.sao_chroma,
        slice.slice_cb_qp_offset,
        slice.slice_cr_qp_offset,
        slice.deblocking_filter_disabled,
        slice.beta_offset_div2,
        slice.tc_offset_div2,
        slice.loop_filter_across_slices_enabled,
        slice.entry_point_count,
        slice.entry_point_offset_len,
        slice.header_rbsp_bits,
        slice.data_byte_offset,
    );

    let cabac_payload = video
        .first_picture_cabac_payload(&picture)
        .expect("failed to locate first KWVIDEO CABAC payload");
    let phase1 = crate::video::rpivid::RPIVID_WORKSPACE
        .submit_first_idr_phase1(&picture, cabac_payload);
    info!(
        "RPIVID phase1 IDR: cabac={} bytes, commands={}, irq={}, complete={}, CFSTATUS={:#010x}/{:#010x}, STATUS={:#010x}, BFSTATUS={:#010x}, intc={:#010x}, PU_stride={} bytes, coeff_stride={} bytes",
        cabac_payload.len(),
        phase1.command_count,
        phase1.irq_received,
        phase1.complete(),
        phase1.cfstatus,
        phase1.cfnum,
        phase1.status,
        phase1.bfstatus,
        phase1.interrupt_control,
        phase1.pu_stride,
        phase1.coeff_stride,
    );

    let background_file = vfs
        .open_fs(FsId::Rom, "background.kwim")
        .expect("failed to load background image");
    let background =
        ImageAsset::parse(background_file.as_bytes()).expect("failed to parse background image");
    let background_surface = background.surface();

    let mut graphics = GraphicsCoreProxy::new();
    let request_id = graphics
        .submit_image(
            &background_surface,
            SourceRect::whole(&background_surface),
            Point::new(0, 0),
            BlendMode::Opaque,
            0x000000ff,
        )
        .expect("failed to submit background frame to CPU1");
    wait_for_graphics_request(&mut graphics, request_id);

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

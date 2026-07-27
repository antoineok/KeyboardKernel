//! Minimal HEVC syntax reader used by the bare-metal RPIVID path.
//!
//! RPIVID is a stateless decoder: software must parse the parameter sets and
//! slice header before programming the hardware. This module intentionally
//! extracts the subset of Main-profile syntax needed by the Keywork `kwv1`
//! profile while retaining WPP entry-point information emitted by x265.

use alloc::vec::Vec;
use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpsSummary {
    pub sps_id: u32,
    pub chroma_format_idc: u32,
    pub separate_colour_plane: bool,
    pub width: u32,
    pub height: u32,
    pub crop_left: u32,
    pub crop_right: u32,
    pub crop_top: u32,
    pub crop_bottom: u32,
    pub bit_depth_luma_minus8: u32,
    pub bit_depth_chroma_minus8: u32,
    pub log2_max_pic_order_cnt_lsb: u32,
    pub num_short_term_ref_pic_sets: u32,
    pub long_term_ref_pics_present: bool,
    pub log2_min_luma_coding_block_size: u32,
    pub log2_ctb_size: u32,
    pub log2_min_luma_transform_block_size: u32,
    pub log2_max_luma_transform_block_size: u32,
    pub max_transform_hierarchy_depth_inter: u32,
    pub max_transform_hierarchy_depth_intra: u32,
    pub scaling_list_enabled: bool,
    pub amp_enabled: bool,
    pub sao_enabled: bool,
    pub pcm_enabled: bool,
    pub pcm_loop_filter_disabled: bool,
    pub pcm_sample_bit_depth_luma: u32,
    pub pcm_sample_bit_depth_chroma: u32,
    pub log2_min_pcm_luma_coding_block_size: u32,
    pub log2_max_pcm_luma_coding_block_size: u32,
    pub temporal_mvp_enabled: bool,
    pub strong_intra_smoothing_enabled: bool,
}

impl SpsSummary {
    pub const fn visible_width(self) -> u32 {
        self.width - self.crop_left - self.crop_right
    }

    pub const fn visible_height(self) -> u32 {
        self.height - self.crop_top - self.crop_bottom
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpsSummary {
    pub pps_id: u32,
    pub sps_id: u32,
    pub dependent_slice_segments_enabled: bool,
    pub output_flag_present: bool,
    pub num_extra_slice_header_bits: u8,
    pub sign_data_hiding_enabled: bool,
    pub cabac_init_present: bool,
    pub num_ref_idx_l0_default_active_minus1: u32,
    pub num_ref_idx_l1_default_active_minus1: u32,
    pub init_qp_minus26: i32,
    pub constrained_intra_pred: bool,
    pub transform_skip_enabled: bool,
    pub cu_qp_delta_enabled: bool,
    pub diff_cu_qp_delta_depth: u32,
    pub pps_cb_qp_offset: i32,
    pub pps_cr_qp_offset: i32,
    pub slice_chroma_qp_offsets_present: bool,
    pub weighted_pred: bool,
    pub weighted_bipred: bool,
    pub transquant_bypass_enabled: bool,
    pub tiles_enabled: bool,
    pub entropy_coding_sync_enabled: bool,
    pub tile_columns: u32,
    pub tile_rows: u32,
    pub loop_filter_across_tiles_enabled: bool,
    pub loop_filter_across_slices_enabled: bool,
    pub deblocking_filter_override_enabled: bool,
    pub pps_deblocking_filter_disabled: bool,
    pub pps_beta_offset_div2: i32,
    pub pps_tc_offset_div2: i32,
    pub scaling_list_data_present: bool,
    pub lists_modification_present: bool,
    pub log2_parallel_merge_level_minus2: u32,
    pub slice_segment_header_extension_present: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SliceSummary {
    pub nal_unit_type: u8,
    pub first_slice_segment_in_pic: bool,
    pub no_output_of_prior_pics: bool,
    pub pps_id: u32,
    pub slice_type: u32,
    pub pic_order_cnt_lsb: u32,
    pub num_ref_idx_l0_active: u32,
    pub num_ref_idx_l1_active: u32,
    pub max_num_merge_cand: u32,
    pub cabac_init: bool,
    pub slice_temporal_mvp_enabled: bool,
    pub closest_negative_delta_poc: i32,
    pub sao_luma: bool,
    pub sao_chroma: bool,
    pub slice_qp_delta: i32,
    pub slice_cb_qp_offset: i32,
    pub slice_cr_qp_offset: i32,
    pub deblocking_filter_disabled: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub loop_filter_across_slices_enabled: bool,
    pub entry_point_count: u32,
    pub entry_point_offset_len: u8,
    pub header_rbsp_bits: usize,
    /// Byte offset from the start of the VCL NAL to the first CABAC byte.
    /// This is measured in the original EBSP (emulation-prevention bytes
    /// included), which is the representation consumed by RPIVID.
    pub data_byte_offset: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirstPictureSyntax {
    pub sps: SpsSummary,
    pub pps: PpsSummary,
    pub slice: SliceSummary,
    pub ctb_width: u32,
    pub ctb_height: u32,
    /// Offset of the first VCL NAL within the length-prefixed MP4 sample.
    pub slice_nal_sample_offset: usize,
    pub slice_nal_len: usize,
}

impl FirstPictureSyntax {
    pub fn parse(hvcc: &[u8], sample: &[u8], nal_length_size: usize) -> Result<Self, HevcSyntaxError> {
        if nal_length_size != 4 {
            return Err(HevcSyntaxError::Unsupported("only four-byte hvc1 NAL lengths are supported"));
        }

        let sps_nal = find_hvcc_nal(hvcc, 33)?;
        let pps_nal = find_hvcc_nal(hvcc, 34)?;
        let slice_nal = find_first_vcl_nal(sample, nal_length_size)?;

        let sps = parse_sps(sps_nal)?;
        let pps = parse_pps(pps_nal)?;
        if pps.sps_id != sps.sps_id {
            return Err(HevcSyntaxError::Invalid("PPS references a different SPS"));
        }
        let slice = parse_slice(slice_nal.bytes, &sps, &pps)?;
        if slice.pps_id != pps.pps_id {
            return Err(HevcSyntaxError::Invalid("slice references a different PPS"));
        }

        let ctb_size = 1u32
            .checked_shl(sps.log2_ctb_size)
            .ok_or(HevcSyntaxError::Invalid("CTB size overflow"))?;
        let ctb_width = sps.width.div_ceil(ctb_size);
        let ctb_height = sps.height.div_ceil(ctb_size);

        Ok(Self {
            sps,
            pps,
            slice,
            ctb_width,
            ctb_height,
            slice_nal_sample_offset: slice_nal.sample_offset,
            slice_nal_len: slice_nal.bytes.len(),
        })
    }
}

fn find_hvcc_nal(hvcc: &[u8], wanted_type: u8) -> Result<&[u8], HevcSyntaxError> {
    if hvcc.len() < 23 {
        return Err(HevcSyntaxError::Truncated("hvcC header"));
    }

    let mut cursor = 23usize;
    for _ in 0..hvcc[22] {
        let array_header = *hvcc.get(cursor).ok_or(HevcSyntaxError::Truncated("hvcC array"))?;
        let nal_type = array_header & 0x3f;
        cursor += 1;
        let count = read_u16(hvcc, cursor)? as usize;
        cursor += 2;
        for _ in 0..count {
            let len = read_u16(hvcc, cursor)? as usize;
            cursor += 2;
            let end = cursor.checked_add(len).ok_or(HevcSyntaxError::Overflow)?;
            let nal = hvcc.get(cursor..end).ok_or(HevcSyntaxError::Truncated("hvcC NAL"))?;
            if nal_type == wanted_type {
                validate_nal_type(nal, wanted_type)?;
                return Ok(nal);
            }
            cursor = end;
        }
    }

    Err(HevcSyntaxError::MissingNal(wanted_type))
}

struct SampleNal<'a> {
    bytes: &'a [u8],
    sample_offset: usize,
}

fn find_first_vcl_nal(
    sample: &[u8],
    nal_length_size: usize,
) -> Result<SampleNal<'_>, HevcSyntaxError> {
    let mut cursor = 0usize;
    while cursor < sample.len() {
        let len = read_u32(sample, cursor)? as usize;
        cursor = cursor
            .checked_add(nal_length_size)
            .ok_or(HevcSyntaxError::Overflow)?;
        if len == 0 {
            return Err(HevcSyntaxError::Invalid("zero-length sample NAL"));
        }
        let nal_start = cursor;
        let end = cursor.checked_add(len).ok_or(HevcSyntaxError::Overflow)?;
        let nal = sample
            .get(cursor..end)
            .ok_or(HevcSyntaxError::Truncated("sample NAL"))?;
        let nal_type = nal_unit_type(nal)?;
        if nal_type <= 31 {
            return Ok(SampleNal {
                bytes: nal,
                sample_offset: nal_start,
            });
        }
        cursor = end;
    }
    Err(HevcSyntaxError::Invalid("sample contains no VCL NAL"))
}

fn parse_sps(nal: &[u8]) -> Result<SpsSummary, HevcSyntaxError> {
    validate_nal_type(nal, 33)?;
    let mut bits = RbspReader::from_nal(nal)?;

    bits.read_bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = bits.read_bits(3)? as u8;
    bits.read_bit()?; // sps_temporal_id_nesting_flag
    skip_profile_tier_level(&mut bits, max_sub_layers_minus1)?;

    let sps_id = bits.read_ue()?;
    let chroma_format_idc = bits.read_ue()?;
    if chroma_format_idc > 3 {
        return Err(HevcSyntaxError::Unsupported("invalid chroma_format_idc"));
    }
    let separate_colour_plane = if chroma_format_idc == 3 { bits.read_bit()? } else { false };
    let width = bits.read_ue()?;
    let height = bits.read_ue()?;
    if width == 0 || height == 0 {
        return Err(HevcSyntaxError::Invalid("zero-sized SPS picture"));
    }

    let (sub_width_c, sub_height_c) = if separate_colour_plane {
        (1, 1)
    } else {
        match chroma_format_idc {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        }
    };
    let (crop_left, crop_right, crop_top, crop_bottom) = if bits.read_bit()? {
        // Conformance-window offsets are expressed in chroma sample units.
        let left = bits.read_ue()?.checked_mul(sub_width_c).ok_or(HevcSyntaxError::Overflow)?;
        let right = bits.read_ue()?.checked_mul(sub_width_c).ok_or(HevcSyntaxError::Overflow)?;
        let top = bits.read_ue()?.checked_mul(sub_height_c).ok_or(HevcSyntaxError::Overflow)?;
        let bottom = bits.read_ue()?.checked_mul(sub_height_c).ok_or(HevcSyntaxError::Overflow)?;
        (left, right, top, bottom)
    } else {
        (0, 0, 0, 0)
    };
    if crop_left.checked_add(crop_right).ok_or(HevcSyntaxError::Overflow)? >= width
        || crop_top.checked_add(crop_bottom).ok_or(HevcSyntaxError::Overflow)? >= height
    {
        return Err(HevcSyntaxError::Invalid("HEVC conformance crop removes the whole picture"));
    }

    let bit_depth_luma_minus8 = bits.read_ue()?;
    let bit_depth_chroma_minus8 = bits.read_ue()?;
    let log2_max_pic_order_cnt_lsb = bits.read_ue()?.checked_add(4).ok_or(HevcSyntaxError::Overflow)?;

    let sub_layer_ordering_info_present = bits.read_bit()?;
    let first_layer = if sub_layer_ordering_info_present { 0 } else { max_sub_layers_minus1 as usize };
    for _ in first_layer..=max_sub_layers_minus1 as usize {
        bits.read_ue()?; // sps_max_dec_pic_buffering_minus1
        bits.read_ue()?; // sps_max_num_reorder_pics
        bits.read_ue()?; // sps_max_latency_increase_plus1
    }

    let log2_min_luma_coding_block_size = bits.read_ue()?.checked_add(3).ok_or(HevcSyntaxError::Overflow)?;
    let log2_ctb_size = log2_min_luma_coding_block_size
        .checked_add(bits.read_ue()?)
        .ok_or(HevcSyntaxError::Overflow)?;
    let log2_min_luma_transform_block_size = bits.read_ue()?.checked_add(2).ok_or(HevcSyntaxError::Overflow)?;
    let log2_max_luma_transform_block_size = log2_min_luma_transform_block_size
        .checked_add(bits.read_ue()?)
        .ok_or(HevcSyntaxError::Overflow)?;
    let max_transform_hierarchy_depth_inter = bits.read_ue()?;
    let max_transform_hierarchy_depth_intra = bits.read_ue()?;

    let scaling_list_enabled = bits.read_bit()?;
    if scaling_list_enabled && bits.read_bit()? {
        skip_scaling_list_data(&mut bits)?;
    }

    let amp_enabled = bits.read_bit()?;
    let sao_enabled = bits.read_bit()?;
    let pcm_enabled = bits.read_bit()?;
    let mut pcm_loop_filter_disabled = false;
    let mut pcm_sample_bit_depth_luma = 0;
    let mut pcm_sample_bit_depth_chroma = 0;
    let mut log2_min_pcm_luma_coding_block_size = 0;
    let mut log2_max_pcm_luma_coding_block_size = 0;
    if pcm_enabled {
        pcm_sample_bit_depth_luma = bits.read_bits(4)? + 1;
        pcm_sample_bit_depth_chroma = bits.read_bits(4)? + 1;
        log2_min_pcm_luma_coding_block_size = bits.read_ue()?.checked_add(3).ok_or(HevcSyntaxError::Overflow)?;
        log2_max_pcm_luma_coding_block_size = log2_min_pcm_luma_coding_block_size
            .checked_add(bits.read_ue()?)
            .ok_or(HevcSyntaxError::Overflow)?;
        pcm_loop_filter_disabled = bits.read_bit()?;
    }

    let num_short_term_ref_pic_sets = bits.read_ue()? as usize;
    let mut num_delta_pocs = Vec::with_capacity(num_short_term_ref_pic_sets);
    for st_rps_idx in 0..num_short_term_ref_pic_sets {
        let count = skip_short_term_ref_pic_set(&mut bits, st_rps_idx, &num_delta_pocs)?;
        num_delta_pocs.push(count);
    }

    let long_term_ref_pics_present = bits.read_bit()?;
    if long_term_ref_pics_present {
        let num_long_term_ref_pics_sps = bits.read_ue()?;
        for _ in 0..num_long_term_ref_pics_sps {
            bits.read_bits(log2_max_pic_order_cnt_lsb)?;
            bits.read_bit()?;
        }
    }

    let temporal_mvp_enabled = bits.read_bit()?;
    let strong_intra_smoothing_enabled = bits.read_bit()?;

    Ok(SpsSummary {
        sps_id,
        chroma_format_idc,
        separate_colour_plane,
        width,
        height,
        crop_left,
        crop_right,
        crop_top,
        crop_bottom,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        log2_max_pic_order_cnt_lsb,
        num_short_term_ref_pic_sets: num_short_term_ref_pic_sets as u32,
        long_term_ref_pics_present,
        log2_min_luma_coding_block_size,
        log2_ctb_size,
        log2_min_luma_transform_block_size,
        log2_max_luma_transform_block_size,
        max_transform_hierarchy_depth_inter,
        max_transform_hierarchy_depth_intra,
        scaling_list_enabled,
        amp_enabled,
        sao_enabled,
        pcm_enabled,
        pcm_loop_filter_disabled,
        pcm_sample_bit_depth_luma,
        pcm_sample_bit_depth_chroma,
        log2_min_pcm_luma_coding_block_size,
        log2_max_pcm_luma_coding_block_size,
        temporal_mvp_enabled,
        strong_intra_smoothing_enabled,
    })
}

fn parse_pps(nal: &[u8]) -> Result<PpsSummary, HevcSyntaxError> {
    validate_nal_type(nal, 34)?;
    let mut bits = RbspReader::from_nal(nal)?;

    let pps_id = bits.read_ue()?;
    let sps_id = bits.read_ue()?;
    let dependent_slice_segments_enabled = bits.read_bit()?;
    let output_flag_present = bits.read_bit()?;
    let num_extra_slice_header_bits = bits.read_bits(3)? as u8;
    let sign_data_hiding_enabled = bits.read_bit()?;
    let cabac_init_present = bits.read_bit()?;
    let num_ref_idx_l0_default_active_minus1 = bits.read_ue()?;
    let num_ref_idx_l1_default_active_minus1 = bits.read_ue()?;
    let init_qp_minus26 = bits.read_se()?;
    let constrained_intra_pred = bits.read_bit()?;
    let transform_skip_enabled = bits.read_bit()?;
    let cu_qp_delta_enabled = bits.read_bit()?;
    let diff_cu_qp_delta_depth = if cu_qp_delta_enabled { bits.read_ue()? } else { 0 };
    let pps_cb_qp_offset = bits.read_se()?;
    let pps_cr_qp_offset = bits.read_se()?;
    let slice_chroma_qp_offsets_present = bits.read_bit()?;
    let weighted_pred = bits.read_bit()?;
    let weighted_bipred = bits.read_bit()?;
    let transquant_bypass_enabled = bits.read_bit()?;
    let tiles_enabled = bits.read_bit()?;
    let entropy_coding_sync_enabled = bits.read_bit()?;

    let mut tile_columns = 1;
    let mut tile_rows = 1;
    let mut loop_filter_across_tiles_enabled = true;
    if tiles_enabled {
        let num_tile_columns_minus1 = bits.read_ue()?;
        let num_tile_rows_minus1 = bits.read_ue()?;
        tile_columns = num_tile_columns_minus1.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
        tile_rows = num_tile_rows_minus1.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
        let uniform_spacing = bits.read_bit()?;
        if !uniform_spacing {
            for _ in 0..num_tile_columns_minus1 {
                bits.read_ue()?;
            }
            for _ in 0..num_tile_rows_minus1 {
                bits.read_ue()?;
            }
        }
        loop_filter_across_tiles_enabled = bits.read_bit()?;
    }

    let loop_filter_across_slices_enabled = bits.read_bit()?;
    let deblocking_filter_control_present = bits.read_bit()?;
    let mut deblocking_filter_override_enabled = false;
    let mut pps_deblocking_filter_disabled = false;
    let mut pps_beta_offset_div2 = 0;
    let mut pps_tc_offset_div2 = 0;
    if deblocking_filter_control_present {
        deblocking_filter_override_enabled = bits.read_bit()?;
        pps_deblocking_filter_disabled = bits.read_bit()?;
        if !pps_deblocking_filter_disabled {
            pps_beta_offset_div2 = bits.read_se()?;
            pps_tc_offset_div2 = bits.read_se()?;
        }
    }

    let scaling_list_data_present = bits.read_bit()?;
    if scaling_list_data_present {
        skip_scaling_list_data(&mut bits)?;
    }
    let lists_modification_present = bits.read_bit()?;
    let log2_parallel_merge_level_minus2 = bits.read_ue()?;
    let slice_segment_header_extension_present = bits.read_bit()?;
    let pps_extension_present = bits.read_bit()?;
    if pps_extension_present {
        return Err(HevcSyntaxError::Unsupported("PPS extensions are not supported by kwv1 bring-up"));
    }

    Ok(PpsSummary {
        pps_id,
        sps_id,
        dependent_slice_segments_enabled,
        output_flag_present,
        num_extra_slice_header_bits,
        sign_data_hiding_enabled,
        cabac_init_present,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        init_qp_minus26,
        constrained_intra_pred,
        transform_skip_enabled,
        cu_qp_delta_enabled,
        diff_cu_qp_delta_depth,
        pps_cb_qp_offset,
        pps_cr_qp_offset,
        slice_chroma_qp_offsets_present,
        weighted_pred,
        weighted_bipred,
        transquant_bypass_enabled,
        tiles_enabled,
        entropy_coding_sync_enabled,
        tile_columns,
        tile_rows,
        loop_filter_across_tiles_enabled,
        loop_filter_across_slices_enabled,
        deblocking_filter_override_enabled,
        pps_deblocking_filter_disabled,
        pps_beta_offset_div2,
        pps_tc_offset_div2,
        scaling_list_data_present,
        lists_modification_present,
        log2_parallel_merge_level_minus2,
        slice_segment_header_extension_present,
    })
}

fn parse_slice(nal: &[u8], sps: &SpsSummary, pps: &PpsSummary) -> Result<SliceSummary, HevcSyntaxError> {
    let nal_unit_type = nal_unit_type(nal)?;
    if nal_unit_type > 31 {
        return Err(HevcSyntaxError::Invalid("slice NAL is not VCL"));
    }
    let is_irap = (16..=23).contains(&nal_unit_type);
    let is_idr = matches!(nal_unit_type, 19 | 20);
    let mut bits = RbspReader::from_nal(nal)?;

    let first_slice_segment_in_pic = bits.read_bit()?;
    let no_output_of_prior_pics = if is_irap { bits.read_bit()? } else { false };
    let pps_id = bits.read_ue()?;

    if !first_slice_segment_in_pic {
        if pps.dependent_slice_segments_enabled && bits.read_bit()? {
            return Err(HevcSyntaxError::Unsupported(
                "dependent slice segments are not supported by kwv1",
            ));
        }
        let ctb_size = 1u32
            .checked_shl(sps.log2_ctb_size)
            .ok_or(HevcSyntaxError::Overflow)?;
        let ctb_count = sps
            .width
            .div_ceil(ctb_size)
            .checked_mul(sps.height.div_ceil(ctb_size))
            .ok_or(HevcSyntaxError::Overflow)?;
        bits.read_bits(ceil_log2(ctb_count))?;
        return Err(HevcSyntaxError::Unsupported(
            "kwv1 currently requires one slice segment per picture",
        ));
    }

    for _ in 0..pps.num_extra_slice_header_bits {
        bits.read_bit()?;
    }
    let slice_type = bits.read_ue()?;
    if slice_type > 2 {
        return Err(HevcSyntaxError::Invalid("invalid HEVC slice type"));
    }
    if slice_type == 0 {
        return Err(HevcSyntaxError::Unsupported(
            "kwv1 does not permit B slices",
        ));
    }
    if pps.output_flag_present {
        bits.read_bit()?;
    }
    if sps.separate_colour_plane {
        bits.read_bits(2)?;
    }

    let mut pic_order_cnt_lsb = 0u32;
    let mut closest_negative_delta_poc = 0i32;
    if !is_idr {
        pic_order_cnt_lsb = bits.read_bits(sps.log2_max_pic_order_cnt_lsb)?;
        let short_term_ref_pic_set_sps_flag = bits.read_bit()?;
        if short_term_ref_pic_set_sps_flag {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 requires inline short-term reference picture sets",
            ));
        }
        if sps.num_short_term_ref_pic_sets != 0 {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 requires SPS num_short_term_ref_pic_sets == 0",
            ));
        }

        let num_negative_pics = bits.read_ue()?;
        let num_positive_pics = bits.read_ue()?;
        if num_negative_pics == 0 || num_positive_pics != 0 {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 P pictures require past-only short-term references",
            ));
        }
        let mut delta_poc = 0i32;
        let mut used_negative = 0u32;
        for index in 0..num_negative_pics {
            let delta_minus1 = bits.read_ue()? as i32;
            delta_poc = delta_poc
                .checked_sub(delta_minus1 + 1)
                .ok_or(HevcSyntaxError::Overflow)?;
            let used = bits.read_bit()?;
            if used {
                used_negative = used_negative.saturating_add(1);
                if index == 0 {
                    closest_negative_delta_poc = delta_poc;
                }
            }
        }
        for _ in 0..num_positive_pics {
            bits.read_ue()?;
            bits.read_bit()?;
        }
        if used_negative == 0 || closest_negative_delta_poc != -1 {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 requires the immediately previous picture as its primary reference",
            ));
        }
        if sps.long_term_ref_pics_present {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 does not support long-term references",
            ));
        }
    }

    let slice_temporal_mvp_enabled = if sps.temporal_mvp_enabled && !is_idr {
        bits.read_bit()?
    } else {
        false
    };
    if slice_temporal_mvp_enabled || sps.temporal_mvp_enabled {
        return Err(HevcSyntaxError::Unsupported(
            "kwv1 requires temporal MVP to be disabled",
        ));
    }

    let sao_luma = if sps.sao_enabled { bits.read_bit()? } else { false };
    let sao_chroma = if sps.sao_enabled && sps.chroma_format_idc != 0 {
        bits.read_bit()?
    } else {
        false
    };

    let mut num_ref_idx_l0_active = 0u32;
    let mut num_ref_idx_l1_active = 0u32;
    let mut cabac_init = false;
    let mut max_num_merge_cand = 0u32;
    if slice_type != 2 {
        let override_active = bits.read_bit()?;
        num_ref_idx_l0_active = pps
            .num_ref_idx_l0_default_active_minus1
            .checked_add(1)
            .ok_or(HevcSyntaxError::Overflow)?;
        if slice_type == 0 {
            num_ref_idx_l1_active = pps
                .num_ref_idx_l1_default_active_minus1
                .checked_add(1)
                .ok_or(HevcSyntaxError::Overflow)?;
        }
        if override_active {
            num_ref_idx_l0_active = bits.read_ue()?.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
            if slice_type == 0 {
                num_ref_idx_l1_active = bits.read_ue()?.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
            }
        }
        if num_ref_idx_l0_active != 1 || num_ref_idx_l1_active != 0 {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 currently requires exactly one L0 reference",
            ));
        }
        if pps.lists_modification_present {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 requires reference-list modification to be disabled",
            ));
        }
        if pps.cabac_init_present {
            cabac_init = bits.read_bit()?;
        }
        if cabac_init {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 requires cabac_init_flag to be false",
            ));
        }
        if pps.weighted_pred || pps.weighted_bipred {
            return Err(HevcSyntaxError::Unsupported(
                "kwv1 requires weighted prediction to be disabled",
            ));
        }
        let five_minus_max_num_merge_cand = bits.read_ue()?;
        if five_minus_max_num_merge_cand > 4 {
            return Err(HevcSyntaxError::Invalid("invalid merge-candidate count"));
        }
        max_num_merge_cand = 5 - five_minus_max_num_merge_cand;
    }

    let slice_qp_delta = bits.read_se()?;
    let (slice_cb_qp_offset, slice_cr_qp_offset) = if pps.slice_chroma_qp_offsets_present {
        (bits.read_se()?, bits.read_se()?)
    } else {
        (0, 0)
    };

    let mut deblocking_filter_disabled = pps.pps_deblocking_filter_disabled;
    let mut beta_offset_div2 = pps.pps_beta_offset_div2;
    let mut tc_offset_div2 = pps.pps_tc_offset_div2;
    if pps.deblocking_filter_override_enabled {
        let deblocking_filter_override = bits.read_bit()?;
        if deblocking_filter_override {
            deblocking_filter_disabled = bits.read_bit()?;
            if !deblocking_filter_disabled {
                beta_offset_div2 = bits.read_se()?;
                tc_offset_div2 = bits.read_se()?;
            }
        }
    }

    let loop_filter_across_slices_enabled = if pps.loop_filter_across_slices_enabled
        && (sao_luma || sao_chroma || !deblocking_filter_disabled)
    {
        bits.read_bit()?
    } else {
        false
    };

    let mut entry_point_count = 0;
    let mut entry_point_offset_len = 0;
    if pps.tiles_enabled || pps.entropy_coding_sync_enabled {
        entry_point_count = bits.read_ue()?;
        if entry_point_count != 0 {
            let offset_len_minus1 = bits.read_ue()?;
            if offset_len_minus1 >= 31 {
                return Err(HevcSyntaxError::Unsupported("entry-point offset width is too large"));
            }
            entry_point_offset_len = (offset_len_minus1 + 1) as u8;
            for _ in 0..entry_point_count {
                bits.read_bits(entry_point_offset_len as u32)?;
            }
        }
    }

    if pps.slice_segment_header_extension_present {
        let extension_length = bits.read_ue()? as usize;
        for _ in 0..extension_length {
            bits.read_bits(8)?;
        }
    }

    bits.byte_align()?;
    let header_rbsp_bits = bits.position_bits();
    if header_rbsp_bits & 7 != 0 {
        return Err(HevcSyntaxError::Invalid("slice header did not end on a byte boundary"));
    }
    let data_byte_offset = rbsp_prefix_to_nal_offset(nal, header_rbsp_bits / 8)?;

    Ok(SliceSummary {
        nal_unit_type,
        first_slice_segment_in_pic,
        no_output_of_prior_pics,
        pps_id,
        slice_type,
        pic_order_cnt_lsb,
        num_ref_idx_l0_active,
        num_ref_idx_l1_active,
        max_num_merge_cand,
        cabac_init,
        slice_temporal_mvp_enabled,
        closest_negative_delta_poc,
        sao_luma,
        sao_chroma,
        slice_qp_delta,
        slice_cb_qp_offset,
        slice_cr_qp_offset,
        deblocking_filter_disabled,
        beta_offset_div2,
        tc_offset_div2,
        loop_filter_across_slices_enabled,
        entry_point_count,
        entry_point_offset_len,
        header_rbsp_bits,
        data_byte_offset,
    })
}

fn rbsp_prefix_to_nal_offset(
    nal: &[u8],
    rbsp_bytes: usize,
) -> Result<usize, HevcSyntaxError> {
    if nal.len() < 2 {
        return Err(HevcSyntaxError::Truncated("HEVC NAL header"));
    }

    let mut source = 2usize;
    let mut emitted = 0usize;
    let mut zero_run = 0u8;
    while emitted < rbsp_bytes {
        let byte = *nal
            .get(source)
            .ok_or(HevcSyntaxError::Truncated("slice header EBSP"))?;
        source = source.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;

        if zero_run >= 2 && byte == 0x03 {
            zero_run = 0;
            continue;
        }

        emitted = emitted.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
        if byte == 0 {
            zero_run = zero_run.saturating_add(1);
        } else {
            zero_run = 0;
        }
    }

    Ok(source)
}

fn skip_profile_tier_level(bits: &mut RbspReader, max_sub_layers_minus1: u8) -> Result<(), HevcSyntaxError> {
    bits.skip_bits(88)?;
    bits.skip_bits(8)?; // general_level_idc

    let mut profile_present = [false; 8];
    let mut level_present = [false; 8];
    for i in 0..max_sub_layers_minus1 as usize {
        profile_present[i] = bits.read_bit()?;
        level_present[i] = bits.read_bit()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            bits.skip_bits(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if profile_present[i] {
            bits.skip_bits(88)?;
        }
        if level_present[i] {
            bits.skip_bits(8)?;
        }
    }
    Ok(())
}

fn skip_scaling_list_data(bits: &mut RbspReader) -> Result<(), HevcSyntaxError> {
    for size_id in 0..4u32 {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut matrix_id = 0u32;
        while matrix_id < 6 {
            if !bits.read_bit()? {
                bits.read_ue()?;
            } else {
                let coef_num = core::cmp::min(64, 1u32 << (4 + (size_id << 1)));
                if size_id > 1 {
                    bits.read_se()?;
                }
                for _ in 0..coef_num {
                    bits.read_se()?;
                }
            }
            matrix_id += step;
        }
    }
    Ok(())
}

fn skip_short_term_ref_pic_set(
    bits: &mut RbspReader,
    st_rps_idx: usize,
    previous_num_delta_pocs: &[u32],
) -> Result<u32, HevcSyntaxError> {
    let inter_ref_pic_set_prediction = st_rps_idx != 0 && bits.read_bit()?;
    if inter_ref_pic_set_prediction {
        bits.read_bit()?; // delta_rps_sign
        bits.read_ue()?; // abs_delta_rps_minus1
        let ref_count = *previous_num_delta_pocs
            .get(st_rps_idx - 1)
            .ok_or(HevcSyntaxError::Invalid("invalid predicted RPS reference"))?;
        let mut count = 0u32;
        for _ in 0..=ref_count {
            let used_by_curr_pic = bits.read_bit()?;
            let use_delta = if used_by_curr_pic { true } else { bits.read_bit()? };
            if used_by_curr_pic || use_delta {
                count = count.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
            }
        }
        Ok(count)
    } else {
        let negative = bits.read_ue()?;
        let positive = bits.read_ue()?;
        for _ in 0..negative {
            bits.read_ue()?;
            bits.read_bit()?;
        }
        for _ in 0..positive {
            bits.read_ue()?;
            bits.read_bit()?;
        }
        negative.checked_add(positive).ok_or(HevcSyntaxError::Overflow)
    }
}

fn validate_nal_type(nal: &[u8], expected: u8) -> Result<(), HevcSyntaxError> {
    let actual = nal_unit_type(nal)?;
    if actual != expected {
        return Err(HevcSyntaxError::Invalid("unexpected HEVC NAL type"));
    }
    Ok(())
}

fn nal_unit_type(nal: &[u8]) -> Result<u8, HevcSyntaxError> {
    if nal.len() < 2 {
        return Err(HevcSyntaxError::Truncated("HEVC NAL header"));
    }
    if nal[0] & 0x80 != 0 {
        return Err(HevcSyntaxError::Invalid("forbidden_zero_bit is set"));
    }
    if nal[1] & 0x07 == 0 {
        return Err(HevcSyntaxError::Invalid("nuh_temporal_id_plus1 is zero"));
    }
    Ok((nal[0] >> 1) & 0x3f)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, HevcSyntaxError> {
    let end = offset.checked_add(2).ok_or(HevcSyntaxError::Overflow)?;
    let data = bytes.get(offset..end).ok_or(HevcSyntaxError::Truncated("u16"))?;
    Ok(u16::from_be_bytes([data[0], data[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, HevcSyntaxError> {
    let end = offset.checked_add(4).ok_or(HevcSyntaxError::Overflow)?;
    let data = bytes.get(offset..end).ok_or(HevcSyntaxError::Truncated("u32"))?;
    Ok(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}

fn ceil_log2(value: u32) -> u32 {
    if value <= 1 { 0 } else { u32::BITS - (value - 1).leading_zeros() }
}

struct RbspReader {
    bytes: Vec<u8>,
    bit_pos: usize,
}

impl RbspReader {
    fn from_nal(nal: &[u8]) -> Result<Self, HevcSyntaxError> {
        if nal.len() < 2 {
            return Err(HevcSyntaxError::Truncated("HEVC NAL header"));
        }
        let mut bytes = Vec::with_capacity(nal.len() - 2);
        let mut zero_run = 0u8;
        for &byte in &nal[2..] {
            if zero_run >= 2 && byte == 0x03 {
                zero_run = 0;
                continue;
            }
            bytes.push(byte);
            if byte == 0 {
                zero_run = zero_run.saturating_add(1);
            } else {
                zero_run = 0;
            }
        }
        Ok(Self { bytes, bit_pos: 0 })
    }

    fn position_bits(&self) -> usize {
        self.bit_pos
    }

    fn read_bit(&mut self) -> Result<bool, HevcSyntaxError> {
        let byte = *self.bytes.get(self.bit_pos / 8).ok_or(HevcSyntaxError::Truncated("RBSP bit"))?;
        let shift = 7 - (self.bit_pos & 7);
        self.bit_pos = self.bit_pos.checked_add(1).ok_or(HevcSyntaxError::Overflow)?;
        Ok(((byte >> shift) & 1) != 0)
    }

    fn read_bits(&mut self, count: u32) -> Result<u32, HevcSyntaxError> {
        if count > 32 {
            return Err(HevcSyntaxError::Unsupported("bit field wider than 32 bits"));
        }
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()? as u32;
        }
        Ok(value)
    }

    fn skip_bits(&mut self, count: u32) -> Result<(), HevcSyntaxError> {
        let end = self.bit_pos.checked_add(count as usize).ok_or(HevcSyntaxError::Overflow)?;
        if end > self.bytes.len() * 8 {
            return Err(HevcSyntaxError::Truncated("RBSP skip"));
        }
        self.bit_pos = end;
        Ok(())
    }

    fn read_ue(&mut self) -> Result<u32, HevcSyntaxError> {
        let mut leading_zero_bits = 0u32;
        while !self.read_bit()? {
            leading_zero_bits += 1;
            if leading_zero_bits > 31 {
                return Err(HevcSyntaxError::Unsupported("Exp-Golomb value exceeds 32 bits"));
            }
        }
        if leading_zero_bits == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(leading_zero_bits)?;
        Ok(((1u32 << leading_zero_bits) - 1) + suffix)
    }

    fn read_se(&mut self) -> Result<i32, HevcSyntaxError> {
        let code_num = self.read_ue()?;
        let magnitude = ((code_num + 1) >> 1) as i32;
        Ok(if code_num & 1 == 0 { -magnitude } else { magnitude })
    }

    fn byte_align(&mut self) -> Result<(), HevcSyntaxError> {
        if self.bit_pos & 7 == 0 {
            return Ok(());
        }
        if !self.read_bit()? {
            return Err(HevcSyntaxError::Invalid("slice byte_alignment one-bit is zero"));
        }
        while self.bit_pos & 7 != 0 {
            if self.read_bit()? {
                return Err(HevcSyntaxError::Invalid("slice byte_alignment padding is nonzero"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HevcSyntaxError {
    Truncated(&'static str),
    Invalid(&'static str),
    Unsupported(&'static str),
    MissingNal(u8),
    Overflow,
}

impl fmt::Display for HevcSyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "truncated HEVC {what}"),
            Self::Invalid(reason) => write!(f, "invalid HEVC syntax: {reason}"),
            Self::Unsupported(reason) => write!(f, "unsupported HEVC syntax: {reason}"),
            Self::MissingNal(nal_type) => write!(f, "missing HEVC NAL type {nal_type}"),
            Self::Overflow => write!(f, "HEVC syntax value overflow"),
        }
    }
}

//! HEVC picture-order-count unwrapping and decoded-picture-buffer reference planning.

use super::*;

pub(super) struct HevcPocUnwrapper {
    max_poc_lsb: i32,
    prev_tid0_poc_lsb: i32,
    prev_tid0_poc_msb: i32,
}

impl HevcPocUnwrapper {
    pub(super) fn new(log2_max_pic_order_cnt_lsb_minus4: u8) -> Self {
        Self {
            max_poc_lsb: 1_i32 << (i32::from(log2_max_pic_order_cnt_lsb_minus4) + 4),
            prev_tid0_poc_lsb: 0,
            prev_tid0_poc_msb: 0,
        }
    }

    /// Advances unwrapper state and returns the full (unwrapped) POC for the current picture.
    pub(super) fn advance(
        &mut self,
        poc_lsb: Option<u16>,
        nal_unit_type: u8,
        temporal_id: u8,
    ) -> i32 {
        // IDR and BLA pictures always start a new POC sequence at 0.
        let is_idr = matches!(nal_unit_type, 19 | 20);
        let is_bla = matches!(nal_unit_type, 16..=18);
        if is_idr || is_bla {
            if temporal_id == 0 {
                self.prev_tid0_poc_lsb = 0;
                self.prev_tid0_poc_msb = 0;
            }
            return 0;
        }

        let poc_lsb = i32::from(poc_lsb.unwrap_or(0));
        let max = self.max_poc_lsb;
        let prev_lsb = self.prev_tid0_poc_lsb;
        let prev_msb = self.prev_tid0_poc_msb;

        // H.265 spec 8.3.1: PicOrderCntMsb derivation.
        let poc_msb = if (poc_lsb < prev_lsb) && ((prev_lsb - poc_lsb) >= max / 2) {
            prev_msb + max
        } else if (poc_lsb > prev_lsb) && ((poc_lsb - prev_lsb) > max / 2) {
            prev_msb - max
        } else {
            prev_msb
        };
        let poc_full = poc_msb + poc_lsb;

        // Update prevTid0 state: temporal_id == 0 and not RASL/RADL/sub-layer non-reference.
        let is_rasl_or_radl = matches!(nal_unit_type, 6..=9);
        let is_sublayer_non_ref = matches!(nal_unit_type, 0 | 2 | 4 | 10 | 12 | 14);
        if temporal_id == 0 && !is_rasl_or_radl && !is_sublayer_non_ref {
            self.prev_tid0_poc_lsb = poc_lsb;
            self.prev_tid0_poc_msb = poc_msb;
        }
        poc_full
    }
}

/// Converts inline RPS absolute POCs from LSB-space to full (unwrapped) POC space in-place.
///
/// `parse_hevc_inline_short_term_ref_pic_set_usage` always applies `rem_euclid(max_poc_lsb)`,
/// producing POC values in `[0, max_poc_lsb)`.  After POC unwrapping, the DPB stores full POCs,
/// so the inline RPS target POCs must be adjusted to the same domain.
///
/// Before-references have `full_poc < current_poc_full`; after-references have
/// `full_poc > current_poc_full`.  The before/after classification resolves the ambiguity
/// in which epoch each LSB-space value belongs to.
pub(super) fn adjust_inline_rps_pocs_to_full_poc(
    pocs: &mut HevcInlineRefPicSetPocs,
    current_poc_full: i32,
    max_poc_lsb: i32,
) {
    let poc_msb = current_poc_full - current_poc_full.rem_euclid(max_poc_lsb);
    for p in pocs.before[..pocs.before_count].iter_mut() {
        let candidate = poc_msb + *p;
        *p = if candidate < current_poc_full {
            candidate
        } else {
            candidate - max_poc_lsb
        };
    }
    for p in pocs.after[..pocs.after_count].iter_mut() {
        let candidate = poc_msb + *p;
        *p = if candidate > current_poc_full {
            candidate
        } else {
            candidate + max_poc_lsb
        };
    }
}

pub(super) fn is_hevc_idr_nal_type(nal_unit_type: u8) -> bool {
    nal_unit_type == 19 || nal_unit_type == 20
}

pub(super) fn is_hevc_reference_nal_type(nal_unit_type: u8) -> bool {
    matches!(
        nal_unit_type,
        1 | 3 | 5 | 7 | 9 | 11 | 13 | 15 | 16 | 17 | 18 | 19 | 20 | 21
    )
}

pub(super) fn select_hevc_decode_dpb_slot(
    experimental_dpb_enabled: bool,
    is_reference: bool,
    dpb_slot_count: usize,
    next_reference_slot: &mut usize,
    active_reference_slots: &[HevcActiveReferenceSlot],
    selected_references: &[HevcActiveReferenceSlot],
) -> usize {
    if !experimental_dpb_enabled || dpb_slot_count == 0 {
        return 0;
    }
    if is_reference {
        if dpb_slot_count <= 1 {
            return 0;
        }
        let reference_slot_count = dpb_slot_count - 1;
        for _ in 0..reference_slot_count {
            let slot = 1 + (*next_reference_slot % reference_slot_count);
            *next_reference_slot = next_reference_slot.saturating_add(1);
            if !selected_references
                .iter()
                .any(|reference| reference.slot == slot)
            {
                return slot;
            }
        }
        return (1..dpb_slot_count)
            .find(|candidate| {
                !active_reference_slots
                    .iter()
                    .any(|reference| reference.slot == *candidate)
            })
            .unwrap_or(1);
    }

    let non_reference_slot = 0_usize;
    if !active_reference_slots
        .iter()
        .any(|entry| entry.slot == non_reference_slot)
    {
        return non_reference_slot;
    }
    (0..dpb_slot_count)
        .find(|candidate| {
            !active_reference_slots
                .iter()
                .any(|entry| entry.slot == *candidate)
        })
        .unwrap_or(non_reference_slot)
}

pub(super) fn resolve_hevc_ref_pic_set_usage_limits(
    sps: &SpsRbsp,
    short_term_ref_pic_set_idx: Option<usize>,
) -> Option<(usize, usize)> {
    let set_idx = short_term_ref_pic_set_idx?;
    let short_term_ref_pic_sets = &sps.short_term_ref_pic_sets;
    let used_s0 = short_term_ref_pic_sets.used_by_curr_pic_s0.get(set_idx)?;
    let used_s1 = short_term_ref_pic_sets.used_by_curr_pic_s1.get(set_idx)?;
    let before_count = used_s0.iter().filter(|&&used| used).count();
    let after_count = used_s1.iter().filter(|&&used| used).count();
    Some((before_count, after_count))
}

pub(super) fn resolve_hevc_slice_type_reference_usage_limits(
    parsed_pps: &ParsedHevcPps,
    slice_type: Option<u8>,
) -> Option<(usize, usize)> {
    let slice_type = slice_type?;
    let default_l0 = usize::from(parsed_pps.num_ref_idx_l0_default_active_minus1).saturating_add(1);
    let default_l1 = usize::from(parsed_pps.num_ref_idx_l1_default_active_minus1).saturating_add(1);
    match slice_type {
        0 => Some((default_l0, default_l1)), // B slice
        1 => Some((default_l0, 0)),          // P slice
        2 => Some((0, 0)),                   // I slice
        _ => None,
    }
}

pub(super) fn select_hevc_references_for_ref_pic_set(
    candidates: &[HevcActiveReferenceSlot],
    current_pic_order_cnt_val: i32,
    max_before: usize,
    max_after: usize,
) -> Vec<HevcActiveReferenceSlot> {
    let mut selected = Vec::new();
    let mut selected_before = 0_usize;
    let mut selected_after = 0_usize;
    for candidate in candidates {
        if candidate.pic_order_cnt_val < current_pic_order_cnt_val {
            if selected_before < max_before {
                selected.push(*candidate);
                selected_before = selected_before.saturating_add(1);
            }
            continue;
        }
        if candidate.pic_order_cnt_val > current_pic_order_cnt_val && selected_after < max_after {
            selected.push(*candidate);
            selected_after = selected_after.saturating_add(1);
        }
    }
    selected
}

/// Selects DPB slots by exact POC match from an inline RPS.
///
/// Before-references are returned first (in the order they appear in `inline_pocs.before`),
/// followed by after-references.  Missing POCs are skipped with a warning so that a stale or
/// incomplete DPB does not panic; the caller will produce a shorter-than-expected list which the
/// driver will likely reject, making the failure visible.
pub(super) fn select_hevc_references_by_inline_poc(
    active_slots: &[HevcActiveReferenceSlot],
    inline_pocs: &HevcInlineRefPicSetPocs,
) -> Vec<HevcActiveReferenceSlot> {
    let mut selected = Vec::new();
    for &target_poc in inline_pocs.before[..inline_pocs.before_count].iter() {
        match active_slots
            .iter()
            .find(|s| s.pic_order_cnt_val == target_poc)
        {
            Some(&slot) => selected.push(slot),
            None => eprintln!(
                "vulkan_hevc: inline RPS before-ref poc={target_poc} not found in DPB (active: {:?})",
                active_slots
                    .iter()
                    .map(|s| s.pic_order_cnt_val)
                    .collect::<Vec<_>>()
            ),
        }
    }
    for &target_poc in inline_pocs.after[..inline_pocs.after_count].iter() {
        match active_slots
            .iter()
            .find(|s| s.pic_order_cnt_val == target_poc)
        {
            Some(&slot) => selected.push(slot),
            None => eprintln!(
                "vulkan_hevc: inline RPS after-ref poc={target_poc} not found in DPB (active: {:?})",
                active_slots
                    .iter()
                    .map(|s| s.pic_order_cnt_val)
                    .collect::<Vec<_>>()
            ),
        }
    }
    selected
}

pub(super) fn build_hevc_ref_pic_set_lists(
    references: &[HevcActiveReferenceSlot],
    current_pic_order_cnt_val: i32,
    max_poc_lsb: i32,
) -> (
    [u8; HEVC_REF_PIC_SET_LIST_SIZE],
    [u8; HEVC_REF_PIC_SET_LIST_SIZE],
    u8,
) {
    let mut short_term_before = [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE];
    let mut short_term_after = [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE];
    let mut before_count = 0_usize;
    let mut after_count = 0_usize;
    for reference in references {
        // `RefPicSetStCurrBefore` and `RefPicSetStCurrAfter` must hold **DPB slot indices**,
        // not indices into the `pReferenceSlots` array.  The spec (VkVideoDecodeH265PictureInfoKHR)
        // states: "each element … identifies an active reference picture using its DPB slot index".
        let slot_u8 = u8::try_from(reference.slot).unwrap_or(u8::MAX);
        // Use modular circular distance so that classification remains correct after POC
        // LSB wraparound. A reference is "before" the current picture when the shortest
        // arc distance on the circular POC space is in the past half.
        let d = (reference.pic_order_cnt_val - current_pic_order_cnt_val)
            .rem_euclid(max_poc_lsb.max(1));
        if d > max_poc_lsb / 2 {
            // Past reference (before current).
            if before_count < HEVC_REF_PIC_SET_LIST_SIZE {
                short_term_before[before_count] = slot_u8;
                before_count = before_count.saturating_add(1);
            }
        } else if d > 0 && after_count < HEVC_REF_PIC_SET_LIST_SIZE {
            // Future reference (after current); d == 0 means same POC (self), excluded.
            short_term_after[after_count] = slot_u8;
            after_count = after_count.saturating_add(1);
        }
    }
    let total_references = before_count
        .saturating_add(after_count)
        .min(usize::from(u8::MAX));
    (
        short_term_before,
        short_term_after,
        u8::try_from(total_references).unwrap_or(u8::MAX),
    )
}

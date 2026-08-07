//! Generated CuNNy pass metadata: entry-point names and per-pass slot wiring.
//!
//! Split out of the bench runner so the runner stays readable — this file is
//! bulk data emitted alongside the WGSL ports, not logic. It must stay in step
//! with the app's own copy in `src/app/realtime_sr/mod.rs`.

use super::cunny::{CunnyPassSpec, DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2, DUMMY_READ};

pub(super) const CUNNY_VERYFAST_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_veryfast_nvl_pass_0",
    "cunny_veryfast_nvl_pass_1",
    "cunny_veryfast_nvl_pass_2",
    "cunny_veryfast_nvl_pass_3",
];

pub(super) const CUNNY_VERYFAST_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_veryfast_soft_pass_0",
    "cunny_veryfast_soft_pass_1",
    "cunny_veryfast_soft_pass_2",
    "cunny_veryfast_soft_pass_3",
];

pub(super) const CUNNY_FASTER_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_nvl_pass_0",
    "cunny_faster_nvl_pass_1",
    "cunny_faster_nvl_pass_2",
    "cunny_faster_nvl_pass_3",
];

pub(super) const CUNNY_FASTER_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_soft_pass_0",
    "cunny_faster_soft_pass_1",
    "cunny_faster_soft_pass_2",
    "cunny_faster_soft_pass_3",
];

pub(super) const CUNNY_FASTER_DS_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_ds_pass_0",
    "cunny_faster_ds_pass_1",
    "cunny_faster_ds_pass_2",
    "cunny_faster_ds_pass_3",
];

pub(super) const CUNNY_FAST_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_nvl_pass_0",
    "cunny_fast_nvl_pass_1",
    "cunny_fast_nvl_pass_2",
    "cunny_fast_nvl_pass_3",
];

pub(super) const CUNNY_FAST_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_soft_pass_0",
    "cunny_fast_soft_pass_1",
    "cunny_fast_soft_pass_2",
    "cunny_fast_soft_pass_3",
];

pub(super) const CUNNY_FAST_DS_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_ds_pass_0",
    "cunny_fast_ds_pass_1",
    "cunny_fast_ds_pass_2",
    "cunny_fast_ds_pass_3",
];

pub(super) const CUNNY_2X12_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_2x12_soft_pass_0",
    "cunny_2x12_soft_pass_1",
    "cunny_2x12_soft_pass_2",
    "cunny_2x12_soft_pass_3",
];

pub(super) const CUNNY_2X12_DS_ENTRY_POINTS: [&str; 4] = [
    "cunny_2x12_ds_pass_0",
    "cunny_2x12_ds_pass_1",
    "cunny_2x12_ds_pass_2",
    "cunny_2x12_ds_pass_3",
];

pub(super) const CUNNY_3X12_NVL_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_nvl_pass_0",
    "cunny_3x12_nvl_pass_1",
    "cunny_3x12_nvl_pass_2",
    "cunny_3x12_nvl_pass_3",
    "cunny_3x12_nvl_pass_4",
];

pub(super) const CUNNY_3X12_SOFT_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_soft_pass_0",
    "cunny_3x12_soft_pass_1",
    "cunny_3x12_soft_pass_2",
    "cunny_3x12_soft_pass_3",
    "cunny_3x12_soft_pass_4",
];

pub(super) const CUNNY_3X12_DS_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_ds_pass_0",
    "cunny_3x12_ds_pass_1",
    "cunny_3x12_ds_pass_2",
    "cunny_3x12_ds_pass_3",
    "cunny_3x12_ds_pass_4",
];

pub(super) const CUNNY_4X12_NVL_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_nvl_pass_0",
    "cunny_4x12_nvl_pass_1",
    "cunny_4x12_nvl_pass_2",
    "cunny_4x12_nvl_pass_3",
    "cunny_4x12_nvl_pass_4",
    "cunny_4x12_nvl_pass_5",
];

pub(super) const CUNNY_4X12_SOFT_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_soft_pass_0",
    "cunny_4x12_soft_pass_1",
    "cunny_4x12_soft_pass_2",
    "cunny_4x12_soft_pass_3",
    "cunny_4x12_soft_pass_4",
    "cunny_4x12_soft_pass_5",
];

pub(super) const CUNNY_4X12_DS_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_ds_pass_0",
    "cunny_4x12_ds_pass_1",
    "cunny_4x12_ds_pass_2",
    "cunny_4x12_ds_pass_3",
    "cunny_4x12_ds_pass_4",
    "cunny_4x12_ds_pass_5",
];

pub(super) const CUNNY_4X16_NVL_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x16_nvl_pass_0_chunk_0",
    "cunny_4x16_nvl_pass_0_chunk_1",
    "cunny_4x16_nvl_pass_1_chunk_0",
    "cunny_4x16_nvl_pass_1_chunk_1",
    "cunny_4x16_nvl_pass_2_chunk_0",
    "cunny_4x16_nvl_pass_2_chunk_1",
    "cunny_4x16_nvl_pass_3_chunk_0",
    "cunny_4x16_nvl_pass_3_chunk_1",
    "cunny_4x16_nvl_pass_4_chunk_0",
    "cunny_4x16_nvl_pass_4_chunk_1",
    "cunny_4x16_nvl_pass_5",
];

pub(super) const CUNNY_4X16_SOFT_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x16_soft_pass_0_chunk_0",
    "cunny_4x16_soft_pass_0_chunk_1",
    "cunny_4x16_soft_pass_1_chunk_0",
    "cunny_4x16_soft_pass_1_chunk_1",
    "cunny_4x16_soft_pass_2_chunk_0",
    "cunny_4x16_soft_pass_2_chunk_1",
    "cunny_4x16_soft_pass_3_chunk_0",
    "cunny_4x16_soft_pass_3_chunk_1",
    "cunny_4x16_soft_pass_4_chunk_0",
    "cunny_4x16_soft_pass_4_chunk_1",
    "cunny_4x16_soft_pass_5",
];

pub(super) const CUNNY_4X16_DS_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x16_ds_pass_0_chunk_0",
    "cunny_4x16_ds_pass_0_chunk_1",
    "cunny_4x16_ds_pass_1_chunk_0",
    "cunny_4x16_ds_pass_1_chunk_1",
    "cunny_4x16_ds_pass_2_chunk_0",
    "cunny_4x16_ds_pass_2_chunk_1",
    "cunny_4x16_ds_pass_3_chunk_0",
    "cunny_4x16_ds_pass_3_chunk_1",
    "cunny_4x16_ds_pass_4_chunk_0",
    "cunny_4x16_ds_pass_4_chunk_1",
    "cunny_4x16_ds_pass_5",
];

pub(super) const CUNNY_4X24_NVL_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x24_nvl_pass_0_chunk_0",
    "cunny_4x24_nvl_pass_0_chunk_1",
    "cunny_4x24_nvl_pass_1_chunk_0",
    "cunny_4x24_nvl_pass_1_chunk_1",
    "cunny_4x24_nvl_pass_2_chunk_0",
    "cunny_4x24_nvl_pass_2_chunk_1",
    "cunny_4x24_nvl_pass_3_chunk_0",
    "cunny_4x24_nvl_pass_3_chunk_1",
    "cunny_4x24_nvl_pass_4_chunk_0",
    "cunny_4x24_nvl_pass_4_chunk_1",
    "cunny_4x24_nvl_pass_5",
];

pub(super) const CUNNY_4X24_SOFT_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x24_soft_pass_0_chunk_0",
    "cunny_4x24_soft_pass_0_chunk_1",
    "cunny_4x24_soft_pass_1_chunk_0",
    "cunny_4x24_soft_pass_1_chunk_1",
    "cunny_4x24_soft_pass_2_chunk_0",
    "cunny_4x24_soft_pass_2_chunk_1",
    "cunny_4x24_soft_pass_3_chunk_0",
    "cunny_4x24_soft_pass_3_chunk_1",
    "cunny_4x24_soft_pass_4_chunk_0",
    "cunny_4x24_soft_pass_4_chunk_1",
    "cunny_4x24_soft_pass_5",
];

pub(super) const CUNNY_4X24_DS_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x24_ds_pass_0_chunk_0",
    "cunny_4x24_ds_pass_0_chunk_1",
    "cunny_4x24_ds_pass_1_chunk_0",
    "cunny_4x24_ds_pass_1_chunk_1",
    "cunny_4x24_ds_pass_2_chunk_0",
    "cunny_4x24_ds_pass_2_chunk_1",
    "cunny_4x24_ds_pass_3_chunk_0",
    "cunny_4x24_ds_pass_3_chunk_1",
    "cunny_4x24_ds_pass_4_chunk_0",
    "cunny_4x24_ds_pass_4_chunk_1",
    "cunny_4x24_ds_pass_5",
];

pub(super) const CUNNY_4X32_NVL_ENTRY_POINTS: [&str; 16] = [
    "cunny_4x32_nvl_pass_0_chunk_0",
    "cunny_4x32_nvl_pass_0_chunk_1",
    "cunny_4x32_nvl_pass_0_chunk_2",
    "cunny_4x32_nvl_pass_1_chunk_0",
    "cunny_4x32_nvl_pass_1_chunk_1",
    "cunny_4x32_nvl_pass_1_chunk_2",
    "cunny_4x32_nvl_pass_2_chunk_0",
    "cunny_4x32_nvl_pass_2_chunk_1",
    "cunny_4x32_nvl_pass_2_chunk_2",
    "cunny_4x32_nvl_pass_3_chunk_0",
    "cunny_4x32_nvl_pass_3_chunk_1",
    "cunny_4x32_nvl_pass_3_chunk_2",
    "cunny_4x32_nvl_pass_4_chunk_0",
    "cunny_4x32_nvl_pass_4_chunk_1",
    "cunny_4x32_nvl_pass_4_chunk_2",
    "cunny_4x32_nvl_pass_5",
];

pub(super) const CUNNY_8X32_NVL_ENTRY_POINTS: [&str; 28] = [
    "cunny_8x32_nvl_pass_0_chunk_0",
    "cunny_8x32_nvl_pass_0_chunk_1",
    "cunny_8x32_nvl_pass_0_chunk_2",
    "cunny_8x32_nvl_pass_1_chunk_0",
    "cunny_8x32_nvl_pass_1_chunk_1",
    "cunny_8x32_nvl_pass_1_chunk_2",
    "cunny_8x32_nvl_pass_2_chunk_0",
    "cunny_8x32_nvl_pass_2_chunk_1",
    "cunny_8x32_nvl_pass_2_chunk_2",
    "cunny_8x32_nvl_pass_3_chunk_0",
    "cunny_8x32_nvl_pass_3_chunk_1",
    "cunny_8x32_nvl_pass_3_chunk_2",
    "cunny_8x32_nvl_pass_4_chunk_0",
    "cunny_8x32_nvl_pass_4_chunk_1",
    "cunny_8x32_nvl_pass_4_chunk_2",
    "cunny_8x32_nvl_pass_5_chunk_0",
    "cunny_8x32_nvl_pass_5_chunk_1",
    "cunny_8x32_nvl_pass_5_chunk_2",
    "cunny_8x32_nvl_pass_6_chunk_0",
    "cunny_8x32_nvl_pass_6_chunk_1",
    "cunny_8x32_nvl_pass_6_chunk_2",
    "cunny_8x32_nvl_pass_7_chunk_0",
    "cunny_8x32_nvl_pass_7_chunk_1",
    "cunny_8x32_nvl_pass_7_chunk_2",
    "cunny_8x32_nvl_pass_8_chunk_0",
    "cunny_8x32_nvl_pass_8_chunk_1",
    "cunny_8x32_nvl_pass_8_chunk_2",
    "cunny_8x32_nvl_pass_9",
];

pub(super) const CUNNY_VERYFAST_NVL_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, DUMMY_OUT0, DUMMY_OUT1],
    },
    CunnyPassSpec {
        inputs: &[0, DUMMY_READ, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_VERYFAST_SOFT_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, DUMMY_OUT0, DUMMY_OUT1],
    },
    CunnyPassSpec {
        inputs: &[0, DUMMY_READ, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_FASTER_NVL_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_FASTER_SOFT_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_FASTER_DS_PASSES: [CunnyPassSpec; 4] = CUNNY_FASTER_SOFT_PASSES;

pub(super) const CUNNY_FAST_NVL_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_FAST_SOFT_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_FAST_DS_PASSES: [CunnyPassSpec; 4] = CUNNY_FAST_SOFT_PASSES;

pub(super) const CUNNY_2X12_MPV_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_3X12_NVL_PASSES: [CunnyPassSpec; 5] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_4X12_NVL_PASSES: [CunnyPassSpec; 6] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

pub(super) const CUNNY_4X16_NVL_PASSES: [CunnyPassSpec; 11] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[4, 5, 6],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[7],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[3],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[4, 5, 6],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[7],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[3],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[],
    },
];

pub(super) const CUNNY_4X24_NVL_PASSES: [CunnyPassSpec; 11] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[6, 7, 8],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[9, 10, 11],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[6, 7, 8],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[9, 10, 11],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[],
    },
];

pub(super) const CUNNY_4X32_NVL_PASSES: [CunnyPassSpec; 16] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[],
    },
];

pub(super) const CUNNY_8X32_NVL_PASSES: [CunnyPassSpec; 28] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[],
    },
];

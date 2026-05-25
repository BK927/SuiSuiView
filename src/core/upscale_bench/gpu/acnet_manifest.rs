use crate::core::state::DisplayUpscaler;

pub(crate) const VARIANTS: [AcnetVariantSource; 4] = [
    AcnetVariantSource {
        method: DisplayUpscaler::WgslAcnetF8B4Luma,
        name: "ACNet F8B4 Luma",
        shader: include_str!("../../acnet_f8b4_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
    AcnetVariantSource {
        method: DisplayUpscaler::WgslAcnetF8B4BoxLuma,
        name: "ACNet F8B4 Box Luma",
        shader: include_str!("../../acnet_f8b4_box_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
    AcnetVariantSource {
        method: DisplayUpscaler::WgslAcnetF8B4HdnLuma,
        name: "ACNet F8B4 HDN Luma",
        shader: include_str!("../../acnet_f8b4_hdn_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
    AcnetVariantSource {
        method: DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma,
        name: "ACNet F8B4 Box HDN Luma",
        shader: include_str!("../../acnet_f8b4_box_hdn_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
];

pub(crate) struct AcnetVariantSource {
    pub(crate) method: DisplayUpscaler,
    pub(crate) name: &'static str,
    pub(crate) shader: &'static str,
    pub(crate) entry_points: &'static [&'static str],
    pub(crate) body_blocks: usize,
}

const F8B4_ENTRY_POINTS: [&str; 12] = [
    "acnet_head_conv_1x8x3x3_part_0",
    "acnet_head_conv_1x8x3x3_part_1",
    "acnet_body_block_1_conv_8x8x3x3_part_0",
    "acnet_body_block_1_conv_8x8x3x3_part_1",
    "acnet_body_block_2_conv_8x8x3x3_part_0",
    "acnet_body_block_2_conv_8x8x3x3_part_1",
    "acnet_body_block_3_conv_8x8x3x3_part_0",
    "acnet_body_block_3_conv_8x8x3x3_part_1",
    "acnet_body_block_4_conv_8x8x3x3_part_0",
    "acnet_body_block_4_conv_8x8x3x3_part_1",
    "acnet_upscale_conv_8x4x3x3_part_0",
    "acnet_pixel_shuffle",
];


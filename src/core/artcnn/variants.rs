const C4F16_ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f16_conv2d",
    "artcnn_c4f16_conv2d_1_relu",
    "artcnn_c4f16_conv2d_2_relu",
    "artcnn_c4f16_conv2d_3_relu",
    "artcnn_c4f16_conv2d_4_relu",
    "artcnn_c4f16_conv2d_5",
    "artcnn_c4f16_conv2d_6",
    "artcnn_c4f16_depth_to_space",
];
const C4F16_DN_ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f16_dn_conv2d",
    "artcnn_c4f16_dn_conv2d_1_relu",
    "artcnn_c4f16_dn_conv2d_2_relu",
    "artcnn_c4f16_dn_conv2d_3_relu",
    "artcnn_c4f16_dn_conv2d_4_relu",
    "artcnn_c4f16_dn_conv2d_5",
    "artcnn_c4f16_dn_conv2d_6",
    "artcnn_c4f16_dn_depth_to_space",
];
const C4F16_DS_ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f16_ds_conv2d",
    "artcnn_c4f16_ds_conv2d_1_relu",
    "artcnn_c4f16_ds_conv2d_2_relu",
    "artcnn_c4f16_ds_conv2d_3_relu",
    "artcnn_c4f16_ds_conv2d_4_relu",
    "artcnn_c4f16_ds_conv2d_5",
    "artcnn_c4f16_ds_conv2d_6",
    "artcnn_c4f16_ds_depth_to_space",
];
const C4F32_ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f32_conv2d",
    "artcnn_c4f32_conv2d_1_relu",
    "artcnn_c4f32_conv2d_2_relu",
    "artcnn_c4f32_conv2d_3_relu",
    "artcnn_c4f32_conv2d_4_relu",
    "artcnn_c4f32_conv2d_5",
    "artcnn_c4f32_conv2d_6",
    "artcnn_c4f32_depth_to_space",
];
const C4F32_DN_ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f32_dn_conv2d",
    "artcnn_c4f32_dn_conv2d_1_relu",
    "artcnn_c4f32_dn_conv2d_2_relu",
    "artcnn_c4f32_dn_conv2d_3_relu",
    "artcnn_c4f32_dn_conv2d_4_relu",
    "artcnn_c4f32_dn_conv2d_5",
    "artcnn_c4f32_dn_conv2d_6",
    "artcnn_c4f32_dn_depth_to_space",
];
const C4F32_DS_ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f32_ds_conv2d",
    "artcnn_c4f32_ds_conv2d_1_relu",
    "artcnn_c4f32_ds_conv2d_2_relu",
    "artcnn_c4f32_ds_conv2d_3_relu",
    "artcnn_c4f32_ds_conv2d_4_relu",
    "artcnn_c4f32_ds_conv2d_5",
    "artcnn_c4f32_ds_conv2d_6",
    "artcnn_c4f32_ds_depth_to_space",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtcnnVariant {
    C4F16,
    C4F16Dn,
    C4F16Ds,
    C4F32,
    C4F32Dn,
    C4F32Ds,
}

impl ArtcnnVariant {
    pub(crate) const ALL: [Self; 6] = [
        Self::C4F16,
        Self::C4F16Dn,
        Self::C4F16Ds,
        Self::C4F32,
        Self::C4F32Dn,
        Self::C4F32Ds,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::C4F16 => "ArtCNN C4F16",
            Self::C4F16Dn => "ArtCNN C4F16 DN",
            Self::C4F16Ds => "ArtCNN C4F16 DS",
            Self::C4F32 => "ArtCNN C4F32",
            Self::C4F32Dn => "ArtCNN C4F32 DN",
            Self::C4F32Ds => "ArtCNN C4F32 DS",
        }
    }

    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::C4F16 => "artcnn_c4f16",
            Self::C4F16Dn => "artcnn_c4f16_dn",
            Self::C4F16Ds => "artcnn_c4f16_ds",
            Self::C4F32 => "artcnn_c4f32",
            Self::C4F32Dn => "artcnn_c4f32_dn",
            Self::C4F32Ds => "artcnn_c4f32_ds",
        }
    }

    pub(super) fn entry_points(self) -> &'static [&'static str; 8] {
        match self {
            Self::C4F16 => &C4F16_ENTRY_POINTS,
            Self::C4F16Dn => &C4F16_DN_ENTRY_POINTS,
            Self::C4F16Ds => &C4F16_DS_ENTRY_POINTS,
            Self::C4F32 => &C4F32_ENTRY_POINTS,
            Self::C4F32Dn => &C4F32_DN_ENTRY_POINTS,
            Self::C4F32Ds => &C4F32_DS_ENTRY_POINTS,
        }
    }

    pub(super) fn shader_source(self) -> &'static str {
        match self {
            Self::C4F16 => include_str!("artcnn_c4f16.wgsl"),
            Self::C4F16Dn => include_str!("artcnn_c4f16_dn.wgsl"),
            Self::C4F16Ds => include_str!("artcnn_c4f16_ds.wgsl"),
            Self::C4F32 => include_str!("artcnn_c4f32.wgsl"),
            Self::C4F32Dn => include_str!("artcnn_c4f32_dn.wgsl"),
            Self::C4F32Ds => include_str!("artcnn_c4f32_ds.wgsl"),
        }
    }

    pub(crate) fn feature_size(self, source_size: [usize; 2]) -> Result<[usize; 2], String> {
        let [tile_width, tile_height] = self.feature_tile_size();
        Ok([
            source_size[0]
                .checked_mul(tile_width)
                .ok_or_else(|| format!("{} feature width overflowed", self.label()))?,
            source_size[1]
                .checked_mul(tile_height)
                .ok_or_else(|| format!("{} feature height overflowed", self.label()))?,
        ])
    }

    fn feature_tile_size(self) -> [usize; 2] {
        match self {
            Self::C4F16 | Self::C4F16Dn | Self::C4F16Ds => [2, 2],
            Self::C4F32 | Self::C4F32Dn | Self::C4F32Ds => [4, 2],
        }
    }
}

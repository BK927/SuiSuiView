use crate::core::state::DisplayUpscaler;
use std::borrow::Cow;
use wgpu::util::DeviceExt;

mod acnet;
mod anime4k;

use acnet::AcnetRenderer;
use anime4k::{Anime4kMRenderer, Anime4kSRenderer};

pub(super) const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const CUNNY_INPUT_SLOTS: usize = 8;
const CUNNY_OUTPUT_SLOTS: usize = 3;
const CUNNY_INTERMEDIATE_CAPACITY: usize = 16;
const DUMMY_READ: usize = CUNNY_INTERMEDIATE_CAPACITY;
const DUMMY_OUT0: usize = DUMMY_READ + 1;
const DUMMY_OUT1: usize = DUMMY_READ + 2;
const DUMMY_OUT2: usize = DUMMY_READ + 3;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CunnyParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
}

pub(super) struct RealtimeSrResources {
    cunny: Option<CunnyRenderer>,
    anime4k_s: Option<Anime4kSRenderer>,
    anime4k_m: Option<Anime4kMRenderer>,
    acnet: Option<AcnetRenderer>,
}

pub(super) struct RealtimeSrOutput {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub(super) size: [usize; 2],
    pub(super) byte_size: usize,
}

impl RealtimeSrResources {
    pub(super) fn new() -> Self {
        Self {
            cunny: None,
            anime4k_s: None,
            anime4k_m: None,
            acnet: None,
        }
    }

    pub(super) fn is_supported(method: DisplayUpscaler) -> bool {
        matches!(
            method,
            DisplayUpscaler::CunnyVeryfastNvl
                | DisplayUpscaler::CunnyFasterNvl
                | DisplayUpscaler::CunnyFastNvl
                | DisplayUpscaler::Cunny3x12Nvl
                | DisplayUpscaler::Cunny4x12Nvl
                | DisplayUpscaler::Cunny4x16Nvl
                | DisplayUpscaler::WgslAnime4kV32CnnX2S
                | DisplayUpscaler::WgslAnime4kV32CnnX2M
                | DisplayUpscaler::WgslAcnetF8B4Luma
                | DisplayUpscaler::WgslAcnetF8B4BoxLuma
                | DisplayUpscaler::WgslAcnetF8B4HdnLuma
                | DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma
        )
    }

    pub(super) fn output_size(method: DisplayUpscaler, source_size: [usize; 2]) -> [usize; 2] {
        if Self::is_supported(method) {
            [
                source_size[0].saturating_mul(2),
                source_size[1].saturating_mul(2),
            ]
        } else {
            source_size
        }
    }

    pub(super) fn render(
        &mut self,
        method: DisplayUpscaler,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        match method {
            DisplayUpscaler::CunnyVeryfastNvl
            | DisplayUpscaler::CunnyFasterNvl
            | DisplayUpscaler::CunnyFastNvl
            | DisplayUpscaler::Cunny3x12Nvl
            | DisplayUpscaler::Cunny4x12Nvl
            | DisplayUpscaler::Cunny4x16Nvl => Some(
                self.cunny
                    .get_or_insert_with(|| CunnyRenderer::new(device))
                    .render(method, device, encoder, source_view, source_size),
            ),
            DisplayUpscaler::WgslAnime4kV32CnnX2S => Some(
                self.anime4k_s
                    .get_or_insert_with(|| Anime4kSRenderer::new(device))
                    .render(device, encoder, source_view, source_size),
            ),
            DisplayUpscaler::WgslAnime4kV32CnnX2M => Some(
                self.anime4k_m
                    .get_or_insert_with(|| Anime4kMRenderer::new(device))
                    .render(device, encoder, source_view, source_size),
            ),
            DisplayUpscaler::WgslAcnetF8B4Luma
            | DisplayUpscaler::WgslAcnetF8B4BoxLuma
            | DisplayUpscaler::WgslAcnetF8B4HdnLuma
            | DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma => self
                .acnet
                .get_or_insert_with(|| AcnetRenderer::new(device))
                .render(method, device, encoder, source_view, source_size),
            _ => None,
        }
    }
}

struct CunnyRenderer {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    variants: Vec<CunnyVariant>,
}

struct CunnyVariant {
    method: DisplayUpscaler,
    name: &'static str,
    shader: &'static str,
    entry_points: &'static [&'static str],
    pass_specs: &'static [CunnyPassSpec],
    intermediate_count: usize,
    pipelines: Option<Vec<wgpu::ComputePipeline>>,
}

#[derive(Clone, Copy)]
struct CunnyPassSpec {
    inputs: &'static [usize],
    outputs: &'static [usize],
}

impl CunnyRenderer {
    fn new(device: &wgpu::Device) -> Self {
        let mut layout_entries = Vec::with_capacity(1 + CUNNY_INPUT_SLOTS + CUNNY_OUTPUT_SLOTS + 2);
        layout_entries.push(texture_entry(0));
        for slot in 0..CUNNY_INPUT_SLOTS {
            layout_entries.push(texture_entry(input_binding(slot)));
        }
        for slot in 0..CUNNY_OUTPUT_SLOTS {
            layout_entries.push(storage_entry(output_binding(slot)));
        }
        layout_entries.push(storage_entry(final_binding()));
        layout_entries.push(wgpu::BindGroupLayoutEntry {
            binding: params_binding(),
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-realtime-cunny-bind-group-layout"),
            entries: &layout_entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-realtime-cunny-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let variants = CUNNY_VARIANTS
            .iter()
            .map(|variant| CunnyVariant {
                method: variant.method,
                name: variant.name,
                shader: variant.shader,
                entry_points: variant.entry_points,
                pass_specs: variant.pass_specs,
                intermediate_count: intermediate_count(variant.pass_specs),
                pipelines: None,
            })
            .collect();
        Self {
            bind_group_layout,
            pipeline_layout,
            variants,
        }
    }

    fn render(
        &mut self,
        method: DisplayUpscaler,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> RealtimeSrOutput {
        let variant = self
            .variants
            .iter_mut()
            .find(|variant| variant.method == method)
            .expect("CuNNy method should have a realtime variant");
        let pipelines = variant.pipelines.get_or_insert_with(|| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(variant.name),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(variant.shader)),
            });
            variant
                .entry_points
                .iter()
                .map(|entry_point| {
                    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some(entry_point),
                        layout: Some(&self.pipeline_layout),
                        module: &shader,
                        entry_point: Some(entry_point),
                        compilation_options: Default::default(),
                        cache: None,
                    })
                })
                .collect()
        });
        let output_size = RealtimeSrResources::output_size(method, source_size);
        let source_extent = extent_for_size(source_size);
        let output_extent = extent_for_size(output_size);
        let intermediates: Vec<wgpu::Texture> = (0..variant.intermediate_count)
            .map(|index| create_intermediate_texture(device, source_extent, index))
            .collect();
        let intermediate_views: Vec<wgpu::TextureView> = intermediates
            .iter()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let dummy_read_texture = create_intermediate_texture(device, source_extent, DUMMY_READ);
        let dummy_read_view =
            dummy_read_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let dummy_output_textures: Vec<wgpu::Texture> = (0..CUNNY_OUTPUT_SLOTS)
            .map(dummy_output_index)
            .map(|index| create_intermediate_texture(device, source_extent, index))
            .collect();
        let dummy_output_views: Vec<wgpu::TextureView> = dummy_output_textures
            .iter()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(variant.name),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let params = CunnyParams {
            source_width: source_size[0] as u32,
            source_height: source_size[1] as u32,
            output_width: output_size[0] as u32,
            output_height: output_size[1] as u32,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-realtime-cunny-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        for (index, pass_spec) in variant.pass_specs.iter().enumerate() {
            let mut bind_entries =
                Vec::with_capacity(1 + CUNNY_INPUT_SLOTS + CUNNY_OUTPUT_SLOTS + 2);
            bind_entries.push(texture_binding(0, source_view));
            for slot in 0..CUNNY_INPUT_SLOTS {
                let input_index = pass_spec.inputs.get(slot).copied().unwrap_or(DUMMY_READ);
                bind_entries.push(texture_binding(
                    input_binding(slot),
                    intermediate_view(
                        &intermediate_views,
                        &dummy_read_view,
                        &dummy_output_views,
                        input_index,
                    ),
                ));
            }
            for slot in 0..CUNNY_OUTPUT_SLOTS {
                let output_index = pass_spec
                    .outputs
                    .get(slot)
                    .copied()
                    .unwrap_or_else(|| dummy_output_index(slot));
                bind_entries.push(storage_binding(
                    output_binding(slot),
                    intermediate_view(
                        &intermediate_views,
                        &dummy_read_view,
                        &dummy_output_views,
                        output_index,
                    ),
                ));
            }
            bind_entries.push(storage_binding(final_binding(), &output_view));
            bind_entries.push(wgpu::BindGroupEntry {
                binding: params_binding(),
                resource: params_buffer.as_entire_binding(),
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(variant.entry_points[index]),
                layout: &self.bind_group_layout,
                entries: &bind_entries,
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(variant.entry_points[index]),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipelines[index]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                (source_size[0] as u32).div_ceil(8),
                (source_size[1] as u32).div_ceil(8),
                1,
            );
        }

        RealtimeSrOutput {
            texture: output_texture,
            view: output_view,
            size: output_size,
            byte_size: output_size[0]
                .saturating_mul(output_size[1])
                .saturating_mul(4),
        }
    }
}

fn intermediate_count(pass_specs: &[CunnyPassSpec]) -> usize {
    pass_specs
        .iter()
        .flat_map(|pass| pass.inputs.iter().chain(pass.outputs).copied())
        .filter(|&index| index < DUMMY_READ)
        .max()
        .map_or(0, |index| index + 1)
}

fn intermediate_view<'a>(
    intermediate_views: &'a [wgpu::TextureView],
    dummy_read_view: &'a wgpu::TextureView,
    dummy_output_views: &'a [wgpu::TextureView],
    index: usize,
) -> &'a wgpu::TextureView {
    if index == DUMMY_READ {
        dummy_read_view
    } else if (DUMMY_OUT0..DUMMY_OUT0 + CUNNY_OUTPUT_SLOTS).contains(&index) {
        &dummy_output_views[index - DUMMY_OUT0]
    } else {
        &intermediate_views[index]
    }
}

struct CunnyVariantSource {
    method: DisplayUpscaler,
    name: &'static str,
    shader: &'static str,
    entry_points: &'static [&'static str],
    pass_specs: &'static [CunnyPassSpec],
}

const CUNNY_VARIANTS: [CunnyVariantSource; 6] = [
    CunnyVariantSource {
        method: DisplayUpscaler::CunnyVeryfastNvl,
        name: "CuNNy veryfast NVL",
        shader: include_str!("../../core/cunny_veryfast_nvl.wgsl"),
        entry_points: &CUNNY_VERYFAST_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_VERYFAST_NVL_PASSES,
    },
    CunnyVariantSource {
        method: DisplayUpscaler::CunnyFasterNvl,
        name: "CuNNy faster NVL",
        shader: include_str!("../../core/cunny_faster_nvl.wgsl"),
        entry_points: &CUNNY_FASTER_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_FASTER_NVL_PASSES,
    },
    CunnyVariantSource {
        method: DisplayUpscaler::CunnyFastNvl,
        name: "CuNNy fast NVL",
        shader: include_str!("../../core/cunny_fast_nvl.wgsl"),
        entry_points: &CUNNY_FAST_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_FAST_NVL_PASSES,
    },
    CunnyVariantSource {
        method: DisplayUpscaler::Cunny3x12Nvl,
        name: "CuNNy 3x12 NVL",
        shader: include_str!("../../core/cunny_3x12_nvl.wgsl"),
        entry_points: &CUNNY_3X12_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_3X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: DisplayUpscaler::Cunny4x12Nvl,
        name: "CuNNy 4x12 NVL",
        shader: include_str!("../../core/cunny_4x12_nvl.wgsl"),
        entry_points: &CUNNY_4X12_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_4X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: DisplayUpscaler::Cunny4x16Nvl,
        name: "CuNNy 4x16 NVL",
        shader: include_str!("../../core/cunny_4x16_nvl.wgsl"),
        entry_points: &CUNNY_4X16_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_4X16_NVL_PASSES,
    },
];

const CUNNY_VERYFAST_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_veryfast_nvl_pass_0",
    "cunny_veryfast_nvl_pass_1",
    "cunny_veryfast_nvl_pass_2",
    "cunny_veryfast_nvl_pass_3",
];

const CUNNY_FASTER_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_nvl_pass_0",
    "cunny_faster_nvl_pass_1",
    "cunny_faster_nvl_pass_2",
    "cunny_faster_nvl_pass_3",
];

const CUNNY_FAST_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_nvl_pass_0",
    "cunny_fast_nvl_pass_1",
    "cunny_fast_nvl_pass_2",
    "cunny_fast_nvl_pass_3",
];

const CUNNY_3X12_NVL_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_nvl_pass_0",
    "cunny_3x12_nvl_pass_1",
    "cunny_3x12_nvl_pass_2",
    "cunny_3x12_nvl_pass_3",
    "cunny_3x12_nvl_pass_4",
];

const CUNNY_4X12_NVL_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_nvl_pass_0",
    "cunny_4x12_nvl_pass_1",
    "cunny_4x12_nvl_pass_2",
    "cunny_4x12_nvl_pass_3",
    "cunny_4x12_nvl_pass_4",
    "cunny_4x12_nvl_pass_5",
];

const CUNNY_4X16_NVL_ENTRY_POINTS: [&str; 11] = [
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

const CUNNY_VERYFAST_NVL_PASSES: [CunnyPassSpec; 4] = [
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

const CUNNY_FASTER_NVL_PASSES: [CunnyPassSpec; 4] = [
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

const CUNNY_FAST_NVL_PASSES: [CunnyPassSpec; 4] = [
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

const CUNNY_3X12_NVL_PASSES: [CunnyPassSpec; 5] = [
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

const CUNNY_4X12_NVL_PASSES: [CunnyPassSpec; 6] = [
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

const CUNNY_4X16_NVL_PASSES: [CunnyPassSpec; 11] = [
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

fn extent_for_size(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    }
}

fn create_intermediate_texture(
    device: &wgpu::Device,
    extent: wgpu::Extent3d,
    index: usize,
) -> wgpu::Texture {
    let label = format!("suisuiview-realtime-cunny-intermediate-{index}");
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&label),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn input_binding(slot: usize) -> u32 {
    1 + slot as u32
}

fn output_binding(slot: usize) -> u32 {
    1 + CUNNY_INPUT_SLOTS as u32 + slot as u32
}

fn final_binding() -> u32 {
    1 + CUNNY_INPUT_SLOTS as u32 + CUNNY_OUTPUT_SLOTS as u32
}

fn params_binding() -> u32 {
    final_binding() + 1
}

fn dummy_output_index(slot: usize) -> usize {
    DUMMY_OUT0 + slot
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: TEXTURE_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn texture_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn storage_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

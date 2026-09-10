//! Background-only programs use the same model weights/entry points as the
//! synchronous renderer. The only compute change is an integer row origin.
use super::super::{
    acnet, anime4k, intermediate_count, CUNNY_INPUT_SLOTS, CUNNY_OUTPUT_SLOTS, CUNNY_VARIANTS,
};
use super::graph::{self, Layer, TextureGraph};
use crate::core::state::WgpuUpscaleMethod;
use std::borrow::Cow;

const FP16: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const RGBA8: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub(super) enum Program {
    Anime(AnimeProgram),
    Compute(ComputeProgram),
}

pub(super) struct AnimeProgram {
    medium: bool,
    layouts: Vec<wgpu::BindGroupLayout>,
    pipelines: Vec<wgpu::RenderPipeline>,
}
pub(super) struct ComputeProgram {
    method: WgpuUpscaleMethod,
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}

pub(super) fn supported(method: WgpuUpscaleMethod) -> bool {
    matches!(
        method,
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S
            | WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
            | WgpuUpscaleMethod::WgslSrLabSpanX2
    ) || CUNNY_VARIANTS
        .iter()
        .any(|variant| variant.method == method)
        || acnet::background_source(method).is_some()
}

/// Full feature tensors and output, excluding the separately charged input.
pub(super) fn graph_bytes(method: WgpuUpscaleMethod, size: [usize; 2]) -> Option<usize> {
    let pixels = size[0].checked_mul(size[1])?;
    let feature_bytes = if method == WgpuUpscaleMethod::WgslAnime4kV32CnnX2S {
        2 * 8
    } else if method == WgpuUpscaleMethod::WgslAnime4kV32CnnX2M {
        8 * 8
    } else if acnet::background_source(method).is_some() {
        4 * 8
    } else {
        (intermediate_count(
            CUNNY_VARIANTS
                .iter()
                .find(|v| v.method == method)?
                .pass_specs,
        ) + 4)
            * 4
    };
    // 2x output is four times the area at four bytes per pixel.
    pixels.checked_mul(feature_bytes + 16)
}

impl Program {
    pub(super) fn new(device: &wgpu::Device, method: WgpuUpscaleMethod) -> Option<Self> {
        match method {
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2S | WgpuUpscaleMethod::WgslAnime4kV32CnnX2M => {
                Some(Self::Anime(AnimeProgram::new(
                    device,
                    method == WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
                )))
            }
            _ if method.is_cunny() || acnet::background_source(method).is_some() => {
                Some(Self::Compute(ComputeProgram::new(device, method)?))
            }
            _ => None,
        }
    }
    pub(super) fn graph(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        size: [usize; 2],
    ) -> TextureGraph {
        match self {
            Self::Anime(p) => p.graph(device, source, size),
            Self::Compute(p) => p.graph(device, source, size),
        }
    }
}

impl AnimeProgram {
    fn new(device: &wgpu::Device, medium: bool) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-refine-anime4k"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(if medium {
                anime4k::M_SHADER
            } else {
                anime4k::S_SHADER
            })),
        });
        let mut entries = vec![vec![graph::sampled(0, false), graph::uniform(1, false)]];
        if medium {
            entries.push((0..7).map(|i| graph::sampled(i, false)).collect());
        }
        entries.push(vec![graph::sampled(0, false), graph::sampled(1, false)]);
        let layouts: Vec<_> = entries
            .iter()
            .map(|entries| {
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("suisuiview-refine-anime4k-layout"),
                    entries,
                })
            })
            .collect();
        let pipelines = layouts
            .iter()
            .enumerate()
            .map(|(i, layout)| {
                let final_pass = i == layouts.len() - 1;
                let pipeline_layout =
                    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[layout],
                        push_constant_ranges: &[],
                    });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("suisuiview-refine-anime4k-pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(if final_pass {
                            "fs_final_main"
                        } else if i == 0 {
                            "fs_conv_main"
                        } else {
                            "fs_combine_main"
                        }),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: if final_pass { RGBA8 } else { FP16 },
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview: None,
                    cache: None,
                })
            })
            .collect();
        Self {
            medium,
            layouts,
            pipelines,
        }
    }

    fn graph(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        size: [usize; 2],
    ) -> TextureGraph {
        let count = if self.medium { 8 } else { 2 };
        let textures: Vec<_> = (0..count)
            .map(|_| graph::texture(device, size, FP16))
            .collect();
        let views: Vec<_> = textures
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        let output = graph::output(device, size);
        let mut layers = Vec::new();
        let conv_count = if self.medium { 7 } else { 4 };
        for pass_id in 0..conv_count {
            let input = if pass_id == 0 {
                source
            } else if self.medium {
                &views[pass_id - 1]
            } else {
                &views[(pass_id - 1) % 2]
            };
            let target = if self.medium {
                &views[pass_id]
            } else {
                &views[pass_id % 2]
            };
            let params = graph::params(device, &[pass_id as u32, 0, 0, 0]);
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layouts[0],
                entries: &[
                    graph::binding(0, input),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: params.as_entire_binding(),
                    },
                ],
            });
            layers.push(Layer::Render {
                pipeline: self.pipelines[0].clone(),
                bindings,
                target: target.clone(),
                size: [size[0] as u32, size[1] as u32],
            });
        }
        if self.medium {
            let entries: Vec<_> = views[..7]
                .iter()
                .enumerate()
                .map(|(i, view)| graph::binding(i as u32, view))
                .collect();
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layouts[1],
                entries: &entries,
            });
            layers.push(Layer::Render {
                pipeline: self.pipelines[1].clone(),
                bindings,
                target: views[7].clone(),
                size: [size[0] as u32, size[1] as u32],
            });
        }
        let last = self.layouts.len() - 1;
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layouts[last],
            entries: &[
                graph::binding(0, source),
                graph::binding(1, &views[if self.medium { 7 } else { 1 }]),
            ],
        });
        layers.push(Layer::Render {
            pipeline: self.pipelines[last].clone(),
            bindings,
            target: output.view.clone(),
            size: [output.size[0] as u32, output.size[1] as u32],
        });
        let bytes = size[0] * size[1] * count * 8 + output.byte_size;
        TextureGraph::new(layers, output, textures, bytes)
    }
}

fn offset_shader(source: &str, cunny: bool) -> String {
    let source = if cunny {
        source.replacen("output_height: u32,", "output_height: u32,\n    _pad0: u32,\n    _pad1: u32,\n    _pad2: u32,\n    _pad3: u32,", 1)
    } else {
        source.to_owned()
    };
    source.replace("@builtin(global_invocation_id) global_id: vec3<u32>) {", "@builtin(global_invocation_id) raw_global_id: vec3<u32>) {\n    let global_id = raw_global_id + vec3<u32>(0u, params._pad0, 0u);")
}

impl ComputeProgram {
    fn new(device: &wgpu::Device, method: WgpuUpscaleMethod) -> Option<Self> {
        let cunny = CUNNY_VARIANTS.iter().find(|v| v.method == method);
        let (source, entry_points, _) = if let Some(v) = cunny {
            (v.shader, v.entry_points, 0)
        } else {
            acnet::background_source(method)?
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-refine-compute"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(offset_shader(source, cunny.is_some()))),
        });
        let mut entries = if cunny.is_some() {
            let mut e: Vec<_> = (0..=CUNNY_INPUT_SLOTS as u32)
                .map(|i| graph::sampled(i, true))
                .collect();
            e.extend((9..=12).map(|i| graph::storage(i, RGBA8)));
            e.push(graph::uniform(13, true));
            e
        } else {
            vec![
                graph::sampled(0, true),
                graph::sampled(1, true),
                graph::sampled(2, true),
                graph::storage(3, FP16),
                graph::storage(4, RGBA8),
                graph::uniform(5, true),
            ]
        };
        entries.sort_by_key(|entry| entry.binding);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipelines = entry_points
            .iter()
            .map(|entry| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        Some(Self {
            method,
            layout,
            pipelines,
        })
    }

    fn graph(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        size: [usize; 2],
    ) -> TextureGraph {
        let cunny = CUNNY_VARIANTS.iter().find(|v| v.method == self.method);
        let feature_count = cunny.map_or(4, |v| intermediate_count(v.pass_specs) + 4);
        let format = if cunny.is_some() { RGBA8 } else { FP16 };
        let textures: Vec<_> = (0..feature_count)
            .map(|_| graph::texture(device, size, format))
            .collect();
        let views: Vec<_> = textures
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        let output = graph::output(device, size);
        let dimensions = [
            size[0] as u32,
            size[1] as u32,
            output.size[0] as u32,
            output.size[1] as u32,
        ];
        let mut layers = Vec::new();
        for (index, pipeline) in self.pipelines.iter().enumerate() {
            let params = graph::params(
                device,
                &[
                    dimensions[0],
                    dimensions[1],
                    dimensions[2],
                    dimensions[3],
                    0,
                    0,
                    0,
                    0,
                ],
            );
            let mut entries = vec![graph::binding(0, source)];
            let (target_size, group) = if let Some(variant) = cunny {
                let count = feature_count - 4;
                let spec = &variant.pass_specs[index];
                let mapped = |index: usize| {
                    if index >= super::super::DUMMY_READ {
                        count + index - super::super::DUMMY_READ
                    } else {
                        index
                    }
                };
                for slot in 0..CUNNY_INPUT_SLOTS {
                    entries.push(graph::binding(
                        slot as u32 + 1,
                        &views[mapped(
                            spec.inputs
                                .get(slot)
                                .copied()
                                .unwrap_or(super::super::DUMMY_READ),
                        )],
                    ));
                }
                for slot in 0..CUNNY_OUTPUT_SLOTS {
                    entries.push(graph::binding(
                        slot as u32 + 9,
                        &views[mapped(
                            spec.outputs
                                .get(slot)
                                .copied()
                                .unwrap_or(super::super::DUMMY_OUT0 + slot),
                        )],
                    ));
                }
                entries.push(graph::binding(12, &output.view));
                entries.push(wgpu::BindGroupEntry {
                    binding: 13,
                    resource: params.as_entire_binding(),
                });
                ([size[0] as u32, size[1] as u32], 8)
            } else {
                let blocks = acnet::background_source(self.method)
                    .expect("checked ACNet method")
                    .2;
                let upscale = 2 + blocks * 2;
                let (input0, input1, target) = if index < 2 {
                    (source, source, &views[index])
                } else if index < upscale {
                    let block = (index - 2) / 2;
                    let read = if block % 2 == 0 { 0 } else { 2 };
                    let write = 2 - read;
                    (
                        &views[read],
                        &views[read + 1],
                        &views[write + (index - 2) % 2],
                    )
                } else if index == upscale {
                    let read = if blocks % 2 == 0 { 0 } else { 2 };
                    (&views[read], &views[read + 1], &views[2 - read])
                } else {
                    let read = if blocks % 2 == 0 { 2 } else { 0 };
                    (&views[read], &views[read], &views[read + 1])
                };
                entries.extend([
                    graph::binding(1, input0),
                    graph::binding(2, input1),
                    graph::binding(3, target),
                    graph::binding(4, &output.view),
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: params.as_entire_binding(),
                    },
                ]);
                (
                    if index == upscale + 1 {
                        [output.size[0] as u32, output.size[1] as u32]
                    } else {
                        [size[0] as u32, size[1] as u32]
                    },
                    16,
                )
            };
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &entries,
            });
            layers.push(Layer::Compute {
                pipeline: pipeline.clone(),
                bindings,
                params,
                dimensions,
                size: target_size,
                workgroup: group,
            });
        }
        let bytes = graph_bytes(self.method, size).expect("supported dimensions");
        TextureGraph::new(layers, output, textures, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn row_offset_rewrite_covers_every_compute_entry() {
        for variant in &CUNNY_VARIANTS {
            let shader = offset_shader(variant.shader, true);
            assert_eq!(
                shader.matches("let global_id = raw_global_id").count(),
                variant.entry_points.len(),
                "{}",
                variant.name
            );
        }
        let methods = [
            WgpuUpscaleMethod::WgslAcnetF8B4Luma,
            WgpuUpscaleMethod::WgslAcnetF8B4BoxLuma,
            WgpuUpscaleMethod::WgslAcnetF8B4HdnLuma,
            WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma,
        ];
        for method in methods {
            let (shader, entries, _) = acnet::background_source(method).unwrap();
            assert_eq!(
                offset_shader(shader, false)
                    .matches("let global_id = raw_global_id")
                    .count(),
                entries.len()
            );
        }
    }
    #[test]
    fn full_feature_memory_is_included() {
        assert_eq!(
            graph_bytes(WgpuUpscaleMethod::WgslAnime4kV32CnnX2M, [100, 100]),
            Some(800_000)
        );
        assert_eq!(
            graph_bytes(WgpuUpscaleMethod::WgslAcnetF8B4Luma, [100, 100]),
            Some(480_000)
        );
        assert!(graph_bytes(WgpuUpscaleMethod::Auto, [100, 100]).is_none());
    }
}

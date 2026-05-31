use crate::core::sr_lab::blob::SrLabWeights;
use wgpu::util::DeviceExt;

pub(super) struct SpanGpuModel {
    tensors: Vec<GpuTensor>,
}

pub(super) struct GpuTensor {
    pub(super) name: String,
    pub(super) shape: Vec<u32>,
    pub(super) buffer: wgpu::Buffer,
}

pub(super) struct GpuBuffer {
    pub(super) buffer: wgpu::Buffer,
    pub(super) channels: usize,
    pub(super) height: usize,
    pub(super) width: usize,
}

impl SpanGpuModel {
    pub(super) fn from_weights(device: &wgpu::Device, weights: &SrLabWeights) -> Self {
        let tensors = weights
            .tensors
            .iter()
            .map(|tensor| GpuTensor {
                name: tensor.name.clone(),
                shape: tensor.shape.clone(),
                buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("suisuiview-sr-lab-span-{}", tensor.name)),
                    contents: bytemuck::cast_slice(&tensor.values),
                    usage: wgpu::BufferUsages::STORAGE,
                }),
            })
            .collect();
        Self { tensors }
    }

    pub(super) fn tensor(&self, name: &str) -> Result<&GpuTensor, String> {
        self.tensors
            .iter()
            .find(|tensor| tensor.name == name)
            .ok_or_else(|| format!("missing SR Lab GPU tensor: {name}"))
    }
}

impl GpuBuffer {
    pub(super) fn byte_len(&self) -> u64 {
        (self.channels * self.height * self.width * std::mem::size_of::<f32>()) as u64
    }
}

pub(super) fn buffer_from_values(
    device: &wgpu::Device,
    label: &str,
    channels: usize,
    height: usize,
    width: usize,
    values: &[f32],
) -> GpuBuffer {
    debug_assert_eq!(values.len(), channels * height * width);
    GpuBuffer {
        buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(values),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        }),
        channels,
        height,
        width,
    }
}

pub(super) fn empty_buffer(
    device: &wgpu::Device,
    label: &str,
    channels: usize,
    height: usize,
    width: usize,
) -> GpuBuffer {
    GpuBuffer {
        buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (channels * height * width * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        channels,
        height,
        width,
    }
}

pub(super) fn storage_read_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    storage_entry(binding, true)
}

pub(super) fn storage_read_write_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    storage_entry(binding, false)
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(super) fn storage_binding(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

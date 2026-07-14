//! Text on ash: cosmic-text shaping (reached through glyphon's re-export, so the
//! shaping behavior and fonts are byte-identical to the wgpu path) + swash
//! rasterization into a self-managed RGBA glyph atlas, drawn by the glyph.wgsl
//! pipeline inside the renderer's render pass.
//!
//! `TextSpan` mirrors `glyphon::TextArea` (buffer + position + scale + bounds +
//! default color) so the eventual cutover from `text_renderer.prepare(...)` is
//! mechanical.
//!
//! Atlas strategy: shelf packing into a 1024² RGBA8 image with a CPU mirror.
//! When new glyphs land, the whole mirror is re-uploaded before the next render
//! pass (bounded 4 MiB, and only on glyph-miss frames); if the atlas fills, it is
//! cleared and repacked with just the current frame's glyphs. Mask glyphs are
//! stored white-with-alpha, color (emoji) glyphs as-is drawn with a white vertex
//! color — glyph.wgsl multiplies either by the vertex color.

use std::collections::HashMap;

use ash::vk;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator,
};
use gpu_allocator::MemoryLocation;

use glyphon::cosmic_text::{Buffer as TextBuffer, CacheKey, SwashContent};
use glyphon::{FontSystem, SwashCache};

use super::renderer::{compile_wgsl, create_cpu_buffer, destroy_cpu_buffer, AllocatedBuffer};

const ATLAS_SIZE: u32 = 1024;
const ATLAS_PAD: u32 = 1;

/// One shaped text run to draw. `left`/`top` are physical pixels and `scale`
/// multiplies the shaped (logical) glyph positions — the same contract as
/// glyphon::TextArea, where callers pass `label.x * scale`.
pub struct TextSpan<'a> {
    pub buffer: &'a TextBuffer,
    pub left: f32,
    pub top: f32,
    pub scale: f32,
    /// Physical-pixel clip rect (left, top, right, bottom); None = whole surface.
    pub bounds: Option<[i32; 4]>,
    /// 0..=1 sRGB + alpha, applied to glyphs without their own color.
    pub default_color: [f32; 4],
    /// Rotate the span's glyph quads by (radians, center_x, center_y) in
    /// physical pixels — the circular network pane's curved rim labels.
    pub rotation: Option<(f32, f32, f32)>,
    /// Fragment circle clip (center_x, center_y, radius) in physical pixels;
    /// zero radius disables (matches shader.wgsl's clip_circle).
    pub clip_circle: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    clip_circle: [f32; 3],
}

#[derive(Clone, Copy)]
struct GlyphEntry {
    /// Atlas texel rect.
    u: u32,
    v: u32,
    w: u32,
    h: u32,
    /// Raster placement offsets (from swash).
    left: i32,
    top: i32,
    is_color: bool,
    /// Zero-sized raster (spaces): nothing to draw, but cached to skip re-rastering.
    empty: bool,
}

struct Shelf {
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

impl Shelf {
    fn new() -> Self {
        Shelf { cursor_x: ATLAS_PAD, cursor_y: ATLAS_PAD, row_height: 0 }
    }

    fn insert(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > ATLAS_SIZE - 2 * ATLAS_PAD || h > ATLAS_SIZE - 2 * ATLAS_PAD {
            return None;
        }
        if self.cursor_x + w + ATLAS_PAD > ATLAS_SIZE {
            self.cursor_x = ATLAS_PAD;
            self.cursor_y += self.row_height + ATLAS_PAD;
            self.row_height = 0;
        }
        if self.cursor_y + h + ATLAS_PAD > ATLAS_SIZE {
            return None;
        }
        let pos = (self.cursor_x, self.cursor_y);
        self.cursor_x += w + ATLAS_PAD;
        self.row_height = self.row_height.max(h);
        Some(pos)
    }
}

struct TextFrame {
    vertex: AllocatedBuffer,
    vertex_count: u32,
    staging: AllocatedBuffer,
    /// Atlas generation this frame's staging buffer last uploaded.
    uploaded_generation: u64,
}

pub(crate) struct TextStage {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    shader_module: vk::ShaderModule,
    sampler: vk::Sampler,

    atlas_image: vk::Image,
    atlas_view: vk::ImageView,
    atlas_allocation: Option<Allocation>,
    /// CPU mirror of the atlas (RGBA8, ATLAS_SIZE²).
    atlas_cpu: Vec<u8>,
    atlas_initialized: bool,
    generation: u64,

    glyphs: HashMap<CacheKey, GlyphEntry>,
    shelf: Shelf,

    pending_vertices: Vec<GlyphVertex>,
    frames: Vec<TextFrame>,
}

impl TextStage {
    pub(crate) fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        render_pass: vk::RenderPass,
        frames_in_flight: usize,
    ) -> Self {
        unsafe {
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let descriptor_set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .expect("Failed to create text descriptor set layout");
            let set_layouts = [descriptor_set_layout];
            let pipeline_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                    None,
                )
                .expect("Failed to create text pipeline layout");

            let spirv = compile_wgsl(include_str!("glyph.wgsl"));
            let shader_module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&spirv), None)
                .expect("Failed to create glyph shader module");

            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(shader_module)
                    .name(c"vs_main"),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(shader_module)
                    .name(c"fs_main"),
            ];
            let vertex_bindings = [vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(std::mem::size_of::<GlyphVertex>() as u32)
                .input_rate(vk::VertexInputRate::VERTEX)];
            let vertex_attributes = [
                vk::VertexInputAttributeDescription::default()
                    .location(0)
                    .binding(0)
                    .format(vk::Format::R32G32_SFLOAT)
                    .offset(0),
                vk::VertexInputAttributeDescription::default()
                    .location(1)
                    .binding(0)
                    .format(vk::Format::R32G32_SFLOAT)
                    .offset(8),
                vk::VertexInputAttributeDescription::default()
                    .location(2)
                    .binding(0)
                    .format(vk::Format::R32G32B32A32_SFLOAT)
                    .offset(16),
                vk::VertexInputAttributeDescription::default()
                    .location(3)
                    .binding(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(32),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&vertex_bindings)
                .vertex_attribute_descriptions(&vertex_attributes);
            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(&blend_attachments);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let pipeline = device
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::GraphicsPipelineCreateInfo::default()
                        .stages(&stages)
                        .vertex_input_state(&vertex_input)
                        .input_assembly_state(&input_assembly)
                        .viewport_state(&viewport_state)
                        .rasterization_state(&rasterization)
                        .multisample_state(&multisample)
                        .color_blend_state(&color_blend)
                        .dynamic_state(&dynamic_state)
                        .layout(pipeline_layout)
                        .render_pass(render_pass)
                        .subpass(0)],
                    None,
                )
                .expect("Failed to create glyph pipeline")[0];

            let atlas_image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D { width: ATLAS_SIZE, height: ATLAS_SIZE, depth: 1 })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .expect("Failed to create atlas image");
            let requirements = device.get_image_memory_requirements(atlas_image);
            let atlas_allocation = allocator
                .allocate(&AllocationCreateDesc {
                    name: "glyph-atlas",
                    requirements,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .expect("Failed to allocate atlas memory");
            device
                .bind_image_memory(atlas_image, atlas_allocation.memory(), atlas_allocation.offset())
                .expect("Failed to bind atlas memory");
            let atlas_view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(atlas_image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )
                .expect("Failed to create atlas view");

            // Glyphs are sampled 1:1; NEAREST keeps them crisp.
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .expect("Failed to create atlas sampler");

            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1),
            ];
            let descriptor_pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .expect("Failed to create text descriptor pool");
            let descriptor_set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(descriptor_pool)
                        .set_layouts(&set_layouts),
                )
                .expect("Failed to allocate text descriptor set")[0];
            let image_infos = [vk::DescriptorImageInfo::default()
                .image_view(atlas_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler_infos = [vk::DescriptorImageInfo::default().sampler(sampler)];
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&image_infos),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&sampler_infos),
                ],
                &[],
            );

            let atlas_bytes = (ATLAS_SIZE * ATLAS_SIZE * 4) as vk::DeviceSize;
            let frames = (0..frames_in_flight)
                .map(|_| TextFrame {
                    vertex: create_cpu_buffer(
                        device,
                        allocator,
                        64 * 1024,
                        vk::BufferUsageFlags::VERTEX_BUFFER,
                        "glyph-vertices",
                    ),
                    vertex_count: 0,
                    staging: create_cpu_buffer(
                        device,
                        allocator,
                        atlas_bytes,
                        vk::BufferUsageFlags::TRANSFER_SRC,
                        "atlas-staging",
                    ),
                    uploaded_generation: 0,
                })
                .collect();

            TextStage {
                pipeline,
                pipeline_layout,
                descriptor_set_layout,
                descriptor_pool,
                descriptor_set,
                shader_module,
                sampler,
                atlas_image,
                atlas_view,
                atlas_allocation: Some(atlas_allocation),
                atlas_cpu: vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
                atlas_initialized: false,
                generation: 1,
                glyphs: HashMap::new(),
                shelf: Shelf::new(),
                pending_vertices: Vec::new(),
                frames,
            }
        }
    }

    /// Rasterize (on miss) and cache one glyph. Returns None when the atlas is full.
    fn ensure_glyph(
        &mut self,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        key: CacheKey,
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.glyphs.get(&key) {
            return Some(*entry);
        }
        let image = swash_cache.get_image_uncached(font_system, key)?;
        let w = image.placement.width;
        let h = image.placement.height;
        if w == 0 || h == 0 || image.data.is_empty() {
            let entry = GlyphEntry {
                u: 0, v: 0, w: 0, h: 0, left: 0, top: 0, is_color: false, empty: true,
            };
            self.glyphs.insert(key, entry);
            return Some(entry);
        }
        let (u, v) = self.shelf.insert(w, h)?;

        let is_color = !matches!(image.content, SwashContent::Mask);
        for row in 0..h {
            for col in 0..w {
                let dst = (((v + row) * ATLAS_SIZE + (u + col)) * 4) as usize;
                let texel = match image.content {
                    SwashContent::Mask => {
                        let a = image.data[(row * w + col) as usize];
                        [255, 255, 255, a]
                    }
                    // Color and SubpixelMask rasters are RGBA.
                    _ => {
                        let src = ((row * w + col) * 4) as usize;
                        [
                            image.data[src],
                            image.data[src + 1],
                            image.data[src + 2],
                            image.data[src + 3],
                        ]
                    }
                };
                self.atlas_cpu[dst..dst + 4].copy_from_slice(&texel);
            }
        }
        self.generation += 1;

        let entry = GlyphEntry {
            u,
            v,
            w,
            h,
            left: image.placement.left,
            top: image.placement.top,
            is_color,
            empty: false,
        };
        self.glyphs.insert(key, entry);
        Some(entry)
    }

    /// Build this frame's glyph vertices. Positions/bounds in physical pixels,
    /// NDC computed against `extent` (wgpu convention; the shader flips for Vulkan).
    pub(crate) fn prepare(
        &mut self,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        spans: &[TextSpan<'_>],
        extent: vk::Extent2D,
    ) {
        self.pending_vertices.clear();
        if !self.try_prepare(font_system, swash_cache, spans, extent) {
            // Atlas full: clear and repack with only the glyphs this frame needs.
            log::info!("glyph atlas full — clearing and repacking");
            self.glyphs.clear();
            self.shelf = Shelf::new();
            self.atlas_cpu.fill(0);
            self.generation += 1;
            self.pending_vertices.clear();
            if !self.try_prepare(font_system, swash_cache, spans, extent) {
                log::error!("glyph atlas full even after repack; text truncated this frame");
            }
        }
    }

    fn try_prepare(
        &mut self,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        spans: &[TextSpan<'_>],
        extent: vk::Extent2D,
    ) -> bool {
        let sw = extent.width as f32;
        let sh = extent.height as f32;
        for span in spans {
            for run in span.buffer.layout_runs() {
                let line_y = (run.line_y * span.scale).round() as i32;
                for glyph in run.glyphs.iter() {
                    let physical = glyph.physical((span.left, span.top), span.scale);
                    let Some(entry) =
                        self.ensure_glyph(font_system, swash_cache, physical.cache_key)
                    else {
                        // Distinguish "atlas full" (retryable) from "unrasterizable"
                        // (skip): a missing swash image caches as empty above, so a
                        // None here means the shelf rejected it.
                        if swash_cache
                            .get_image_uncached(font_system, physical.cache_key)
                            .is_some()
                        {
                            return false;
                        }
                        continue;
                    };
                    if entry.empty {
                        continue;
                    }

                    // glyphon's placement formula, physical pixels.
                    let mut x0 = (physical.x + entry.left) as f32;
                    let mut y0 = (line_y + physical.y - entry.top) as f32;
                    let mut x1 = x0 + entry.w as f32;
                    let mut y1 = y0 + entry.h as f32;
                    let mut u0 = entry.u as f32;
                    let mut v0 = entry.v as f32;
                    let mut u1 = u0 + entry.w as f32;
                    let mut v1 = v0 + entry.h as f32;

                    // CPU clip to span bounds, shrinking UVs proportionally.
                    if let Some([bl, bt, br, bb]) = span.bounds {
                        let (bl, bt, br, bb) = (bl as f32, bt as f32, br as f32, bb as f32);
                        if x0 >= br || x1 <= bl || y0 >= bb || y1 <= bt {
                            continue;
                        }
                        if x0 < bl {
                            u0 += bl - x0;
                            x0 = bl;
                        }
                        if x1 > br {
                            u1 -= x1 - br;
                            x1 = br;
                        }
                        if y0 < bt {
                            v0 += bt - y0;
                            y0 = bt;
                        }
                        if y1 > bb {
                            v1 -= y1 - bb;
                            y1 = bb;
                        }
                    }

                    let color = if entry.is_color {
                        [1.0, 1.0, 1.0, 1.0]
                    } else if let Some(c) = glyph.color_opt {
                        [
                            c.r() as f32 / 255.0,
                            c.g() as f32 / 255.0,
                            c.b() as f32 / 255.0,
                            c.a() as f32 / 255.0,
                        ]
                    } else {
                        span.default_color
                    };

                    // Corner positions, optionally rotated about the span's center
                    // (physical px) before the NDC mapping.
                    let corners = match span.rotation {
                        None => [[x0, y0], [x1, y0], [x0, y1], [x1, y1]],
                        Some((angle, cx, cy)) => {
                            let (sin_a, cos_a) = angle.sin_cos();
                            let rot = |px: f32, py: f32| {
                                let (dx, dy) = (px - cx, py - cy);
                                [cx + dx * cos_a - dy * sin_a, cy + dx * sin_a + dy * cos_a]
                            };
                            [rot(x0, y0), rot(x1, y0), rot(x0, y1), rot(x1, y1)]
                        }
                    };
                    let ndc = |p: [f32; 2]| {
                        [(p[0] / sw) * 2.0 - 1.0, 1.0 - (p[1] / sh) * 2.0]
                    };
                    let uv = |u: f32, v: f32| [u / ATLAS_SIZE as f32, v / ATLAS_SIZE as f32];
                    let clip_circle = span.clip_circle;
                    let tl = GlyphVertex { position: ndc(corners[0]), uv: uv(u0, v0), color, clip_circle };
                    let tr = GlyphVertex { position: ndc(corners[1]), uv: uv(u1, v0), color, clip_circle };
                    let bl = GlyphVertex { position: ndc(corners[2]), uv: uv(u0, v1), color, clip_circle };
                    let br = GlyphVertex { position: ndc(corners[3]), uv: uv(u1, v1), color, clip_circle };
                    self.pending_vertices.extend([tl, tr, bl, tr, br, bl]);
                }
            }
        }
        true
    }

    /// Called after this frame's fence has been waited: move pending vertices into
    /// the frame's buffer and refresh its staging copy if the atlas changed.
    pub(crate) fn write_frame_buffers(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        frame_index: usize,
    ) {
        let vertices = std::mem::take(&mut self.pending_vertices);
        let frame = &mut self.frames[frame_index];

        let bytes: &[u8] = bytemuck::cast_slice(&vertices);
        let needed = bytes.len() as vk::DeviceSize;
        if needed > frame.vertex.size {
            let mut old = std::mem::replace(&mut frame.vertex, AllocatedBuffer::null());
            destroy_cpu_buffer(device, allocator, &mut old);
            frame.vertex = create_cpu_buffer(
                device,
                allocator,
                needed.next_power_of_two(),
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "glyph-vertices",
            );
        }
        if !bytes.is_empty() {
            frame.vertex.allocation.as_mut().unwrap().mapped_slice_mut().unwrap()
                [..bytes.len()]
                .copy_from_slice(bytes);
        }
        frame.vertex_count = vertices.len() as u32;
        self.pending_vertices = vertices;
        self.pending_vertices.clear();

        let frame = &mut self.frames[frame_index];
        if frame.uploaded_generation != self.generation {
            frame.staging.allocation.as_mut().unwrap().mapped_slice_mut().unwrap()
                [..self.atlas_cpu.len()]
                .copy_from_slice(&self.atlas_cpu);
        }
    }

    /// Record the atlas upload (if this frame's staging is newer than the image).
    /// Must be called outside a render pass.
    pub(crate) fn record_upload(&mut self, device: &ash::Device, cmd: vk::CommandBuffer, frame_index: usize) {
        let frame = &mut self.frames[frame_index];
        if frame.uploaded_generation == self.generation {
            return;
        }
        frame.uploaded_generation = self.generation;

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let (old_layout, src_stage, src_access) = if self.atlas_initialized {
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::SHADER_READ,
            )
        } else {
            (
                vk::ImageLayout::UNDEFINED,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::AccessFlags::empty(),
            )
        };
        self.atlas_initialized = true;

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                src_stage,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .src_access_mask(src_access)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(old_layout)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.atlas_image)
                    .subresource_range(range)],
            );
            device.cmd_copy_buffer_to_image(
                cmd,
                frame.staging.buffer,
                self.atlas_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::BufferImageCopy::default()
                    .buffer_offset(0)
                    .buffer_row_length(ATLAS_SIZE)
                    .buffer_image_height(ATLAS_SIZE)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: ATLAS_SIZE,
                        height: ATLAS_SIZE,
                        depth: 1,
                    })],
            );
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.atlas_image)
                    .subresource_range(range)],
            );
        }
    }

    /// Record the glyph draw. Must be called inside the render pass, after the
    /// 2D quads (text goes on top). Viewport/scissor are inherited (dynamic,
    /// already set by the caller).
    pub(crate) fn record_draw(&self, device: &ash::Device, cmd: vk::CommandBuffer, frame_index: usize) {
        let frame = &self.frames[frame_index];
        if frame.vertex_count == 0 || !self.atlas_initialized {
            return;
        }
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[frame.vertex.buffer], &[0]);
            device.cmd_draw(cmd, frame.vertex_count, 1, 0, 0);
        }
    }

    pub(crate) fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for frame in &mut self.frames {
                let mut vertex = std::mem::replace(&mut frame.vertex, AllocatedBuffer::null());
                destroy_cpu_buffer(device, allocator, &mut vertex);
                let mut staging = std::mem::replace(&mut frame.staging, AllocatedBuffer::null());
                destroy_cpu_buffer(device, allocator, &mut staging);
            }
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.atlas_view, None);
            device.destroy_image(self.atlas_image, None);
            if let Some(allocation) = self.atlas_allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_shader_module(self.shader_module, None);
        }
    }
}

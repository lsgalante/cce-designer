//! 3D scene stage: the ash port of the app's "3D canvas render pass". Draws
//! Vertex3D meshes (shader_3d.wgsl: mvp transform, z=9.99 background-quad
//! special case, window-corner discard) into the full-size backdrop image with
//! a depth buffer, scissored to the viewport pane. The renderer then copies the
//! backdrop into the swapchain image and draws the UI pass over it — the same
//! image doubles as the blur-behind source for the 2D shader, replacing
//! milestone 1's 1x1 placeholder.
//!
//! Meshes are handle-based (`MeshId`); per-draw uniforms (mvp + window info) go
//! into one dynamic-offset uniform buffer per frame in flight, so a frame's
//! draws share a single descriptor set.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;

use super::renderer::{compile_wgsl, create_cpu_buffer, destroy_cpu_buffer, AllocatedBuffer};

/// Layout-identical to the app's `geometry::Vertex3D` (bytemuck-castable at cutover).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex3D {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshId(usize);

/// One draw in the staged scene: a mesh under an mvp. The window-size/radius
/// tail of shader_3d's uniform block is filled in by the renderer.
pub struct SceneDraw {
    pub mesh: MeshId,
    pub mvp: [[f32; 4]; 4],
}

/// shader_3d.wgsl's uniform block.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SceneUniforms {
    mvp: [[f32; 4]; 4],
    window_size: [f32; 2],
    window_radius: f32,
    _padding: f32,
}

const UNIFORM_SIZE: vk::DeviceSize = std::mem::size_of::<SceneUniforms>() as vk::DeviceSize;

struct Mesh {
    buffer: AllocatedBuffer,
    count: u32,
}

struct StagedScene {
    scissor: (u32, u32, u32, u32),
    draws: Vec<SceneDraw>,
}

struct SceneFrame {
    uniforms: AllocatedBuffer,
    descriptor_set: vk::DescriptorSet,
    draw_count: u32,
}

pub(crate) struct SceneStage {
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    shader_module: vk::ShaderModule,
    uniform_stride: vk::DeviceSize,

    format: vk::Format,
    extent: vk::Extent2D,
    pub(crate) backdrop_image: vk::Image,
    pub(crate) backdrop_view: vk::ImageView,
    backdrop_allocation: Option<Allocation>,
    depth_image: vk::Image,
    depth_view: vk::ImageView,
    depth_allocation: Option<Allocation>,
    framebuffer: vk::Framebuffer,

    meshes: Vec<Mesh>,
    frames: Vec<SceneFrame>,
    staged: Option<StagedScene>,
    /// True once the backdrop holds rendered content worth copying to screen.
    pub(crate) backdrop_valid: bool,
}

impl SceneStage {
    pub(crate) fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        format: vk::Format,
        extent: vk::Extent2D,
        frames_in_flight: usize,
        min_uniform_align: vk::DeviceSize,
    ) -> Self {
        unsafe {
            // Offscreen pass: color -> TRANSFER_SRC (copied to the swapchain
            // right after), depth is transient.
            let attachments = [
                vk::AttachmentDescription::default()
                    .format(format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
                vk::AttachmentDescription::default()
                    .format(vk::Format::D32_SFLOAT)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            ];
            let color_refs = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let depth_ref = vk::AttachmentReference::default()
                .attachment(1)
                .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
            let subpasses = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_refs)
                .depth_stencil_attachment(&depth_ref)];
            let dependencies = [
                // Prior frame sampled the backdrop (blur plates) and used the depth
                // image; execution dependency before we overwrite from UNDEFINED.
                vk::SubpassDependency::default()
                    .src_subpass(vk::SUBPASS_EXTERNAL)
                    .dst_subpass(0)
                    .src_stage_mask(
                        vk::PipelineStageFlags::FRAGMENT_SHADER
                            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                    )
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_stage_mask(
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                            | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                    )
                    .dst_access_mask(
                        vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                            | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                    ),
                // The copy to the swapchain reads the color attachment right after.
                vk::SubpassDependency::default()
                    .src_subpass(0)
                    .dst_subpass(vk::SUBPASS_EXTERNAL)
                    .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
            ];
            let render_pass = device
                .create_render_pass(
                    &vk::RenderPassCreateInfo::default()
                        .attachments(&attachments)
                        .subpasses(&subpasses)
                        .dependencies(&dependencies),
                    None,
                )
                .expect("Failed to create scene render pass");

            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
            let descriptor_set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .expect("Failed to create scene descriptor set layout");
            let set_layouts_one = [descriptor_set_layout];
            let pipeline_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts_one),
                    None,
                )
                .expect("Failed to create scene pipeline layout");

            let spirv = compile_wgsl(include_str!("../shader_3d.wgsl"));
            let shader_module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&spirv), None)
                .expect("Failed to create 3D shader module");
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
                .stride(std::mem::size_of::<Vertex3D>() as u32)
                .input_rate(vk::VertexInputRate::VERTEX)];
            let vertex_attributes = [
                vk::VertexInputAttributeDescription::default()
                    .location(0)
                    .binding(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(0),
                vk::VertexInputAttributeDescription::default()
                    .location(1)
                    .binding(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(12),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&vertex_bindings)
                .vertex_attribute_descriptions(&vertex_attributes);
            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            // wgpu pipeline_3d: CCW front, back-face culling. Winding survives
            // because the renderer flips Y via negative viewport height (like
            // wgpu-hal), not in the shader.
            let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::BACK)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(true)
                .depth_compare_op(vk::CompareOp::LESS);
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
                        .depth_stencil_state(&depth_stencil)
                        .color_blend_state(&color_blend)
                        .dynamic_state(&dynamic_state)
                        .layout(pipeline_layout)
                        .render_pass(render_pass)
                        .subpass(0)],
                    None,
                )
                .expect("Failed to create 3D pipeline")[0];

            let uniform_stride = UNIFORM_SIZE.next_multiple_of(min_uniform_align.max(1));

            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .descriptor_count(frames_in_flight as u32)];
            let descriptor_pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(frames_in_flight as u32)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .expect("Failed to create scene descriptor pool");
            let set_layouts: Vec<vk::DescriptorSetLayout> =
                vec![descriptor_set_layout; frames_in_flight];
            let sets = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(descriptor_pool)
                        .set_layouts(&set_layouts),
                )
                .expect("Failed to allocate scene descriptor sets");
            let frames: Vec<SceneFrame> = sets
                .into_iter()
                .map(|descriptor_set| {
                    let uniforms = create_cpu_buffer(
                        device,
                        allocator,
                        uniform_stride * 16,
                        vk::BufferUsageFlags::UNIFORM_BUFFER,
                        "scene-uniforms",
                    );
                    SceneFrame { uniforms, descriptor_set, draw_count: 0 }
                })
                .collect();
            for frame in &frames {
                Self::write_descriptor(device, frame);
            }

            let mut stage = SceneStage {
                render_pass,
                pipeline,
                pipeline_layout,
                descriptor_set_layout,
                descriptor_pool,
                shader_module,
                uniform_stride,
                format,
                extent: vk::Extent2D { width: 0, height: 0 },
                backdrop_image: vk::Image::null(),
                backdrop_view: vk::ImageView::null(),
                backdrop_allocation: None,
                depth_image: vk::Image::null(),
                depth_view: vk::ImageView::null(),
                depth_allocation: None,
                framebuffer: vk::Framebuffer::null(),
                meshes: Vec::new(),
                frames,
                staged: None,
                backdrop_valid: false,
            };
            stage.resize(device, allocator, extent);
            stage
        }
    }

    fn write_descriptor(device: &ash::Device, frame: &SceneFrame) {
        let buffer_infos = [vk::DescriptorBufferInfo::default()
            .buffer(frame.uniforms.buffer)
            .offset(0)
            .range(UNIFORM_SIZE)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(frame.descriptor_set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                    .buffer_info(&buffer_infos)],
                &[],
            );
        }
    }

    fn destroy_targets(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            if self.framebuffer != vk::Framebuffer::null() {
                device.destroy_framebuffer(self.framebuffer, None);
                self.framebuffer = vk::Framebuffer::null();
            }
            if self.backdrop_view != vk::ImageView::null() {
                device.destroy_image_view(self.backdrop_view, None);
                device.destroy_image(self.backdrop_image, None);
                self.backdrop_view = vk::ImageView::null();
                self.backdrop_image = vk::Image::null();
            }
            if self.depth_view != vk::ImageView::null() {
                device.destroy_image_view(self.depth_view, None);
                device.destroy_image(self.depth_image, None);
                self.depth_view = vk::ImageView::null();
                self.depth_image = vk::Image::null();
            }
        }
        if let Some(a) = self.backdrop_allocation.take() {
            let _ = allocator.free(a);
        }
        if let Some(a) = self.depth_allocation.take() {
            let _ = allocator.free(a);
        }
    }

    /// (Re)create the backdrop + depth targets at `extent`. Caller must have the
    /// device idle (the renderer's swapchain-rebuild path guarantees it) and must
    /// re-point the UI descriptor at the new `backdrop_view` and re-init its layout.
    pub(crate) fn resize(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        extent: vk::Extent2D,
    ) {
        if extent == self.extent && self.framebuffer != vk::Framebuffer::null() {
            return;
        }
        self.destroy_targets(device, allocator);
        self.extent = extent;
        self.backdrop_valid = false;
        unsafe {
            let backdrop_image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(self.format)
                        .extent(vk::Extent3D {
                            width: extent.width,
                            height: extent.height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::SAMPLED
                                | vk::ImageUsageFlags::TRANSFER_SRC
                                | vk::ImageUsageFlags::TRANSFER_DST,
                        )
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .expect("Failed to create backdrop image");
            let requirements = device.get_image_memory_requirements(backdrop_image);
            let allocation = allocator
                .allocate(&AllocationCreateDesc {
                    name: "backdrop",
                    requirements,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .expect("Failed to allocate backdrop memory");
            device
                .bind_image_memory(backdrop_image, allocation.memory(), allocation.offset())
                .expect("Failed to bind backdrop memory");
            let backdrop_view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(backdrop_image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(self.format)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )
                .expect("Failed to create backdrop view");
            self.backdrop_image = backdrop_image;
            self.backdrop_view = backdrop_view;
            self.backdrop_allocation = Some(allocation);

            let depth_image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::D32_SFLOAT)
                        .extent(vk::Extent3D {
                            width: extent.width,
                            height: extent.height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .expect("Failed to create depth image");
            let requirements = device.get_image_memory_requirements(depth_image);
            let allocation = allocator
                .allocate(&AllocationCreateDesc {
                    name: "depth",
                    requirements,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .expect("Failed to allocate depth memory");
            device
                .bind_image_memory(depth_image, allocation.memory(), allocation.offset())
                .expect("Failed to bind depth memory");
            let depth_view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(depth_image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::D32_SFLOAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )
                .expect("Failed to create depth view");
            self.depth_image = depth_image;
            self.depth_view = depth_view;
            self.depth_allocation = Some(allocation);

            let attachments = [self.backdrop_view, self.depth_view];
            self.framebuffer = device
                .create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(self.render_pass)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
                .expect("Failed to create scene framebuffer");
        }
    }

    pub(crate) fn create_mesh(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        verts: &[Vertex3D],
    ) -> MeshId {
        let bytes: &[u8] = bytemuck::cast_slice(verts);
        let mut buffer = create_cpu_buffer(
            device,
            allocator,
            (bytes.len() as vk::DeviceSize).max(64),
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "mesh",
        );
        if !bytes.is_empty() {
            buffer.allocation.as_mut().unwrap().mapped_slice_mut().unwrap()[..bytes.len()]
                .copy_from_slice(bytes);
        }
        self.meshes.push(Mesh { buffer, count: verts.len() as u32 });
        MeshId(self.meshes.len() - 1)
    }

    /// Replace a mesh's vertices. Caller must have the device idle: meshes may be
    /// referenced by in-flight frames (geometry updates are rare — settings
    /// changes and graph rebuilds — so a wait is acceptable here).
    #[allow(dead_code)] // cutover API: the app's rebuild_scene_geometry path
    pub(crate) fn update_mesh(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        id: MeshId,
        verts: &[Vertex3D],
    ) {
        let mesh = &mut self.meshes[id.0];
        let bytes: &[u8] = bytemuck::cast_slice(verts);
        let needed = bytes.len() as vk::DeviceSize;
        if needed > mesh.buffer.size {
            let mut old = std::mem::replace(&mut mesh.buffer, AllocatedBuffer::null());
            destroy_cpu_buffer(device, allocator, &mut old);
            mesh.buffer = create_cpu_buffer(
                device,
                allocator,
                needed.next_power_of_two(),
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "mesh",
            );
        }
        if !bytes.is_empty() {
            mesh.buffer.allocation.as_mut().unwrap().mapped_slice_mut().unwrap()[..bytes.len()]
                .copy_from_slice(bytes);
        }
        mesh.count = verts.len() as u32;
    }

    pub(crate) fn stage(&mut self, scissor: (u32, u32, u32, u32), draws: Vec<SceneDraw>) {
        self.staged = Some(StagedScene { scissor, draws });
    }

    /// After the frame fence: write this frame's per-draw uniforms (mvp + the
    /// window-corner info shader_3d shares with the 2D shader).
    pub(crate) fn write_frame_uniforms(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        frame_index: usize,
        corner_radius_px: f32,
    ) {
        let Some(staged) = &self.staged else {
            self.frames[frame_index].draw_count = 0;
            return;
        };
        let frame = &mut self.frames[frame_index];
        let needed = self.uniform_stride * staged.draws.len().max(1) as vk::DeviceSize;
        if needed > frame.uniforms.size {
            let mut old = std::mem::replace(&mut frame.uniforms, AllocatedBuffer::null());
            destroy_cpu_buffer(device, allocator, &mut old);
            frame.uniforms = create_cpu_buffer(
                device,
                allocator,
                needed.next_power_of_two(),
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                "scene-uniforms",
            );
            Self::write_descriptor(device, frame);
        }
        let window_size = [self.extent.width as f32, self.extent.height as f32];
        let mapped = frame.uniforms.allocation.as_mut().unwrap().mapped_slice_mut().unwrap();
        for (i, draw) in staged.draws.iter().enumerate() {
            let uniforms = SceneUniforms {
                mvp: draw.mvp,
                window_size,
                window_radius: corner_radius_px,
                _padding: 0.0,
            };
            let offset = (self.uniform_stride as usize) * i;
            mapped[offset..offset + UNIFORM_SIZE as usize]
                .copy_from_slice(bytemuck::bytes_of(&uniforms));
        }
        frame.draw_count = staged.draws.len() as u32;
    }

    /// Record the offscreen scene pass. Consumes the staged scene; afterwards the
    /// backdrop is in TRANSFER_SRC layout, ready for the swapchain copy. Returns
    /// false if nothing was staged.
    pub(crate) fn record(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: usize,
    ) -> bool {
        let Some(staged) = self.staged.take() else {
            return false;
        };
        let frame = &self.frames[frame_index];
        unsafe {
            let clear_values = [
                vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] } },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 },
                },
            ];
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: self.extent,
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            // Negative-height viewport: wgpu's Y-up NDC without touching winding.
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: self.extent.height as f32,
                    width: self.extent.width as f32,
                    height: -(self.extent.height as f32),
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            let (sx, sy, sw, sh) = staged.scissor;
            let sx = sx.min(self.extent.width);
            let sy = sy.min(self.extent.height);
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: sx as i32, y: sy as i32 },
                    extent: vk::Extent2D {
                        width: sw.min(self.extent.width - sx),
                        height: sh.min(self.extent.height - sy),
                    },
                }],
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            for (i, draw) in staged.draws.iter().enumerate() {
                let mesh = &self.meshes[draw.mesh.0];
                if mesh.count == 0 {
                    continue;
                }
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[frame.descriptor_set],
                    &[(self.uniform_stride as u32) * i as u32],
                );
                device.cmd_bind_vertex_buffers(cmd, 0, &[mesh.buffer.buffer], &[0]);
                device.cmd_draw(cmd, mesh.count, 1, 0, 0);
            }
            device.cmd_end_render_pass(cmd);
        }
        self.backdrop_valid = true;
        true
    }

    pub(crate) fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        self.destroy_targets(device, allocator);
        unsafe {
            for frame in &mut self.frames {
                let mut uniforms = std::mem::replace(&mut frame.uniforms, AllocatedBuffer::null());
                destroy_cpu_buffer(device, allocator, &mut uniforms);
            }
            for mesh in &mut self.meshes {
                let mut buffer = std::mem::replace(&mut mesh.buffer, AllocatedBuffer::null());
                destroy_cpu_buffer(device, allocator, &mut buffer);
            }
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_shader_module(self.shader_module, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

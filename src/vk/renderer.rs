//! The ash renderer. One graphics queue, a classic render pass, two frames in
//! flight, FIFO (vsync) presentation. Memory goes through gpu-allocator; the
//! descriptor set mirrors `shader.wgsl`'s @group(0): sampled backdrop texture
//! (binding 0), sampler (binding 1), WindowInfo uniform (binding 2). The backdrop
//! is a 1x1 placeholder until the blur-behind path is wired to a real framebuffer
//! copy at cutover.

use std::ffi::{c_void, CStr, CString};

use ash::vk;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc,
};
use gpu_allocator::MemoryLocation;

use cce_ui::engine::Vertex;

use super::text::{TextSpan, TextStage};

const FRAMES_IN_FLIGHT: usize = 2;
const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

pub(crate) struct AllocatedBuffer {
    pub(crate) buffer: vk::Buffer,
    pub(crate) allocation: Option<Allocation>,
    pub(crate) size: vk::DeviceSize,
}

impl AllocatedBuffer {
    pub(crate) fn null() -> Self {
        AllocatedBuffer { buffer: vk::Buffer::null(), allocation: None, size: 0 }
    }
}

/// Create a host-visible buffer bound to gpu-allocator memory.
pub(crate) fn create_cpu_buffer(
    device: &ash::Device,
    allocator: &mut Allocator,
    size: vk::DeviceSize,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> AllocatedBuffer {
    unsafe {
        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .expect("Failed to create buffer");
        let requirements = device.get_buffer_memory_requirements(buffer);
        let allocation = allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .expect("Failed to allocate buffer memory");
        device
            .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
            .expect("Failed to bind buffer memory");
        AllocatedBuffer { buffer, allocation: Some(allocation), size }
    }
}

/// Destroy a buffer and return its memory to the allocator.
pub(crate) fn destroy_cpu_buffer(
    device: &ash::Device,
    allocator: &mut Allocator,
    buf: &mut AllocatedBuffer,
) {
    unsafe {
        self::destroy_buffer_handle(device, buf.buffer);
    }
    if let Some(allocation) = buf.allocation.take() {
        let _ = allocator.free(allocation);
    }
    buf.buffer = vk::Buffer::null();
    buf.size = 0;
}

unsafe fn destroy_buffer_handle(device: &ash::Device, buffer: vk::Buffer) {
    if buffer != vk::Buffer::null() {
        device.destroy_buffer(buffer, None);
    }
}

struct Frame {
    cmd: vk::CommandBuffer,
    image_available: vk::Semaphore,
    in_flight: vk::Fence,
    vertex: AllocatedBuffer,
    vertex_count: u32,
}

pub struct VkRenderer {
    _entry: ash::Entry,
    instance: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    allocator: Option<Allocator>,

    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    surface_format: vk::SurfaceFormatKHR,
    extent: vk::Extent2D,
    swapchain_views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    // One per swapchain image (not per frame in flight): present waits on the
    // semaphore tied to the image being presented.
    render_finished: Vec<vk::Semaphore>,

    render_pass: vk::RenderPass,
    descriptor_set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    shader_module: vk::ShaderModule,

    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    backdrop_image: vk::Image,
    backdrop_view: vk::ImageView,
    backdrop_allocation: Option<Allocation>,
    backdrop_sampler: vk::Sampler,
    window_info: AllocatedBuffer,

    command_pool: vk::CommandPool,
    frames: Vec<Frame>,
    frame_index: usize,
    text: TextStage,

    desired_extent: vk::Extent2D,
    corner_radius_px: f32,
    swapchain_dirty: bool,
}

/// Compile WGSL to SPIR-V with the same coordinate-space adjustment wgpu applies
/// (wgpu NDC is Y-up; ADJUST_COORDINATE_SPACE emits the Vulkan Y-flip), so the
/// existing NDC math in the app carries over unchanged.
pub(crate) fn compile_wgsl(source: &str) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source).expect("WGSL parse failed");
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("WGSL validation failed");
    let options = naga::back::spv::Options {
        lang_version: (1, 0),
        flags: naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE
            | naga::back::spv::WriterFlags::LABEL_VARYINGS,
        ..Default::default()
    };
    naga::back::spv::write_vec(&module, &info, &options, None).expect("SPIR-V write failed")
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    if data.is_null() {
        return vk::FALSE;
    }
    let message = unsafe {
        let p = (*data).p_message;
        if p.is_null() {
            return vk::FALSE;
        }
        CStr::from_ptr(p).to_string_lossy()
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        log::error!("[vulkan] {message}");
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        log::warn!("[vulkan] {message}");
    } else {
        log::debug!("[vulkan] {message}");
    }
    vk::FALSE
}

impl VkRenderer {
    /// # Safety
    /// `display_ptr` and `surface_ptr` must be live `wl_display` / `wl_surface`
    /// pointers that outlive the renderer (same contract as `WgpuAdapter::new`).
    pub unsafe fn new(
        display_ptr: *mut c_void,
        surface_ptr: *mut c_void,
        width: u32,
        height: u32,
        corner_radius_px: f32,
    ) -> Self {
        let entry = ash::Entry::load().expect("Failed to load libvulkan");

        // Instance, with validation when available (debug builds or CCE_VK_VALIDATION=1).
        let want_validation =
            cfg!(debug_assertions) || std::env::var_os("CCE_VK_VALIDATION").is_some();
        let validation_available = want_validation
            && entry
                .enumerate_instance_layer_properties()
                .map(|layers| {
                    layers.iter().any(|l| {
                        CStr::from_ptr(l.layer_name.as_ptr()) == VALIDATION_LAYER
                    })
                })
                .unwrap_or(false);
        if want_validation && !validation_available {
            log::warn!("Vulkan validation requested but VK_LAYER_KHRONOS_validation is not installed");
        }

        let api_version = match entry.try_enumerate_instance_version().ok().flatten() {
            Some(v) if v >= vk::API_VERSION_1_2 => vk::API_VERSION_1_2,
            Some(v) => v,
            None => vk::API_VERSION_1_0,
        };
        let app_name = c"cce-designer";
        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name)
            .engine_name(app_name)
            .api_version(api_version);

        let mut extension_names = vec![
            ash::khr::surface::NAME.as_ptr(),
            ash::khr::wayland_surface::NAME.as_ptr(),
        ];
        if validation_available {
            extension_names.push(ash::ext::debug_utils::NAME.as_ptr());
        }
        let layer_names_owned: Vec<CString> = if validation_available {
            vec![VALIDATION_LAYER.to_owned()]
        } else {
            Vec::new()
        };
        let layer_names: Vec<*const i8> =
            layer_names_owned.iter().map(|l| l.as_ptr()).collect();

        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app_info)
                    .enabled_extension_names(&extension_names)
                    .enabled_layer_names(&layer_names),
                None,
            )
            .expect("Failed to create Vulkan instance");

        let debug = if validation_available {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let messenger = loader
                .create_debug_utils_messenger(
                    &vk::DebugUtilsMessengerCreateInfoEXT::default()
                        .message_severity(
                            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                        )
                        .message_type(
                            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                        )
                        .pfn_user_callback(Some(debug_callback)),
                    None,
                )
                .expect("Failed to create debug messenger");
            log::info!("Vulkan validation layers enabled");
            Some((loader, messenger))
        } else {
            None
        };

        // Wayland surface from the same raw pointers WgpuAdapter uses.
        let wayland_loader = ash::khr::wayland_surface::Instance::new(&entry, &instance);
        let surface = wayland_loader
            .create_wayland_surface(
                &vk::WaylandSurfaceCreateInfoKHR::default()
                    .display(display_ptr)
                    .surface(surface_ptr),
                None,
            )
            .expect("Failed to create Wayland surface");
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);

        // Physical device + queue family: graphics with present support on this
        // surface. Prefer integrated (matches WgpuAdapter's LowPower preference).
        let mut candidates: Vec<(vk::PhysicalDevice, u32, i32)> = Vec::new();
        for pd in instance
            .enumerate_physical_devices()
            .expect("No Vulkan physical devices")
        {
            let families = instance.get_physical_device_queue_family_properties(pd);
            let family = families.iter().enumerate().find_map(|(i, f)| {
                let graphics = f.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                let present = surface_loader
                    .get_physical_device_surface_support(pd, i as u32, surface)
                    .unwrap_or(false);
                (graphics && present).then_some(i as u32)
            });
            if let Some(family) = family {
                let props = instance.get_physical_device_properties(pd);
                let rank = match props.device_type {
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 0,
                    vk::PhysicalDeviceType::DISCRETE_GPU => 1,
                    vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
                    _ => 3,
                };
                candidates.push((pd, family, rank));
            }
        }
        candidates.sort_by_key(|&(_, _, rank)| rank);
        let (physical_device, queue_family, _) = *candidates
            .first()
            .expect("No Vulkan device supports this Wayland surface");
        {
            let props = instance.get_physical_device_properties(physical_device);
            let name = CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy();
            log::info!("Vulkan device: {name}");
        }

        let queue_priorities = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&queue_priorities)];
        let device_extensions = [ash::khr::swapchain::NAME.as_ptr()];
        let device = instance
            .create_device(
                physical_device,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queue_infos)
                    .enabled_extension_names(&device_extensions),
                None,
            )
            .expect("Failed to create Vulkan device");
        let queue = device.get_device_queue(queue_family, 0);

        let mut allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })
        .expect("Failed to create GPU allocator");

        // Surface format: prefer sRGB (wgpu's get_default_config sorts sRGB first,
        // so this matches the colors the app renders today).
        let formats = surface_loader
            .get_physical_device_surface_formats(physical_device, surface)
            .expect("No surface formats");
        let surface_format = formats
            .iter()
            .copied()
            .find(|f| {
                (f.format == vk::Format::B8G8R8A8_SRGB || f.format == vk::Format::R8G8B8A8_SRGB)
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(formats[0]);

        // Render pass: one color attachment, clear -> present.
        let attachments = [vk::AttachmentDescription::default()
            .format(surface_format.format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
        let color_refs = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs)];
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
        let render_pass = device
            .create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&dependencies),
                None,
            )
            .expect("Failed to create render pass");

        // Descriptor set layout mirroring shader.wgsl @group(0): naga maps WGSL
        // texture/sampler/uniform bindings 1:1 onto set 0 descriptor bindings.
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
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let descriptor_set_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
            .expect("Failed to create descriptor set layout");

        let set_layouts = [descriptor_set_layout];
        let pipeline_layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                None,
            )
            .expect("Failed to create pipeline layout");

        // Pipeline from shader.wgsl (both entry points live in one SPIR-V module).
        let spirv = compile_wgsl(include_str!("../shader.wgsl"));
        let shader_module = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&spirv), None)
            .expect("Failed to create shader module");

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

        // Vertex layout = cce_ui::engine::Vertex: pos vec2f, color vec4f, clip vec3f.
        let vertex_bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<Vertex>() as u32)
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
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(24),
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
        // wgpu::BlendState::ALPHA_BLENDING.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
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
            .expect("Failed to create graphics pipeline")[0];

        let command_pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(queue_family),
                None,
            )
            .expect("Failed to create command pool");

        // Backdrop placeholder: 1x1 black, cleared + transitioned once at startup.
        let backdrop_image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D { width: 1, height: 1, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .expect("Failed to create backdrop image");
        let backdrop_requirements = device.get_image_memory_requirements(backdrop_image);
        let backdrop_allocation = allocator
            .allocate(&AllocationCreateDesc {
                name: "backdrop",
                requirements: backdrop_requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .expect("Failed to allocate backdrop memory");
        device
            .bind_image_memory(
                backdrop_image,
                backdrop_allocation.memory(),
                backdrop_allocation.offset(),
            )
            .expect("Failed to bind backdrop memory");

        let subresource_range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);

        // One-time init: clear the backdrop and move it to SHADER_READ_ONLY.
        {
            let cmd = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(command_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .expect("Failed to allocate init command buffer")[0];
            device
                .begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .unwrap();
            let to_transfer = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(backdrop_image)
                .subresource_range(subresource_range);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            device.cmd_clear_color_image(
                cmd,
                backdrop_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] },
                &[subresource_range],
            );
            let to_sampled = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(backdrop_image)
                .subresource_range(subresource_range);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_sampled],
            );
            device.end_command_buffer(cmd).unwrap();
            let cmds = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmds);
            device
                .queue_submit(queue, &[submit], vk::Fence::null())
                .expect("Init submit failed");
            device.queue_wait_idle(queue).expect("Init wait failed");
            device.free_command_buffers(command_pool, &cmds);
        }

        let backdrop_view = device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(backdrop_image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(subresource_range),
                None,
            )
            .expect("Failed to create backdrop view");

        // Matches the wgpu backdrop sampler: linear, clamp-to-edge.
        let backdrop_sampler = device
            .create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
            .expect("Failed to create sampler");

        let window_info = create_cpu_buffer(
            &device,
            &mut allocator,
            16,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            "window-info",
        );

        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1),
        ];
        let descriptor_pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )
            .expect("Failed to create descriptor pool");
        let descriptor_set = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&set_layouts),
            )
            .expect("Failed to allocate descriptor set")[0];

        let image_infos = [vk::DescriptorImageInfo::default()
            .image_view(backdrop_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_infos = [vk::DescriptorImageInfo::default().sampler(backdrop_sampler)];
        let buffer_infos = [vk::DescriptorBufferInfo::default()
            .buffer(window_info.buffer)
            .offset(0)
            .range(16)];
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
                vk::WriteDescriptorSet::default()
                    .dst_set(descriptor_set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer_infos),
            ],
            &[],
        );

        // Per-frame command buffers, sync, and vertex buffers.
        let cmds = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(FRAMES_IN_FLIGHT as u32),
            )
            .expect("Failed to allocate command buffers");
        let frames = cmds
            .into_iter()
            .map(|cmd| Frame {
                cmd,
                image_available: device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .unwrap(),
                in_flight: device
                    .create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                    .unwrap(),
                vertex: create_cpu_buffer(
                    &device,
                    &mut allocator,
                    64 * 1024,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    "vertices",
                ),
                vertex_count: 0,
            })
            .collect();

        let text = TextStage::new(&device, &mut allocator, render_pass, FRAMES_IN_FLIGHT);

        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
        let mut renderer = Self {
            _entry: entry,
            instance,
            debug,
            surface_loader,
            surface,
            physical_device,
            device,
            queue,
            allocator: Some(allocator),
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            surface_format,
            extent: vk::Extent2D { width: width.max(1), height: height.max(1) },
            swapchain_views: Vec::new(),
            framebuffers: Vec::new(),
            render_finished: Vec::new(),
            render_pass,
            descriptor_set_layout,
            pipeline_layout,
            pipeline,
            shader_module,
            descriptor_pool,
            descriptor_set,
            backdrop_image,
            backdrop_view,
            backdrop_allocation: Some(backdrop_allocation),
            backdrop_sampler,
            window_info,
            command_pool,
            frames,
            frame_index: 0,
            text,
            desired_extent: vk::Extent2D { width: width.max(1), height: height.max(1) },
            corner_radius_px,
            swapchain_dirty: false,
        };
        renderer.create_swapchain();
        renderer.write_window_info();
        renderer
    }

    fn write_window_info(&mut self) {
        let data = [
            self.extent.width as f32,
            self.extent.height as f32,
            self.corner_radius_px,
            0.0f32,
        ];
        if let Some(allocation) = self.window_info.allocation.as_mut() {
            allocation.mapped_slice_mut().unwrap()[..16]
                .copy_from_slice(bytemuck::cast_slice(&data));
        }
    }

    fn destroy_swapchain_resources(&mut self) {
        unsafe {
            for fb in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(fb, None);
            }
            for view in self.swapchain_views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            for sem in self.render_finished.drain(..) {
                self.device.destroy_semaphore(sem, None);
            }
        }
    }

    fn create_swapchain(&mut self) {
        unsafe {
            let caps = self
                .surface_loader
                .get_physical_device_surface_capabilities(self.physical_device, self.surface)
                .expect("Failed to query surface capabilities");

            // Wayland reports "extent defined by the swapchain" (u32::MAX); use the
            // size the configure events gave us.
            let extent = if caps.current_extent.width != u32::MAX {
                caps.current_extent
            } else {
                vk::Extent2D {
                    width: self
                        .desired_extent
                        .width
                        .clamp(caps.min_image_extent.width, caps.max_image_extent.width.max(1)),
                    height: self
                        .desired_extent
                        .height
                        .clamp(caps.min_image_extent.height, caps.max_image_extent.height.max(1)),
                }
            };

            let mut image_count = caps.min_image_count + 1;
            if caps.max_image_count > 0 {
                image_count = image_count.min(caps.max_image_count);
            }

            // Prefer premultiplied (what the DE's other clients pick), else opaque,
            // else whatever the surface offers.
            let composite_alpha = [
                vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
                vk::CompositeAlphaFlagsKHR::INHERIT,
            ]
            .into_iter()
            .find(|&mode| caps.supported_composite_alpha.contains(mode))
            .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);

            let old_swapchain = self.swapchain;
            self.swapchain = self
                .swapchain_loader
                .create_swapchain(
                    &vk::SwapchainCreateInfoKHR::default()
                        .surface(self.surface)
                        .min_image_count(image_count)
                        .image_format(self.surface_format.format)
                        .image_color_space(self.surface_format.color_space)
                        .image_extent(extent)
                        .image_array_layers(1)
                        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .pre_transform(caps.current_transform)
                        .composite_alpha(composite_alpha)
                        .present_mode(vk::PresentModeKHR::FIFO)
                        .clipped(true)
                        .old_swapchain(old_swapchain),
                    None,
                )
                .expect("Failed to create swapchain");
            if old_swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader.destroy_swapchain(old_swapchain, None);
            }
            self.extent = extent;

            let images = self
                .swapchain_loader
                .get_swapchain_images(self.swapchain)
                .expect("Failed to get swapchain images");
            let subresource_range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1);
            for image in &images {
                let view = self
                    .device
                    .create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(*image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(self.surface_format.format)
                            .subresource_range(subresource_range),
                        None,
                    )
                    .expect("Failed to create swapchain view");
                self.swapchain_views.push(view);
                let attachments = [view];
                let fb = self
                    .device
                    .create_framebuffer(
                        &vk::FramebufferCreateInfo::default()
                            .render_pass(self.render_pass)
                            .attachments(&attachments)
                            .width(extent.width)
                            .height(extent.height)
                            .layers(1),
                        None,
                    )
                    .expect("Failed to create framebuffer");
                self.framebuffers.push(fb);
                self.render_finished.push(
                    self.device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                        .unwrap(),
                );
            }
        }
    }

    fn recreate_swapchain(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
        }
        self.destroy_swapchain_resources();
        self.create_swapchain();
        self.write_window_info();
    }

    /// Request a new physical size (from xdg configure / scale changes). Applied
    /// lazily on the next `draw_frame`.
    pub fn resize(&mut self, width: u32, height: u32) {
        let extent = vk::Extent2D { width: width.max(1), height: height.max(1) };
        if extent.width != self.extent.width || extent.height != self.extent.height {
            self.desired_extent = extent;
            self.swapchain_dirty = true;
        }
    }

    // Used at cutover, when scale changes re-derive the radius; vk-smoke fixes it at init.
    #[allow(dead_code)]
    pub fn set_corner_radius(&mut self, radius_px: f32) {
        self.corner_radius_px = radius_px;
        // Written on the next swapchain rebuild or draw-idle moment; a mapped write
        // here would race in-flight frames, so route it through the dirty path.
        self.swapchain_dirty = true;
    }

    /// Stage text for the next `draw_frame`: shape-cache misses are rasterized
    /// into the glyph atlas and vertices are built against the current extent.
    /// Mirrors `glyphon::TextRenderer::prepare`.
    pub fn prepare_text(
        &mut self,
        font_system: &mut glyphon::FontSystem,
        swash_cache: &mut glyphon::SwashCache,
        spans: &[TextSpan<'_>],
    ) {
        self.text.prepare(font_system, swash_cache, spans, self.extent);
    }

    /// Render one frame: 2D geometry, then any text staged via `prepare_text`.
    /// Returns false if the frame was skipped (swapchain rebuild); the caller
    /// just draws again next tick.
    pub fn draw_frame(&mut self, verts: &[Vertex]) -> bool {
        if self.swapchain_dirty {
            self.swapchain_dirty = false;
            self.recreate_swapchain();
        }

        unsafe {
            let frame_index = self.frame_index;
            let (in_flight, image_available) = {
                let f = &self.frames[frame_index];
                (f.in_flight, f.image_available)
            };
            self.device
                .wait_for_fences(&[in_flight], true, u64::MAX)
                .expect("Fence wait failed");

            let image_index = match self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                image_available,
                vk::Fence::null(),
            ) {
                Ok((index, suboptimal)) => {
                    if suboptimal {
                        self.swapchain_dirty = true;
                    }
                    index
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.swapchain_dirty = true;
                    return false;
                }
                Err(e) => {
                    log::error!("acquire_next_image failed: {e:?}");
                    return false;
                }
            };

            self.device.reset_fences(&[in_flight]).unwrap();

            // Upload vertices into this frame's buffer (its fence has signaled, so
            // the GPU is done with it; growing swaps in a fresh buffer).
            let bytes: &[u8] = bytemuck::cast_slice(verts);
            let needed = bytes.len() as vk::DeviceSize;
            if needed > self.frames[frame_index].vertex.size {
                let mut old =
                    std::mem::replace(&mut self.frames[frame_index].vertex, AllocatedBuffer::null());
                let allocator = self.allocator.as_mut().unwrap();
                destroy_cpu_buffer(&self.device, allocator, &mut old);
                self.frames[frame_index].vertex = create_cpu_buffer(
                    &self.device,
                    allocator,
                    needed.next_power_of_two(),
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    "vertices",
                );
            }
            if !bytes.is_empty() {
                self.frames[frame_index]
                    .vertex
                    .allocation
                    .as_mut()
                    .unwrap()
                    .mapped_slice_mut()
                    .unwrap()[..bytes.len()]
                    .copy_from_slice(bytes);
            }
            self.frames[frame_index].vertex_count = verts.len() as u32;
            self.text.write_frame_buffers(
                &self.device,
                self.allocator.as_mut().unwrap(),
                frame_index,
            );

            // Record.
            let frame = &self.frames[frame_index];
            let cmd = frame.cmd;
            self.device
                .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
                .unwrap();
            self.text.record_upload(&self.device, cmd, frame_index);
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] },
            }];
            self.device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffers[image_index as usize])
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: self.extent,
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: self.extent.width as f32,
                    height: self.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.extent,
                }],
            );
            if frame.vertex_count > 0 {
                self.device
                    .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
                self.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[self.descriptor_set],
                    &[],
                );
                self.device
                    .cmd_bind_vertex_buffers(cmd, 0, &[frame.vertex.buffer], &[0]);
                self.device.cmd_draw(cmd, frame.vertex_count, 1, 0, 0);
            }
            self.text.record_draw(&self.device, cmd, frame_index);
            self.device.cmd_end_render_pass(cmd);
            self.device.end_command_buffer(cmd).unwrap();

            // Submit + present.
            let wait_semaphores = [image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let cmds = [cmd];
            let signal_semaphores = [self.render_finished[image_index as usize]];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmds)
                .signal_semaphores(&signal_semaphores);
            self.device
                .queue_submit(self.queue, &[submit], in_flight)
                .expect("Queue submit failed");

            let swapchains = [self.swapchain];
            let image_indices = [image_index];
            let present = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            match self.swapchain_loader.queue_present(self.queue, &present) {
                Ok(suboptimal) => {
                    if suboptimal {
                        self.swapchain_dirty = true;
                    }
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.swapchain_dirty = true;
                }
                Err(e) => log::error!("queue_present failed: {e:?}"),
            }

            self.frame_index = (self.frame_index + 1) % FRAMES_IN_FLIGHT;
        }
        true
    }
}

impl Drop for VkRenderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();

            let mut frames = std::mem::take(&mut self.frames);
            for frame in &mut frames {
                self.device.destroy_semaphore(frame.image_available, None);
                self.device.destroy_fence(frame.in_flight, None);
                let mut vertex = std::mem::replace(&mut frame.vertex, AllocatedBuffer::null());
                if let Some(allocator) = self.allocator.as_mut() {
                    destroy_cpu_buffer(&self.device, allocator, &mut vertex);
                }
            }

            self.destroy_swapchain_resources();
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader.destroy_swapchain(self.swapchain, None);
            }

            if let Some(allocator) = self.allocator.as_mut() {
                self.text.destroy(&self.device, allocator);
            }

            self.device.destroy_sampler(self.backdrop_sampler, None);
            self.device.destroy_image_view(self.backdrop_view, None);
            self.device.destroy_image(self.backdrop_image, None);
            if let (Some(allocator), Some(allocation)) =
                (self.allocator.as_mut(), self.backdrop_allocation.take())
            {
                let _ = allocator.free(allocation);
            }
            let mut window_info = std::mem::replace(&mut self.window_info, AllocatedBuffer::null());
            if let Some(allocator) = self.allocator.as_mut() {
                destroy_cpu_buffer(&self.device, allocator, &mut window_info);
            }

            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_shader_module(self.shader_module, None);
            self.device.destroy_render_pass(self.render_pass, None);
            self.device.destroy_command_pool(self.command_pool, None);

            // The allocator must go before the device it allocates from.
            drop(self.allocator.take());

            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            if let Some((loader, messenger)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

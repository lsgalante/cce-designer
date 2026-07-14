//! Raw-Vulkan (ash) rendering foundation — milestone 1 of the wgpu → ash migration.
//!
//! `VkRenderer` reproduces the designer's 2D UI pipeline (`shader.wgsl`: NDC quads
//! with window-corner rounding, circle clipping, and the blur-behind branch) directly
//! on Vulkan: instance (+ validation layers when available), VK_KHR_wayland_surface,
//! swapchain, gpu-allocator memory, and the WGSL compiled to SPIR-V through naga at
//! startup — so `shader.wgsl` stays the single source of truth for both stacks.
//!
//! The 3D pass, glyphon text, and curved-text passes stay on wgpu until the cutover;
//! `vk-smoke` is the standalone proof of this renderer.

mod renderer;
mod text;

pub use renderer::VkRenderer;
pub use text::TextSpan;

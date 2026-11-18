//! One GPU per test process: the instance, adapter and device are made once and never dropped
//! (dropping an instance is the racy call). Works headless on lavapipe, Metal and DX12.
use std::sync::OnceLock;

static GPU: OnceLock<(wgpu::Instance, wgpu::Device, wgpu::Queue)> = OnceLock::new();

pub(crate) fn shared() -> (wgpu::Device, wgpu::Queue) {
    let (_, device, queue) = GPU.get_or_init(|| {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(desc);
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        (instance, device, queue)
    });
    (device.clone(), queue.clone())
}

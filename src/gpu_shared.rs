//! The GPU every egui window opens on, as one handle. A window opener takes a `&GpuShared`
//! instead of four wgpu objects, so a second backend adds one arm here and not one parameter
//! list per opener.

/// The shared device and what a window needs to draw on it. Each wgpu object is a counted
/// handle, so a clone is cheap and shares the same device.
#[derive(Clone)]
pub enum GpuShared {
    Wgpu { instance: wgpu::Instance, adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue },
    /// The native Metal device the terminal windows draw on (macOS).
    #[cfg(target_os = "macos")]
    Metal(crate::metal_backend::MetalShared),
}

impl GpuShared {
    /// The four wgpu objects of a wgpu GPU; an error for any other.
    pub fn wgpu(&self) -> Result<(&wgpu::Instance, &wgpu::Adapter, &wgpu::Device, &wgpu::Queue), String> {
        match self {
            GpuShared::Wgpu { instance, adapter, device, queue } => Ok((instance, adapter, device, queue)),
            #[cfg(target_os = "macos")]
            GpuShared::Metal(_) => Err("not a wgpu device".to_string()),
        }
    }

    /// The renderer line About shows: the API and the GPU.
    pub fn renderer_name(&self) -> String {
        match self {
            GpuShared::Wgpu { adapter, .. } => crate::about_ui::renderer_line(&adapter.get_info()),
            #[cfg(target_os = "macos")]
            GpuShared::Metal(m) => format!("Metal, {}", m.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    /// Every `.rs` file under `dir`.
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                sources(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// The 1-based lines that still pass the shared GPU as separate wgpu objects: the argument
    /// list, the underscore stub, the four-object tuple (`prefs_gpu`) and the one-per-line form.
    fn offending(text: &str) -> Vec<usize> {
        let lines: Vec<&str> = text.lines().collect();
        let mut out = Vec::new();
        for (n, line) in lines.iter().enumerate() {
            let t = line.trim();
            let same_line = line.contains("&g.instance, &g.adapter")
                || line.contains("instance: &wgpu::Instance, adapter: &wgpu::Adapter")
                || line.contains("_instance: &wgpu::Instance, _adapter: &wgpu::Adapter")
                || line.contains("(&wgpu::Instance, &wgpu::Adapter")
                || line.contains("(wgpu::Instance, wgpu::Adapter")
                || line.contains("gpu.0, gpu.1, gpu.2, gpu.3");
            let stacked = matches!(t, "instance: &wgpu::Instance," | "_instance: &wgpu::Instance,") && lines.get(n + 1).is_some_and(|l| matches!(l.trim(), "adapter: &wgpu::Adapter," | "_adapter: &wgpu::Adapter,"));
            if same_line || stacked {
                out.push(n + 1);
            }
        }
        out
    }

    #[test]
    fn no_opener_takes_four_wgpu_objects() {
        let mut files = Vec::new();
        sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut bad = Vec::new();
        for f in files {
            // render.rs keeps the shared test device as a plain tuple; it opens no window.
            if f.ends_with("gpu_shared.rs") || f.ends_with("render.rs") {
                continue;
            }
            for n in offending(&std::fs::read_to_string(&f).unwrap()) {
                bad.push(format!("{}:{n}", f.display()));
            }
        }
        assert!(bad.is_empty(), "openers still take four wgpu objects: {bad:?}");
    }

    /// Each old shape, planted, is caught.
    #[test]
    fn the_scan_catches_every_old_shape() {
        for planted in [
            "x(event_loop, &g.instance, &g.adapter, &g.device, &g.queue)",
            "fn open(instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue)",
            "fn open(_instance: &wgpu::Instance, _adapter: &wgpu::Adapter, _device: &wgpu::Device, _queue: &wgpu::Queue)",
            "prefs_gpu: Option<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue)>,",
            "gpu: (&wgpu::Instance, &wgpu::Adapter, &wgpu::Device, &wgpu::Queue),",
            "host(gpu.0, gpu.1, gpu.2, gpu.3)",
            "    _instance: &wgpu::Instance,\n    _adapter: &wgpu::Adapter,\n",
            "    instance: &wgpu::Instance,\n    adapter: &wgpu::Adapter,\n",
        ] {
            assert!(!offending(planted).is_empty(), "{planted}");
        }
        assert!(offending("fn open(gpu: &GpuShared)").is_empty());
    }
}

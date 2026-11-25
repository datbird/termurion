//! The GPU every egui window opens on, as one handle. A window opener takes a `&GpuShared`
//! instead of four wgpu objects, so a second backend adds one arm here and not one parameter
//! list per opener.

/// The shared device and what a window needs to draw on it. Each wgpu object is a counted
/// handle, so a clone is cheap and shares the same device.
#[derive(Clone)]
pub enum GpuShared {
    Wgpu { instance: wgpu::Instance, adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue },
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

    #[test]
    fn no_opener_takes_four_wgpu_objects() {
        let mut files = Vec::new();
        sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut bad = Vec::new();
        for f in files {
            if f.ends_with("gpu_shared.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for (n, line) in text.lines().enumerate() {
                let four_args = line.contains("&g.instance, &g.adapter") || line.contains("instance: &wgpu::Instance, adapter: &wgpu::Adapter") || line.contains("(&wgpu::Instance, &wgpu::Adapter") || line.contains("gpu.0, gpu.1, gpu.2, gpu.3");
                let multi = line.trim() == "instance: &wgpu::Instance," && text.lines().nth(n + 1).is_some_and(|l| l.trim() == "adapter: &wgpu::Adapter,") && !f.ends_with("render.rs");
                if four_args || multi {
                    bad.push(format!("{}:{}", f.display(), n + 1));
                }
            }
        }
        assert!(bad.is_empty(), "openers still take four wgpu objects: {bad:?}");
    }
}

//! Shaders to Metal shading language the way wgpu does it, so the native backend runs the same
//! MSL wgpu's Metal backend runs. The options, the slot numbering and the vertex pulling are
//! copied from wgpu-hal 30.0.1, `src/metal/device.rs` (`load_shader`, `create_pipeline_layout`
//! and `create_render_pipeline`).

use naga::back::msl;
use naga::ShaderStage;

/// The buffer slots Metal has per stage; wgpu-hal puts vertex buffers at the top, counting down.
pub const MAX_BUFFERS: u32 = 31;

/// The Metal buffer slot of vertex buffer `i` (wgpu-hal `set_vertex_buffer`).
pub fn vertex_buffer_slot(i: u32) -> u32 {
    MAX_BUFFERS - 1 - i
}

/// What a bind group entry is, as far as slot numbering goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Buffer,
    Texture,
    Sampler,
}

/// One bind group layout entry: its binding, kind and the stages that see it.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub binding: u32,
    pub kind: Kind,
    pub vertex: bool,
    pub fragment: bool,
}

impl Entry {
    pub const fn new(binding: u32, kind: Kind, vertex: bool, fragment: bool) -> Entry {
        Entry { binding, kind, vertex, fragment }
    }

    fn seen_by(&self, stage: ShaderStage) -> bool {
        match stage {
            ShaderStage::Vertex => self.vertex,
            ShaderStage::Fragment => self.fragment,
            _ => false,
        }
    }
}

/// One stage's slots for a pipeline layout of `groups` (bind groups in order, each in its
/// entry order): a counter per resource kind, then the sizes buffer, which the vertex stage
/// always has (vertex pulling reads the buffer sizes from it). No immediates, no storage
/// buffers, no binding arrays here.
pub fn stage_resources(groups: &[&[Entry]], stage: ShaderStage) -> msl::EntryPointResources {
    let (mut buffers, mut textures, mut samplers) = (0u8, 0u8, 0u8);
    let mut resources = msl::BindingMap::default();
    for (group, entries) in groups.iter().enumerate() {
        for e in entries.iter().filter(|e| e.seen_by(stage)) {
            let mut target = msl::BindTarget::default();
            match e.kind {
                Kind::Buffer => {
                    target.buffer = Some(buffers);
                    buffers += 1;
                }
                Kind::Texture => {
                    target.texture = Some(textures);
                    textures += 1;
                }
                Kind::Sampler => {
                    target.sampler = Some(msl::BindSamplerTarget::Resource(samplers));
                    samplers += 1;
                }
            }
            resources.insert(naga::ResourceBinding { group: group as u32, binding: e.binding }, target);
        }
    }
    let sizes_buffer = (stage == ShaderStage::Vertex).then_some(buffers);
    msl::EntryPointResources { resources, immediates_buffer: None, sizes_buffer }
}

/// The slots of a layout read off the module: every global with a binding, by group and then
/// binding, seen by the stage that uses it. Right for layouts whose entries are in binding order
/// and visible only where used (the effects and the tests).
pub fn module_resources(module: &naga::Module, info: &naga::valid::ModuleInfo, stage: ShaderStage, entry: &str) -> Result<msl::EntryPointResources, String> {
    let index = module.entry_points.iter().position(|ep| ep.stage == stage && ep.name == entry).ok_or_else(|| format!("no {stage:?} entry point {entry}"))?;
    let used = info.get_entry_point(index);
    let mut bound: Vec<(u32, u32, Kind)> = Vec::new();
    for (handle, var) in module.global_variables.iter() {
        let (Some(b), false) = (var.binding, used[handle].is_empty()) else { continue };
        let kind = match module.types[var.ty].inner {
            naga::TypeInner::Image { .. } => Kind::Texture,
            naga::TypeInner::Sampler { .. } => Kind::Sampler,
            _ => Kind::Buffer,
        };
        bound.push((b.group, b.binding, kind));
    }
    bound.sort_by_key(|&(g, b, _)| (g, b));
    let groups = bound.last().map_or(0, |&(g, ..)| g as usize + 1);
    let entries: Vec<Vec<Entry>> = (0..groups).map(|g| bound.iter().filter(|b| b.0 as usize == g).map(|&(_, b, k)| Entry::new(b, k, stage == ShaderStage::Vertex, stage == ShaderStage::Fragment)).collect()).collect();
    let refs: Vec<&[Entry]> = entries.iter().map(Vec::as_slice).collect();
    Ok(stage_resources(&refs, stage))
}

/// The MSL version wgpu-hal picks for this OS (`PrivateCapabilities::new`).
#[cfg(target_os = "macos")]
pub fn lang_version() -> (u8, u8) {
    use objc2::available;
    if available!(macos = 26.0) {
        (4, 0)
    } else if available!(macos = 15.0) {
        (3, 2)
    } else if available!(macos = 14.0) {
        (3, 1)
    } else if available!(macos = 13.0) {
        (3, 0)
    } else if available!(macos = 12.0) {
        (2, 4)
    } else if available!(macos = 11.0) {
        (2, 3)
    } else {
        (2, 2)
    }
}

/// Off macOS only the translation test runs: the newest version.
#[cfg(not(target_os = "macos"))]
pub fn lang_version() -> (u8, u8) {
    (4, 0)
}

/// The writer options wgpu-hal passes for one entry point. `wgpu::Device::create_shader_module`
/// asks for every runtime check (`ShaderRuntimeChecks::checked`), so bounds checks restrict.
pub fn msl_options(lang_version: (u8, u8), entry: &str, resources: msl::EntryPointResources) -> msl::Options {
    let checked = naga::proc::BoundsCheckPolicy::Restrict;
    msl::Options {
        lang_version,
        inline_samplers: Default::default(),
        spirv_cross_compatibility: false,
        fake_missing_bindings: false,
        per_entry_point_map: msl::EntryPointResourceMap::from([(entry.to_owned(), resources)]),
        bounds_check_policies: naga::proc::BoundsCheckPolicies { index: checked, buffer: checked, image_load: checked, binding_array: naga::proc::BoundsCheckPolicy::Unchecked },
        // wgpu's PipelineCompilationOptions default.
        zero_initialize_workgroup_memory: true,
        force_loop_bounding: true,
        // Only a task shader reads it; none is compiled here.
        task_dispatch_limits: None,
        mesh_shader_primitive_indices_clamp: true,
        emit_int_div_checks: true,
        ray_query_initialization_tracking: true,
    }
}

/// The pipeline options for one entry point: vertex pulling on, as wgpu-hal sets it, over
/// `vertex_buffers` (none for a fragment stage). Every pipeline here draws triangles.
pub fn pipeline_options(stage: ShaderStage, entry: &str, vertex_buffers: Vec<msl::VertexBufferMapping>) -> msl::PipelineOptions {
    msl::PipelineOptions {
        entry_point: Some((stage, entry.to_owned())),
        allow_and_force_point_size: false,
        vertex_pulling_transform: true,
        vertex_buffer_mappings: vertex_buffers,
        binding_array_length_map: Default::default(),
    }
}

/// A WGSL source parsed and validated as wgpu-core does it.
pub fn parse_wgsl(src: &str) -> Result<(naga::Module, naga::valid::ModuleInfo), String> {
    let module = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module).map_err(|e| format!("{e:?}"))?;
    Ok((module, info))
}

/// One entry point of `module` as MSL, with the slots read off the module and no vertex buffers.
pub fn to_msl(module: &naga::Module, info: &naga::valid::ModuleInfo, stage: ShaderStage, entry: &str) -> Result<String, String> {
    let resources = module_resources(module, info, stage, entry)?;
    to_msl_with(module, info, stage, entry, resources, Vec::new()).map(|(src, _)| src)
}

/// One entry point as MSL with the given slots and vertex buffers, and the name the entry point
/// has in the MSL.
pub fn to_msl_with(module: &naga::Module, info: &naga::valid::ModuleInfo, stage: ShaderStage, entry: &str, resources: msl::EntryPointResources, vertex_buffers: Vec<msl::VertexBufferMapping>) -> Result<(String, String), String> {
    let (module, info) = naga::back::pipeline_constants::process_overrides(module, info, Some((stage, entry)), &Default::default()).map_err(|e| format!("MSL: {e}"))?;
    let options = msl_options(lang_version(), entry, resources);
    let (src, translated) = msl::write_string(&module, &info, &options, &pipeline_options(stage, entry, vertex_buffers)).map_err(|e| format!("MSL: {e}"))?;
    let name = translated.entry_point_names.first().cloned().ok_or("MSL: no entry point")?.map_err(|e| format!("MSL: {e}"))?;
    Ok((src, name))
}

/// The effect passes' bind group 0 (`render_fx_gpu::Chain::new`): the input texture, the uniform
/// block and the sampler, all seen by the fragment stage only.
pub const EFFECT: [Entry; 3] = [Entry::new(0, Kind::Texture, false, true), Entry::new(1, Kind::Buffer, false, true), Entry::new(2, Kind::Sampler, false, true)];

/// The copy passes' bind group 0: one texture read by the fragment stage.
pub const COPY_INPUT: [Entry; 1] = [Entry::new(0, Kind::Texture, false, true)];

/// An effect's fragment shader as MSL over the effect layout, as wgpu-hal translates it for the
/// chain's pipeline, and the entry point's MSL name.
pub fn effect_to_msl(compiled: &crate::render_fx::Compiled) -> Result<(String, String), String> {
    to_msl_with(&compiled.module, &compiled.info, ShaderStage::Fragment, &compiled.entry, stage_resources(&[&EFFECT], ShaderStage::Fragment), Vec::new())
}

/// The full-screen triangle every effect and copy pass draws (`render_fx_gpu::VERTEX`), as MSL.
pub fn effect_vertex_msl() -> Result<(String, String), String> {
    let (module, info) = parse_wgsl(crate::render_fx_gpu::VERTEX)?;
    to_msl_with(&module, &info, ShaderStage::Vertex, "vs_main", stage_resources(&[&EFFECT], ShaderStage::Vertex), Vec::new())
}

/// The fill copy (`render_fx_gpu::COPY`) as MSL over its one-texture layout.
pub fn copy_msl() -> Result<(String, String), String> {
    let (module, info) = parse_wgsl(crate::render_fx_gpu::COPY)?;
    to_msl_with(&module, &info, ShaderStage::Fragment, "fs_main", stage_resources(&[&COPY_INPUT], ShaderStage::Fragment), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_and_image_wgsl_translate_to_msl() {
        for (name, src) in [("quad", include_str!("../quad.wgsl")), ("image", include_str!("../image.wgsl"))] {
            let (module, info) = parse_wgsl(src).unwrap_or_else(|e| panic!("{name}: {e}"));
            for (stage, entry) in [(ShaderStage::Vertex, "vs"), (ShaderStage::Fragment, "fs")] {
                let msl = to_msl(&module, &info, stage, entry).unwrap_or_else(|e| panic!("{name} {entry}: {e}"));
                assert!(msl.contains("#include <metal_stdlib>"), "{name} {entry}:\n{msl}");
            }
        }
    }

    /// The slots wgpu-hal hands out for the image pipeline's layout.
    #[test]
    fn slots_count_per_stage_and_kind() {
        let screen = [Entry::new(0, Kind::Buffer, true, false)];
        let picture = [Entry::new(0, Kind::Texture, false, true), Entry::new(1, Kind::Sampler, false, true)];
        let vs = stage_resources(&[&screen, &picture], ShaderStage::Vertex);
        assert_eq!(vs.resources[&naga::ResourceBinding { group: 0, binding: 0 }].buffer, Some(0));
        assert_eq!((vs.resources.len(), vs.sizes_buffer), (1, Some(1)));
        let fs = stage_resources(&[&screen, &picture], ShaderStage::Fragment);
        assert_eq!(fs.resources[&naga::ResourceBinding { group: 1, binding: 0 }].texture, Some(0));
        assert!(matches!(fs.resources[&naga::ResourceBinding { group: 1, binding: 1 }].sampler, Some(msl::BindSamplerTarget::Resource(0))));
        assert_eq!(fs.sizes_buffer, None);
        assert_eq!(vertex_buffer_slot(0), 30);
    }

    /// Every built-in effect, with every combination of the shared switches, reaches MSL over the
    /// chain's layout, as do the triangle and the copy the passes share.
    #[test]
    fn every_plugin_translates_to_msl() {
        let consts: Vec<&str> = crate::render_fx::SWITCHES.iter().filter_map(|(_, c, ..)| *c).collect();
        let plugins: Vec<_> = crate::render_fx::discover(None).0.into_iter().filter(|p| p.builtin()).collect();
        assert_eq!(plugins.len(), 13, "the shipped plugins");
        for (name, msl) in [("vertex", effect_vertex_msl()), ("copy", copy_msl())] {
            let (src, _) = msl.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(src.contains("#include <metal_stdlib>"), "{name}");
        }
        let combos = 1u32 << consts.len();
        let failures: Vec<String> = std::thread::scope(|scope| {
            let runs: Vec<_> = plugins
                .iter()
                .map(|p| {
                    let consts = &consts;
                    scope.spawn(move || {
                        let mut bad = Vec::new();
                        for bits in 0..combos {
                            let switches: Vec<(&str, bool)> = consts.iter().enumerate().map(|(i, c)| (*c, bits & (1 << i) != 0)).collect();
                            let compiled = match p.compile_with(&switches) {
                                Ok(c) => c,
                                Err(e) => {
                                    bad.push(format!("{} {switches:?}: {e}", p.id));
                                    continue;
                                }
                            };
                            for c in &compiled {
                                match effect_to_msl(c) {
                                    Ok((src, entry)) if src.contains(&entry) => {}
                                    Ok((_, entry)) => bad.push(format!("{} {switches:?}: no {entry} in the MSL", p.id)),
                                    Err(e) => bad.push(format!("{} {switches:?}: {e}", p.id)),
                                }
                            }
                        }
                        bad
                    })
                })
                .collect();
            runs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    }
}

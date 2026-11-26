//! Rectangles: the quad pipeline (`src/quad.wgsl`) and its fade passes, plus the pipeline building
//! the image pipeline shares. Each pipeline is set up as wgpu-hal sets up the same wgpu one.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLCompileOptions, MTLDevice, MTLFunction, MTLLanguageVersion, MTLLibrary, MTLPixelFormat, MTLPrimitiveTopologyClass, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLVertexDescriptor, MTLVertexFormat,
    MTLVertexStepFunction,
};

use super::shaders::{self, Entry, Kind};
use crate::draw_list::QUAD_STRIDE;

/// One vertex attribute: shader location, byte offset and format.
pub(super) struct Attr {
    pub location: u32,
    pub offset: u32,
    pub format: wgpu::VertexFormat,
}

/// One vertex buffer: its stride, whether it steps per instance, and its attributes. Buffer `i`
/// of a pipeline sits at `shaders::vertex_buffer_slot(i)`.
pub(super) struct Layout {
    pub stride: u32,
    pub instance: bool,
    pub attrs: &'static [Attr],
}

/// A WGSL file's vertex and fragment entry points as Metal functions, translated with wgpu's
/// options over a pipeline layout of `groups` and the vertex buffers `buffers`.
pub(super) struct Shader {
    pub vs: Retained<ProtocolObject<dyn MTLFunction>>,
    pub fs: Retained<ProtocolObject<dyn MTLFunction>>,
    buffers: &'static [Layout],
    /// The vertex stage's slots, as the translation used them.
    vertex: naga::back::msl::EntryPointResources,
    /// The fragment stage's slots, as the translation used them.
    fragment: naga::back::msl::EntryPointResources,
}

/// The Metal vertex format wgpu-hal maps `format` to (`conv::map_vertex_format`), for the ones
/// these pipelines use.
fn vertex_format(format: wgpu::VertexFormat) -> MTLVertexFormat {
    use wgpu::VertexFormat as F;
    match format {
        F::Float32 => MTLVertexFormat::Float,
        F::Float32x2 => MTLVertexFormat::Float2,
        F::Float32x4 => MTLVertexFormat::Float4,
        F::Uint32 => MTLVertexFormat::UInt,
        F::Sint32x2 => MTLVertexFormat::Int2,
        other => unreachable!("no pipeline here uses {other:?}"),
    }
}

/// The Metal library for `src` (MSL), compiled with the options wgpu-hal gives `load_shader`.
fn library(device: &ProtocolObject<dyn MTLDevice>, src: &str) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>, String> {
    let options = MTLCompileOptions::new();
    let (major, minor) = shaders::lang_version();
    options.setLanguageVersion(MTLLanguageVersion(((major as usize) << 16) + minor as usize));
    if objc2::available!(macos = 11.0) {
        options.setPreserveInvariance(true);
    }
    device.newLibraryWithSource_options_error(&NSString::from_str(src), Some(&options)).map_err(|e| format!("Metal: {}", e.localizedDescription()))
}

pub(super) fn function(device: &ProtocolObject<dyn MTLDevice>, src: &str, name: &str) -> Result<Retained<ProtocolObject<dyn MTLFunction>>, String> {
    library(device, src)?.newFunctionWithName(&NSString::from_str(name)).ok_or_else(|| format!("Metal: no function {name}"))
}

impl Shader {
    /// `wgsl`'s entry points `vs` and `fs`.
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, wgsl: &str, (vs, fs): (&str, &str), groups: &[&[Entry]], buffers: &'static [Layout]) -> Result<Shader, String> {
        use naga::back::msl::{AttributeMapping, VertexBufferMapping, VertexBufferStepMode};
        use naga::ShaderStage;
        let (module, info) = shaders::parse_wgsl(wgsl)?;
        let mappings = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| VertexBufferMapping {
                id: shaders::vertex_buffer_slot(i as u32),
                stride: b.stride,
                step_mode: if b.instance { VertexBufferStepMode::ByInstance } else { VertexBufferStepMode::ByVertex },
                attributes: b.attrs.iter().map(|a| AttributeMapping { shader_location: a.location, offset: a.offset, format: a.format }).collect(),
            })
            .collect();
        let vertex = shaders::stage_resources(groups, ShaderStage::Vertex);
        let (vs_src, vs_name) = shaders::to_msl_with(&module, &info, ShaderStage::Vertex, vs, vertex.clone(), mappings)?;
        let fragment = shaders::stage_resources(groups, ShaderStage::Fragment);
        let (fs_src, fs_name) = shaders::to_msl_with(&module, &info, ShaderStage::Fragment, fs, fragment.clone(), Vec::new())?;
        Ok(Shader { vs: function(device, &vs_src, &vs_name)?, fs: function(device, &fs_src, &fs_name)?, buffers, vertex, fragment })
    }

    /// The fragment stage's slot for `group`, `binding`: a buffer, a texture or a sampler.
    pub fn fragment_slot(&self, group: u32, binding: u32) -> Result<usize, String> {
        let t = self.fragment.resources.get(&naga::ResourceBinding { group, binding });
        let sampler = t.and_then(|t| match t.sampler {
            Some(naga::back::msl::BindSamplerTarget::Resource(s)) => Some(s),
            _ => None,
        });
        t.and_then(|t| t.buffer.or(t.texture)).or(sampler).map(usize::from).ok_or_else(|| format!("no fragment slot for group {group} binding {binding}"))
    }

    /// The vertex stage's buffer slot for `group`, `binding`.
    pub fn vertex_buffer(&self, group: u32, binding: u32) -> Result<usize, String> {
        let at = self.vertex.resources.get(&naga::ResourceBinding { group, binding }).and_then(|t| t.buffer);
        at.map(usize::from).ok_or_else(|| format!("no vertex buffer slot for group {group} binding {binding}"))
    }

    /// The vertex stage's sizes buffer slot (vertex pulling reads the bound lengths there).
    pub fn sizes_slot(&self) -> usize {
        usize::from(self.vertex.sizes_buffer.expect("a vertex stage always has a sizes buffer"))
    }

    /// A pipeline writing `format`, blending as `blend` says (source and destination factors for
    /// color and alpha, all added), every channel written.
    pub fn pipeline(&self, device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat, blend: Blend) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, String> {
        let d = MTLRenderPipelineDescriptor::new();
        d.setVertexFunction(Some(&self.vs));
        d.setFragmentFunction(Some(&self.fs));
        // The vertex layout too, as wgpu-hal sets it beside the pulled vertices.
        let vd = MTLVertexDescriptor::new();
        // SAFETY: the slots and locations are inside Metal's tables (31 buffers, 31 attributes).
        unsafe {
            for (i, b) in self.buffers.iter().enumerate() {
                let slot = shaders::vertex_buffer_slot(i as u32) as usize;
                let layout = vd.layouts().objectAtIndexedSubscript(slot);
                layout.setStride(b.stride as usize);
                layout.setStepFunction(if b.instance { MTLVertexStepFunction::PerInstance } else { MTLVertexStepFunction::PerVertex });
                for a in b.attrs {
                    let at = vd.attributes().objectAtIndexedSubscript(a.location as usize);
                    at.setFormat(vertex_format(a.format));
                    at.setBufferIndex(slot);
                    at.setOffset(a.offset as usize);
                }
            }
            // Triangle lists and strips are both of the triangle class.
            d.setInputPrimitiveTopology(MTLPrimitiveTopologyClass::Triangle);
        }
        d.setVertexDescriptor(Some(&vd));
        // SAFETY: index 0 is the one color target.
        let at = unsafe { d.colorAttachments().objectAtIndexedSubscript(0) };
        at.setPixelFormat(format);
        at.setWriteMask(MTLColorWriteMask::All);
        at.setBlendingEnabled(true);
        at.setRgbBlendOperation(MTLBlendOperation::Add);
        at.setSourceRGBBlendFactor(blend.color.0);
        at.setDestinationRGBBlendFactor(blend.color.1);
        at.setAlphaBlendOperation(MTLBlendOperation::Add);
        at.setSourceAlphaBlendFactor(blend.alpha.0);
        at.setDestinationAlphaBlendFactor(blend.alpha.1);
        device.newRenderPipelineStateWithDescriptor_error(&d).map_err(|e| format!("Metal pipeline: {}", e.localizedDescription()))
    }
}

/// Blend factors (source, destination) for color and for alpha.
#[derive(Clone, Copy)]
pub(super) struct Blend {
    color: (MTLBlendFactor, MTLBlendFactor),
    alpha: (MTLBlendFactor, MTLBlendFactor),
}

impl Blend {
    /// wgpu's `BlendState::ALPHA_BLENDING`.
    pub const ALPHA: Blend = Blend { color: (MTLBlendFactor::SourceAlpha, MTLBlendFactor::OneMinusSourceAlpha), alpha: (MTLBlendFactor::One, MTLBlendFactor::OneMinusSourceAlpha) };
    /// The fade: destination times the blend color, in every channel.
    pub const SCALE: Blend = Blend { color: (MTLBlendFactor::Zero, MTLBlendFactor::BlendColor), alpha: (MTLBlendFactor::Zero, MTLBlendFactor::BlendColor) };
    /// egui-wgpu's: premultiplied color, and alpha that only grows where the target had little.
    pub const EGUI: Blend = Blend { color: (MTLBlendFactor::One, MTLBlendFactor::OneMinusSourceAlpha), alpha: (MTLBlendFactor::OneMinusDestinationAlpha, MTLBlendFactor::One) };
}

/// The quad and image shaders' screen uniform, at group 0 binding 0, seen by the vertex stage.
pub(super) const SCREEN: [Entry; 1] = [Entry::new(0, Kind::Buffer, true, false)];

const QUAD_ATTRS: [Attr; 2] = [Attr { location: 0, offset: 0, format: wgpu::VertexFormat::Float32x2 }, Attr { location: 1, offset: 8, format: wgpu::VertexFormat::Float32x4 }];
const QUAD_BUFFERS: [Layout; 1] = [Layout { stride: QUAD_STRIDE as u32, instance: false, attrs: &QUAD_ATTRS }];

/// The vertex stage slots of the screen uniform and of the sizes buffer, as the translation of a
/// quad or image shader gave them.
#[derive(Clone, Copy)]
pub(super) struct ScreenSlots {
    pub screen: usize,
    pub sizes: usize,
}

impl ScreenSlots {
    pub fn of(shader: &Shader) -> Result<ScreenSlots, String> {
        Ok(ScreenSlots { screen: shader.vertex_buffer(0, 0)?, sizes: shader.sizes_slot() })
    }
}

/// Rectangles: two triangles each, position and color per vertex.
pub(super) struct QuadPipeline {
    shader: Shader,
    pub pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Also the fade pipelines' (the same shader).
    pub slots: ScreenSlots,
    /// The fade pass on a non-sRGB view of the target, made on first use.
    fader: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
    /// The fade pass on the target's own sRGB format (no second view), made on first use.
    fader_view: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
    format: MTLPixelFormat,
}

impl QuadPipeline {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat) -> Result<QuadPipeline, String> {
        let shader = Shader::new(device, include_str!("../quad.wgsl"), ("vs", "fs"), &[&SCREEN], &QUAD_BUFFERS)?;
        let pipeline = shader.pipeline(device, format, Blend::ALPHA)?;
        Ok(QuadPipeline { slots: ScreenSlots::of(&shader)?, shader, pipeline, fader: None, fader_view: None, format })
    }

    /// The fade pipeline for a pass on a raw (non-sRGB) view when `raw`, else on the target's view.
    pub fn fader(&mut self, device: &ProtocolObject<dyn MTLDevice>, raw: bool) -> Result<&ProtocolObject<dyn MTLRenderPipelineState>, String> {
        let (slot, format) = if raw { (&mut self.fader, super::draw::raw_format(self.format)) } else { (&mut self.fader_view, self.format) };
        if slot.is_none() {
            *slot = Some(self.shader.pipeline(device, format, Blend::SCALE)?);
        }
        Ok(slot.as_deref().expect("made above"))
    }
}

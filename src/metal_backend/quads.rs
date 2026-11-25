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

/// One vertex attribute: shader location, byte offset, and whether it is two floats or four.
pub(super) struct Attr {
    pub location: u32,
    pub offset: u32,
    pub floats: u32,
}

/// A WGSL file's `vs` and `fs` as Metal functions, translated with wgpu's options over a pipeline
/// layout of `groups` and one vertex buffer of `stride` bytes.
pub(super) struct Shader {
    pub vs: Retained<ProtocolObject<dyn MTLFunction>>,
    pub fs: Retained<ProtocolObject<dyn MTLFunction>>,
    stride: u32,
    attrs: &'static [Attr],
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

fn function(device: &ProtocolObject<dyn MTLDevice>, src: &str, name: &str) -> Result<Retained<ProtocolObject<dyn MTLFunction>>, String> {
    library(device, src)?.newFunctionWithName(&NSString::from_str(name)).ok_or_else(|| format!("Metal: no function {name}"))
}

impl Shader {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, wgsl: &str, groups: &[&[Entry]], stride: u32, attrs: &'static [Attr]) -> Result<Shader, String> {
        use naga::ShaderStage;
        let (module, info) = shaders::parse_wgsl(wgsl)?;
        let attributes = attrs
            .iter()
            .map(|a| naga::back::msl::AttributeMapping { shader_location: a.location, offset: a.offset, format: if a.floats == 2 { wgpu::VertexFormat::Float32x2 } else { wgpu::VertexFormat::Float32x4 } })
            .collect();
        let mapping = naga::back::msl::VertexBufferMapping { id: shaders::vertex_buffer_slot(0), stride, step_mode: naga::back::msl::VertexBufferStepMode::ByVertex, attributes };
        let (vs_src, vs_name) = shaders::to_msl_with(&module, &info, ShaderStage::Vertex, "vs", shaders::stage_resources(groups, ShaderStage::Vertex), vec![mapping])?;
        let (fs_src, fs_name) = shaders::to_msl_with(&module, &info, ShaderStage::Fragment, "fs", shaders::stage_resources(groups, ShaderStage::Fragment), Vec::new())?;
        Ok(Shader { vs: function(device, &vs_src, &vs_name)?, fs: function(device, &fs_src, &fs_name)?, stride, attrs })
    }

    /// A pipeline writing `format`, blending as `blend` says (source and destination factors for
    /// color and alpha, all added), every channel written.
    pub fn pipeline(&self, device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat, blend: Blend) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, String> {
        let d = MTLRenderPipelineDescriptor::new();
        d.setVertexFunction(Some(&self.vs));
        d.setFragmentFunction(Some(&self.fs));
        // The vertex layout too, as wgpu-hal sets it beside the pulled vertices.
        let vd = MTLVertexDescriptor::new();
        let slot = shaders::vertex_buffer_slot(0) as usize;
        // SAFETY: the slot and locations are inside Metal's tables (31 buffers, 31 attributes).
        unsafe {
            let layout = vd.layouts().objectAtIndexedSubscript(slot);
            layout.setStride(self.stride as usize);
            layout.setStepFunction(MTLVertexStepFunction::PerVertex);
            for a in self.attrs {
                let at = vd.attributes().objectAtIndexedSubscript(a.location as usize);
                at.setFormat(if a.floats == 2 { MTLVertexFormat::Float2 } else { MTLVertexFormat::Float4 });
                at.setBufferIndex(slot);
                at.setOffset(a.offset as usize);
            }
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
}

/// The quad and image shaders' screen uniform, at group 0 binding 0, seen by the vertex stage.
pub(super) const SCREEN: [Entry; 1] = [Entry::new(0, Kind::Buffer, true, false)];

const QUAD_ATTRS: [Attr; 2] = [Attr { location: 0, offset: 0, floats: 2 }, Attr { location: 1, offset: 8, floats: 4 }];

/// Rectangles: two triangles each, position and color per vertex.
pub(super) struct QuadPipeline {
    shader: Shader,
    pub pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// The fade pass on a non-sRGB view of the target, made on first use.
    fader: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
    /// The fade pass on the target's own sRGB format (no second view), made on first use.
    fader_view: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
    format: MTLPixelFormat,
}

impl QuadPipeline {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat) -> Result<QuadPipeline, String> {
        let shader = Shader::new(device, include_str!("../quad.wgsl"), &[&SCREEN], QUAD_STRIDE as u32, &QUAD_ATTRS)?;
        let pipeline = shader.pipeline(device, format, Blend::ALPHA)?;
        Ok(QuadPipeline { shader, pipeline, fader: None, fader_view: None, format })
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

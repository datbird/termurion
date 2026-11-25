use crate::{
    custom_glyph::CustomGlyphCacheKey, AtlasOp, ColorMode, ContentType, FontSystem, GlyphDetails,
    GlyphToRender, GpuCacheStatus, PrepareError, RasterizeCustomGlyphRequest,
    RasterizedCustomGlyph, Resolution, SwashCache, SwashContent, TextArea, TextAtlas, TextBounds,
};
use cosmic_text::{Color, SubpixelBin};

/// Termurion patch 9: the glyph instances `TextRenderer::prepare_cpu` makes, in draw order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphDraws {
    pub instances: Vec<GlyphToRender>,
}

impl GlyphDraws {
    /// The instances as the bytes a GPU buffer holds.
    pub fn bytes(&self) -> &[u8] {
        crate::glyph_bytes(&self.instances)
    }
}

/// A text renderer that turns text areas into glyph instances over cached atlas glyphs.
///
/// Termurion patch 9: the CPU half; a backend uploads the instances and draws them.
#[derive(Debug, Default)]
pub struct TextRenderer {
    _cpu: (),
}

impl TextRenderer {
    /// Creates a new `TextRenderer`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Termurion patch 9: prepares the text areas into `out` (its old instances are replaced),
    /// rasterizing into `atlas` what it lacks. `resolution` is the target's size, which clips the
    /// bounds. The atlas changes wait in `atlas` for `TextAtlas::take_ops`.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_cpu<'a>(
        &mut self,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        resolution: Resolution,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
        mut rasterize_custom_glyph: impl FnMut(
            RasterizeCustomGlyphRequest,
        ) -> Option<RasterizedCustomGlyph>,
        out: &mut GlyphDraws,
    ) -> Result<(), PrepareError> {
        let glyph_vertices = &mut out.instances;
        glyph_vertices.clear();

        let mut system = GlyphSystem {
            atlas,
            cache,
            font_system,
        };

        for text_area in text_areas {
            let bounds = GlyphBounds::clamped(text_area.bounds, resolution);

            for glyph in text_area.custom_glyphs.iter() {
                let x = text_area.left + (glyph.left * text_area.scale);
                let y = text_area.top + (glyph.top * text_area.scale);
                let width = (glyph.width * text_area.scale).round() as u16;
                let height = (glyph.height * text_area.scale).round() as u16;

                let (x, y, x_bin, y_bin) = if glyph.snap_to_physical_pixel {
                    (
                        x.round() as i32,
                        y.round() as i32,
                        SubpixelBin::Zero,
                        SubpixelBin::Zero,
                    )
                } else {
                    let (x, x_bin) = SubpixelBin::new(x);
                    let (y, y_bin) = SubpixelBin::new(y);
                    (x, y, x_bin, y_bin)
                };

                let cache_key = GlyphonCacheKey::Custom(CustomGlyphCacheKey {
                    glyph_id: glyph.id,
                    width,
                    height,
                    x_bin,
                    y_bin,
                });

                let color = glyph.color.unwrap_or(text_area.default_color);

                if let Some(glyph_to_render) = prepare_glyph(
                    &mut system,
                    GlyphMetadata {
                        x,
                        y,
                        line_y: 0.0,
                        scale_factor: text_area.scale,
                        color,
                        cache_key,
                    },
                    bounds,
                    |_system, rasterize_custom_glyph| -> Option<GetGlyphImageResult> {
                        if width == 0 || height == 0 {
                            return None;
                        }

                        let input = RasterizeCustomGlyphRequest {
                            id: glyph.id,
                            width,
                            height,
                            x_bin,
                            y_bin,
                            scale: text_area.scale,
                        };

                        let output = (rasterize_custom_glyph)(input)?;

                        output.validate(&input, None);

                        Some(GetGlyphImageResult {
                            content_type: output.content_type,
                            top: 0,
                            left: 0,
                            width,
                            height,
                            data: output.data,
                        })
                    },
                    &mut rasterize_custom_glyph,
                )? {
                    glyph_vertices.push(glyph_to_render);
                }
            }

            let is_run_visible = |run: &cosmic_text::LayoutRun| {
                let start_y_physical = (text_area.top + (run.line_top * text_area.scale)) as i32;
                let end_y_physical = start_y_physical + (run.line_height * text_area.scale) as i32;

                start_y_physical <= text_area.bounds.bottom
                    && text_area.bounds.top <= end_y_physical
            };

            let layout_runs = text_area
                .buffer
                .layout_runs()
                .skip_while(|run| !is_run_visible(run))
                .take_while(is_run_visible);

            for run in layout_runs {
                for glyph in run.glyphs.iter() {
                    let physical_glyph =
                        glyph.physical((text_area.left, text_area.top), text_area.scale);

                    let color = match glyph.color_opt {
                        Some(some) => some,
                        None => text_area.default_color,
                    };

                    if let Some(glyph_to_render) = prepare_glyph(
                        &mut system,
                        GlyphMetadata {
                            x: physical_glyph.x,
                            y: physical_glyph.y,
                            line_y: run.line_y,
                            color,
                            cache_key: GlyphonCacheKey::Text(physical_glyph.cache_key),
                            scale_factor: text_area.scale,
                        },
                        bounds,
                        |system, _rasterize_custom_glyph| {
                            text_glyph_image(system, physical_glyph.cache_key)
                        },
                        &mut rasterize_custom_glyph,
                    )? {
                        glyph_vertices.push(glyph_to_render);
                    }
                }
            }
        }

        Ok(())
    }
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TextColorConversion {
    None = 0,
    ConvertToLinear = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GlyphonCacheKey {
    Text(cosmic_text::CacheKey),
    Custom(CustomGlyphCacheKey),
}

pub(crate) struct GetGlyphImageResult {
    content_type: ContentType,
    top: i16,
    left: i16,
    width: u16,
    height: u16,
    data: Vec<u8>,
}

pub(crate) struct GlyphMetadata {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) line_y: f32,
    pub(crate) scale_factor: f32,
    pub(crate) color: Color,
    pub(crate) cache_key: GlyphonCacheKey,
}

#[derive(Clone, Copy)]
pub(crate) struct Bounds {
    pub(crate) min: i32,
    pub(crate) max: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct GlyphBounds {
    pub(crate) x: Bounds,
    pub(crate) y: Bounds,
}

impl GlyphBounds {
    /// The bounds clamped to the viewport (shared with `RowRenderer`'s clip rectangles).
    pub(crate) fn clamped(bounds: TextBounds, resolution: Resolution) -> Self {
        let x_min = bounds.left.max(0);
        let y_min = bounds.top.max(0);
        Self {
            x: Bounds {
                min: x_min,
                max: bounds.right.min(resolution.width as i32).max(x_min),
            },
            y: Bounds {
                min: y_min,
                max: bounds.bottom.min(resolution.height as i32).max(y_min),
            },
        }
    }

    /// Termurion patch 6: clips nothing (`RowRenderer` clips in its vertex shader).
    pub(crate) const UNBOUNDED: Self = Self {
        x: Bounds { min: i32::MIN, max: i32::MAX },
        y: Bounds { min: i32::MIN, max: i32::MAX },
    };
}

pub(crate) struct GlyphSystem<'a> {
    pub(crate) atlas: &'a mut TextAtlas,
    pub(crate) cache: &'a mut SwashCache,
    pub(crate) font_system: &'a mut FontSystem,
}

/// Termurion patch 6: rasterizes a text glyph for its first upload (shared with `RowRenderer`).
pub(crate) fn text_glyph_image(
    system: &mut GlyphSystem,
    cache_key: cosmic_text::CacheKey,
) -> Option<GetGlyphImageResult> {
    let mut image = system
        .cache
        .get_image_uncached(system.font_system, cache_key)?;
    // Termurion patch 1: filter the mask of an aliased or thin-stroke glyph.
    if matches!(image.content, SwashContent::Mask) {
        crate::filter_text_mask(cache_key.flags.bits(), &mut image.data);
    }

    let content_type = match image.content {
        SwashContent::Color => ContentType::Color,
        SwashContent::Mask => ContentType::Mask,
        SwashContent::SubpixelMask => {
            // Not implemented yet, but don't panic if this happens.
            ContentType::Mask
        }
    };

    Some(GetGlyphImageResult {
        content_type,
        top: image.placement.top as i16,
        left: image.placement.left as i16,
        width: image.placement.width as u16,
        height: image.placement.height as u16,
        data: image.data,
    })
}

pub(crate) fn prepare_glyph<R>(
    system: &mut GlyphSystem,
    metadata: GlyphMetadata,
    bounds: GlyphBounds,
    get_glyph_image: impl FnOnce(&mut GlyphSystem, &mut R) -> Option<GetGlyphImageResult>,
    mut rasterize_custom_glyph: R,
) -> Result<Option<GlyphToRender>, PrepareError>
where
    R: FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
{
    let details =
        if let Some(details) = system.atlas.mask_atlas.glyph_cache.get(&metadata.cache_key) {
            system
                .atlas
                .mask_atlas
                .glyphs_in_use
                .insert(metadata.cache_key);
            details
        } else if let Some(details) = system
            .atlas
            .color_atlas
            .glyph_cache
            .get(&metadata.cache_key)
        {
            system
                .atlas
                .color_atlas
                .glyphs_in_use
                .insert(metadata.cache_key);
            details
        } else {
            let Some(image) = (get_glyph_image)(system, &mut rasterize_custom_glyph) else {
                return Ok(None);
            };

            let should_rasterize = image.width > 0 && image.height > 0;

            let (gpu_cache, atlas_id, inner) = if should_rasterize {
                // Find a position in the packer
                let allocation = loop {
                    let inner = system.atlas.inner_for_content_mut(image.content_type);
                    match inner.try_allocate(image.width as usize, image.height as usize) {
                        Some(a) => break a,
                        None => {
                            if !system.atlas.grow(
                                system.font_system,
                                system.cache,
                                image.content_type,
                                metadata.scale_factor,
                                &mut rasterize_custom_glyph,
                            ) {
                                return Err(PrepareError::AtlasFull);
                            }
                        }
                    }
                };
                let atlas_min = allocation.rectangle.min;

                // Termurion patch 9: the texture write is an op the backend applies.
                system.atlas.ops.push(AtlasOp::Upload {
                    kind: image.content_type,
                    x: atlas_min.x as u32,
                    y: atlas_min.y as u32,
                    w: image.width as u32,
                    h: image.height as u32,
                    bytes: image.data,
                });

                (
                    GpuCacheStatus::InAtlas {
                        x: atlas_min.x as u16,
                        y: atlas_min.y as u16,
                        content_type: image.content_type,
                    },
                    Some(allocation.id),
                    system.atlas.inner_for_content_mut(image.content_type),
                )
            } else {
                let inner = &mut system.atlas.color_atlas;
                (GpuCacheStatus::SkipRasterization, None, inner)
            };

            inner.glyphs_in_use.insert(metadata.cache_key);
            // Insert the glyph into the cache and return the details reference
            inner
                .glyph_cache
                .get_or_insert(metadata.cache_key, || GlyphDetails {
                    width: image.width,
                    height: image.height,
                    gpu_cache,
                    atlas_id,
                    top: image.top,
                    left: image.left,
                })
        };

    let mut x = metadata.x + details.left as i32;
    let mut y =
        (metadata.line_y * metadata.scale_factor).round() as i32 + metadata.y - details.top as i32;

    let (mut atlas_x, mut atlas_y, content_type) = match details.gpu_cache {
        GpuCacheStatus::InAtlas { x, y, content_type } => (x, y, content_type),
        GpuCacheStatus::SkipRasterization => return Ok(None),
    };

    let mut width = details.width as i32;
    let mut height = details.height as i32;

    // Starts beyond right edge or ends beyond left edge
    let max_x = x + width;
    if x > bounds.x.max || max_x < bounds.x.min {
        return Ok(None);
    }

    // Starts beyond bottom edge or ends beyond top edge
    let max_y = y + height;
    if y > bounds.y.max || max_y < bounds.y.min {
        return Ok(None);
    }

    // Clip left ege
    if x < bounds.x.min {
        let right_shift = bounds.x.min - x;

        x = bounds.x.min;
        width = max_x - bounds.x.min;
        atlas_x += right_shift as u16;
    }

    // Clip right edge
    if x + width > bounds.x.max {
        width = bounds.x.max - x;
    }

    // Clip top edge
    if y < bounds.y.min {
        let bottom_shift = bounds.y.min - y;

        y = bounds.y.min;
        height = max_y - bounds.y.min;
        atlas_y += bottom_shift as u16;
    }

    // Clip bottom edge
    if y + height > bounds.y.max {
        height = bounds.y.max - y;
    }

    // Termurion patch 9: every caller drew at depth 0 (`zero_depth`), now the only depth.
    let depth = 0.0;

    Ok(Some(GlyphToRender {
        pos: [x, y],
        dim: [width as u16, height as u16],
        uv: [atlas_x, atlas_y],
        color: metadata.color.0,
        content_type_with_srgb: [
            content_type as u16,
            match system.atlas.color_mode {
                ColorMode::Accurate => TextColorConversion::ConvertToLinear,
                ColorMode::Web => TextColorConversion::None,
            } as u16,
        ],
        depth,
    }))
}

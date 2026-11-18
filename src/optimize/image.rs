//! Images: decodes PNG and JPEG, scales down one wider than the site allows, converts an
//! embedded color profile to sRGB, and encodes AVIF with libaom.

use std::error::Error;
use std::io::Cursor;
use std::slice;

use anyhow::{Result, anyhow};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use libavif_sys as avif;
use moxcms::{ColorProfile, DataColorSpace, Layout, TransformOptions};

pub(super) struct Encoded {
    pub(super) avif: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// The fixed encoder settings, which join the store key so a change invalidates old entries.
pub(super) const SETTINGS: &str = "avif speed 2, 4 threads, 4:2:0";
const AVIF_SPEED: i32 = 2; // speed 3 and up lose libaom's screen-content tools
const AVIF_THREADS: i32 = 4; // fixed, since libaom's output differs between 1 thread and several

/// Encodes a PNG or JPEG to AVIF at `quality`, from 0 to 100, at most `max_width` pixels
/// across.
pub(super) fn encode(source: &[u8], quality: u8, max_width: u32) -> Result<Encoded> {
    let rgba = decode(source, max_width).map_err(|e| anyhow!("decoding: {e}"))?;
    Ok(Encoded { avif: encode_avif(&rgba, quality)?, width: rgba.width(), height: rgba.height() })
}

/// What `max_width` adds to the store key: the scaling, for an image that it may shrink, and
/// nothing for any other, so that a new limit keeps the other encodes. Either side counts,
/// since EXIF orientation can turn the image.
pub(super) fn scaling(source: &[u8], max_width: u32) -> String {
    let reader = ImageReader::new(Cursor::new(source)).with_guessed_format().ok();
    let size = reader.and_then(|reader| reader.into_dimensions().ok());
    match size {
        Some((width, height)) if width.max(height) > max_width => {
            format!("lanczos3 to {max_width}")
        }
        _ => String::new(),
    }
}

/// Decodes to 8-bit RGBA, applies EXIF orientation, scales down to `max_width`, and converts
/// an RGB ICC profile to sRGB.
fn decode(bytes: &[u8], max_width: u32) -> Result<RgbaImage, Box<dyn Error>> {
    let mut decoder = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let icc = decoder.icc_profile()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    if image.width() > max_width {
        let height = u64::from(image.height()) * u64::from(max_width) / u64::from(image.width());
        // The height only shrinks, so it still fits.
        let height = u32::try_from(height).unwrap_or(u32::MAX).max(1);
        image = image.resize_exact(max_width, height, FilterType::Lanczos3);
    }
    let mut rgba = image.into_rgba8();
    if let Some(profile) = icc.and_then(|icc| ColorProfile::new_from_slice(&icc).ok())
        && profile.color_space == DataColorSpace::Rgb
    {
        let to_srgb = profile.create_transform_8bit(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgba,
            TransformOptions::default(),
        )?;
        let source = rgba.to_vec();
        to_srgb.transform(&source, &mut rgba)?;
    }
    Ok(rgba)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "libavif declares each color constant wider than the field it is for"
)]
fn encode_avif(rgba: &RgbaImage, quality: u8) -> Result<Vec<u8>> {
    let width = rgba.width();
    // SAFETY: the call takes no pointer. It returns null if it cannot allocate.
    let image =
        unsafe { avif::avifImageCreate(width, rgba.height(), 8, avif::AVIF_PIXEL_FORMAT_YUV420) };
    // SAFETY: as above.
    let encoder = unsafe { avif::avifEncoderCreate() };
    let mut out = avif::avifRWData::default();
    let mut result = avif::AVIF_RESULT_OUT_OF_MEMORY;
    // SAFETY: the pointer is null or names the image created above, which nothing else holds.
    if let Some(image) = unsafe { image.as_mut() }
        // SAFETY: the same holds for the encoder.
        && let Some(encoder) = unsafe { encoder.as_mut() }
    {
        image.yuvRange = avif::AVIF_RANGE_FULL;
        image.colorPrimaries = avif::AVIF_COLOR_PRIMARIES_BT709 as u16;
        image.transferCharacteristics = avif::AVIF_TRANSFER_CHARACTERISTICS_SRGB as u16;
        image.matrixCoefficients = avif::AVIF_MATRIX_COEFFICIENTS_BT601 as u16;
        let mut rgb = avif::avifRGBImage::default();
        // SAFETY: both point at live values, and the call only fills `rgb` from the image.
        unsafe { avif::avifRGBImageSetDefaults(&mut rgb, image) };
        rgb.pixels = rgba.as_ptr().cast_mut();
        rgb.rowBytes = width * 4;
        // An alpha plane is kept only when some pixel uses it.
        rgb.ignoreAlpha = rgba.pixels().all(|pixel| pixel.0[3] == 255).into();
        encoder.quality = quality.into();
        encoder.qualityAlpha = 100;
        encoder.speed = AVIF_SPEED;
        encoder.maxThreads = AVIF_THREADS;
        // SAFETY: `rgb.pixels` holds a row of `rowBytes` bytes for each row of the image, which
        // the call reads and does not write.
        result = unsafe { avif::avifImageRGBToYUV(image, &rgb) };
        if result == avif::AVIF_RESULT_OK {
            // SAFETY: all three point at live values that nothing else holds.
            result = unsafe { avif::avifEncoderWrite(encoder, image, &mut out) };
        }
    }
    let bytes = (result == avif::AVIF_RESULT_OK && !out.data.is_null()).then(|| {
        // SAFETY: a successful write leaves `size` bytes at `data`, which live until the free.
        unsafe { slice::from_raw_parts(out.data, out.size) }.to_vec()
    });
    // SAFETY: `out` is empty or as the write left it, and is freed once.
    unsafe { avif::avifRWDataFree(&mut out) };
    if !encoder.is_null() {
        // SAFETY: the encoder came from its create call, and is freed once, after its last use.
        unsafe { avif::avifEncoderDestroy(encoder) };
    }
    if !image.is_null() {
        // SAFETY: the same holds for the image.
        unsafe { avif::avifImageDestroy(image) };
    }
    bytes.ok_or_else(|| anyhow!("AVIF encoding failed with libavif result {result}"))
}

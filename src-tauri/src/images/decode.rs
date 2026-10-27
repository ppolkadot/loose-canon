//! Decoding untrusted image bytes with hard limits.
//!
//! Image bytes come from the internet, so every limit is checked before the work it guards:
//! total bytes before decoding, format by content (never by file name or `Content-Type`),
//! dimensions from the header before any pixel buffer is allocated, and an allocation cap during
//! decoding. Only JPEG, PNG and WebP are accepted.

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use super::phash::perceptual_hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLimits {
    pub max_bytes: usize,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            max_width: 10_000,
            max_height: 10_000,
            max_pixels: 40_000_000,
        }
    }
}

/// Formats we accept (and cache).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredFormat {
    Jpeg,
    Png,
    WebP,
}

impl StoredFormat {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedImage {
    pub width: u32,
    pub height: u32,
    pub phash: u64,
    pub format: StoredFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("the image is empty")]
    Empty,
    #[error("the image is larger than {limit} bytes")]
    TooLarge { limit: usize },
    #[error("only JPEG, PNG and WebP images are supported")]
    UnsupportedFormat,
    #[error("the image is {width}x{height} pixels, above the allowed size")]
    TooManyPixels { width: u32, height: u32 },
    #[error("the image is corrupt: {0}")]
    Corrupt(String),
}

impl ImageError {
    /// Stable name stored in `property_images.processing_error`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLarge { .. } => "too_large",
            Self::UnsupportedFormat => "unsupported_format",
            Self::TooManyPixels { .. } => "too_many_pixels",
            Self::Corrupt(_) => "corrupt",
        }
    }
}

/// How far from the end of a JPEG the end-of-image marker may sit (padding, small trailers).
const JPEG_TRAILER_WINDOW: usize = 4096;

fn has_jpeg_end_marker(bytes: &[u8]) -> bool {
    let tail = &bytes[bytes.len().saturating_sub(JPEG_TRAILER_WINDOW)..];
    tail.windows(2).any(|pair| pair == [0xFF, 0xD9])
}

fn corrupt(error: &image::ImageError) -> ImageError {
    // Keep it short: the message may be stored and shown.
    ImageError::Corrupt(error.to_string().chars().take(120).collect())
}

/// Validates, decodes and hashes `bytes` within `limits`.
pub fn hash_image(bytes: &[u8], limits: &ImageLimits) -> Result<HashedImage, ImageError> {
    if bytes.is_empty() {
        return Err(ImageError::Empty);
    }
    if bytes.len() > limits.max_bytes {
        return Err(ImageError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    let reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImageError::Corrupt(e.to_string().chars().take(120).collect()))?;
    let format = match reader.format() {
        Some(ImageFormat::Jpeg) => StoredFormat::Jpeg,
        Some(ImageFormat::Png) => StoredFormat::Png,
        Some(ImageFormat::WebP) => StoredFormat::WebP,
        _ => return Err(ImageError::UnsupportedFormat),
    };
    // The JPEG decoder is lenient: a cut-off file decodes "successfully" with the missing part
    // filled in, which would give a misleading hash. A complete JPEG carries an end-of-image
    // marker (FF D9) at its end, give or take a short trailer.
    if format == StoredFormat::Jpeg && !has_jpeg_end_marker(bytes) {
        return Err(ImageError::Corrupt(
            "the JPEG is truncated (no end-of-image marker)".to_owned(),
        ));
    }

    // The header alone gives the dimensions; nothing is allocated for pixels yet.
    let mut decoder = reader.into_decoder().map_err(|e| corrupt(&e))?;
    let (width, height) = decoder.dimensions();
    if width > limits.max_width
        || height > limits.max_height
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        return Err(ImageError::TooManyPixels { width, height });
    }
    let mut decode_limits = Limits::default();
    decode_limits.max_image_width = Some(limits.max_width);
    decode_limits.max_image_height = Some(limits.max_height);
    decode_limits.max_alloc = Some(limits.max_pixels.saturating_mul(8));
    decoder.set_limits(decode_limits).map_err(|e| corrupt(&e))?;

    let image = DynamicImage::from_decoder(decoder).map_err(|e| corrupt(&e))?;
    Ok(HashedImage {
        width,
        height,
        phash: perceptual_hash(&image),
        format,
    })
}

#[cfg(test)]
pub(crate) mod tests;

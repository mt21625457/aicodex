//! Byte-bounded transport copies. Geometry and source history are never changed.
use super::*;

/// Encoding steps for a bounded request planner. Every step starts from source bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TransportImageEncoding {
    Lossless,
    Jpeg85,
    Jpeg75,
    Jpeg65,
}

type TransportCache = BlockingLruCache<([u8; 20], TransportImageEncoding), String>;
static CACHE: LazyLock<TransportCache> =
    LazyLock::new(|| BlockingLruCache::new(NonZeroUsize::new(32).unwrap_or(NonZeroUsize::MIN)));
const MAX_SOURCE_BYTES: usize = 50 * 1024 * 1024;

/// Returns a smaller encoding when possible, without resizing or rotating pixels.
/// Preserves alpha and orientation metadata; errors never contain image contents.
/// Call from a blocking worker, not an async executor thread.
pub fn optimize_data_url_for_transport(
    url: &str,
    encoding: TransportImageEncoding,
) -> Result<String, String> {
    TransportImagePreparationCache::default().optimize(url, encoding)
}

/// 仅属于一次请求预算计算的解码缓存，避免跨编码档位反复解码原图。
/// 最多保留 32 张图片、合计 64 MiB 像素与元数据；超限仍即时处理，不丢弃图片。
#[derive(Default)]
pub struct TransportImagePreparationCache {
    entries: std::collections::HashMap<[u8; 20], PreparedTransportImage>,
    retained_bytes: usize,
}

const MAX_PREPARED_IMAGE_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
thread_local! {
    static TRANSPORT_DECODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct PreparedTransportImage {
    format: ImageFormat,
    metadata: ImageMetadata,
    decoded: DynamicImage,
    transparent: bool,
}

impl TransportImagePreparationCache {
    /// 各档位均从同一原始像素编码，保持首次满足预算的选择顺序与输出。
    /// 与单次优化入口相同，应从 blocking worker 调用。
    pub fn optimize(
        &mut self,
        url: &str,
        encoding: TransportImageEncoding,
    ) -> Result<String, String> {
        let key = (sha1_digest(url.as_bytes()), encoding);
        if let Some(encoded) = CACHE.get(&key) {
            return Ok(encoded);
        }
        if let Some(prepared) = self.entries.get(&key.0) {
            return encode_prepared_transport_image(url, encoding, key, prepared);
        }
        let Some(prepared) = prepare_transport_image(url, encoding)? else {
            return Ok(url.to_owned());
        };
        let result = encode_prepared_transport_image(url, encoding, key, &prepared)?;
        let retained_bytes = prepared
            .decoded
            .as_bytes()
            .len()
            .saturating_add(prepared.metadata.icc_profile.as_ref().map_or(0, Vec::len))
            .saturating_add(prepared.metadata.exif.as_ref().map_or(0, Vec::len));
        if self.entries.len() < 32
            && self.retained_bytes.saturating_add(retained_bytes) <= MAX_PREPARED_IMAGE_BYTES
        {
            self.retained_bytes += retained_bytes;
            self.entries.insert(key.0, prepared);
        }
        Ok(result)
    }
}

fn prepare_transport_image(
    url: &str,
    encoding: TransportImageEncoding,
) -> Result<Option<PreparedTransportImage>, String> {
    let (prefix, encoded) = url.split_once(";base64,").ok_or("Invalid image data URL")?;
    if encoded.len() > MAX_SOURCE_BYTES.div_ceil(3) * 4 {
        return Err("Image exceeds the 50 MiB preparation limit; crop it before sending".into());
    }
    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| "Invalid image base64")?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("Image exceeds the 50 MiB preparation limit; crop it before sending".into());
    }
    let format = image::guess_format(&bytes).map_err(|_| "Invalid image content")?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    ) || prefix != format!("data:{}", format_to_mime(format))
    {
        return Err("Image content does not match a supported media type".into());
    }
    if format == ImageFormat::Jpeg && encoding == TransportImageEncoding::Lossless {
        return Ok(None);
    }
    // Animation remains unchanged; the request planner reports any remaining oversize.
    if format == ImageFormat::Gif
        || (format == ImageFormat::Png
            && image::codecs::png::PngDecoder::new(Cursor::new(&bytes))
                .map_err(|_| "Invalid PNG")?
                .is_apng()
                .map_err(|_| "Invalid PNG animation")?)
        || (format == ImageFormat::WebP
            && image::codecs::webp::WebPDecoder::new(Cursor::new(&bytes))
                .map_err(|_| "Invalid WebP")?
                .has_animation())
    {
        return Ok(None);
    }
    if format == ImageFormat::Png {
        // The encoder can carry ICC/EXIF but not these color interpretation
        // chunks. Keep the source when recompression would discard them.
        let mut chunks = &bytes[8..];
        while chunks.len() >= 12 {
            let length = u32::from_be_bytes([chunks[0], chunks[1], chunks[2], chunks[3]]) as usize;
            let Some(chunk_bytes) = length.checked_add(12) else {
                return Err("Invalid PNG chunk length".into());
            };
            if chunk_bytes > chunks.len() {
                return Err("Invalid PNG chunk length".into());
            }
            if matches!(
                &chunks[4..8],
                b"gAMA" | b"cHRM" | b"sRGB" | b"cICP" | b"mDCV" | b"cLLI"
            ) {
                return Ok(None);
            }
            if &chunks[4..8] == b"IEND" {
                break;
            }
            chunks = &chunks[chunk_bytes..];
        }
    }
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "Cannot decode image within resource limits")?;
    let metadata = ImageMetadata {
        icc_profile: decoder
            .icc_profile()
            .map_err(|_| "Invalid image color profile")?,
        exif: decoder
            .exif_metadata()
            .map_err(|_| "Invalid image orientation metadata")?,
    };
    // JPEG candidates use RGB pixels. Without a color-management transform,
    // dropping a grayscale/CMYK profile or attaching it to RGB changes colors.
    // Lossless PNG keeps the native pixel type and its original profile.
    if encoding != TransportImageEncoding::Lossless
        && metadata
            .icc_profile
            .as_ref()
            .is_some_and(|profile| profile.get(16..20) != Some(b"RGB "))
    {
        return Ok(None);
    }
    #[cfg(test)]
    TRANSPORT_DECODE_COUNT.with(|count| count.set(count.get() + 1));
    let decoded = DynamicImage::from_decoder(decoder)
        .map_err(|_| "Cannot decode image within resource limits")?;
    // Inspect alpha at native precision: 65534/65535 must not round to opaque.
    let transparent = match &decoded {
        DynamicImage::ImageRgba16(pixels) => pixels.pixels().any(|pixel| pixel[3] != u16::MAX),
        DynamicImage::ImageLumaA16(pixels) => pixels.pixels().any(|pixel| pixel[1] != u16::MAX),
        DynamicImage::ImageRgba8(pixels) => pixels.pixels().any(|pixel| pixel[3] != u8::MAX),
        DynamicImage::ImageLumaA8(pixels) => pixels.pixels().any(|pixel| pixel[1] != u8::MAX),
        _ if decoded.color().has_alpha() => {
            decoded.to_rgba8().pixels().any(|pixel| pixel[3] != 255)
        }
        _ => false,
    };
    Ok(Some(PreparedTransportImage {
        format,
        metadata,
        decoded,
        transparent,
    }))
}

fn encode_prepared_transport_image(
    url: &str,
    encoding: TransportImageEncoding,
    key: ([u8; 20], TransportImageEncoding),
    prepared: &PreparedTransportImage,
) -> Result<String, String> {
    let PreparedTransportImage {
        format,
        metadata,
        decoded,
        transparent,
    } = prepared;
    if (*format == ImageFormat::Jpeg && encoding == TransportImageEncoding::Lossless)
        || (encoding != TransportImageEncoding::Lossless
            && metadata
                .icc_profile
                .as_ref()
                .is_some_and(|profile| profile.get(16..20) != Some(b"RGB ")))
    {
        return Ok(url.to_owned());
    }
    let mut best = url.to_owned();
    if encoding == TransportImageEncoding::Lossless {
        // Keep native channel precision. The general prompt encoder converts
        // to RGBA8, which is not lossless for original-detail 16-bit PNG inputs.
        let mut png = Vec::new();
        let mut encoder = PngEncoder::new(&mut png);
        apply_image_metadata(
            &mut encoder,
            metadata.icc_profile.clone(),
            metadata.exif.clone(),
            ImageFormat::Png,
        )
        .map_err(|_| "Cannot preserve image metadata")?;
        encoder
            .write_image(
                decoded.as_bytes(),
                decoded.width(),
                decoded.height(),
                decoded.color().into(),
            )
            .map_err(|_| "Cannot encode image as PNG")?;
        let candidate = data_url_from_bytes("image/png", &png);
        if candidate.len() < best.len() {
            best = candidate;
        }
    } else if !*transparent {
        let quality = match encoding {
            TransportImageEncoding::Lossless => unreachable!(),
            TransportImageEncoding::Jpeg85 => 85,
            TransportImageEncoding::Jpeg75 => 75,
            TransportImageEncoding::Jpeg65 => 65,
        };
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, quality);
        apply_image_metadata(
            &mut encoder,
            metadata.icc_profile.clone(),
            metadata.exif.clone(),
            ImageFormat::Jpeg,
        )
        .map_err(|_| "Cannot preserve image metadata")?;
        encoder
            .encode_image(&decoded.to_rgb8())
            .map_err(|_| "Cannot encode image as JPEG")?;
        let candidate = data_url_from_bytes("image/jpeg", &jpeg);
        if candidate.len() < best.len() {
            best = candidate;
        }
    }
    if best.len() <= 2 * 1024 * 1024 {
        CACHE.insert(key, best.clone());
    }
    Ok(best)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

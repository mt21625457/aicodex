use super::*;

fn noisy_png(alpha: u8) -> Vec<u8> {
    let mut seed = 42u32;
    let pixels = image::RgbaImage::from_fn(400, 300, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        image::Rgba([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, alpha])
    });
    let mut bytes = Cursor::new(Vec::new());
    pixels.write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn transport_compression_preserves_geometry_and_source() {
    let source = noisy_png(/*alpha*/ 255);
    let url = data_url_from_bytes("image/png", &source);
    let budget = 200 * 1024;
    assert!(source.len() > budget);
    let prepared = optimize_data_url_for_transport(&url, TransportImageEncoding::Jpeg85).unwrap();
    let decoded = BASE64_STANDARD
        .decode(prepared.split_once(',').unwrap().1)
        .unwrap();
    assert!(decoded.len() <= budget);
    assert_eq!(
        image::load_from_memory(&decoded).unwrap().dimensions(),
        (400, 300)
    );
    assert_eq!(url, data_url_from_bytes("image/png", &source));
}

#[test]
fn oversized_transparency_is_never_flattened_or_resized() {
    let url = data_url_from_bytes("image/png", &noisy_png(/*alpha*/ 127));
    let prepared = optimize_data_url_for_transport(&url, TransportImageEncoding::Jpeg65).unwrap();
    assert_eq!(prepared, url);
}

#[test]
fn lossless_encoding_preserves_pixels_and_invalid_images_fail_privately() {
    let url = data_url_from_bytes("image/png", &noisy_png(/*alpha*/ 255));
    let prepared = optimize_data_url_for_transport(&url, TransportImageEncoding::Lossless).unwrap();
    let decoded = BASE64_STANDARD
        .decode(prepared.split_once(',').unwrap().1)
        .unwrap();
    assert_eq!(
        image::load_from_memory(&decoded).unwrap().to_rgba8(),
        image::load_from_memory(&noisy_png(255)).unwrap().to_rgba8()
    );
    for bad in [
        "data:image/png;base64,private-secret",
        "data:image/png;base64,eA==",
    ] {
        let error =
            optimize_data_url_for_transport(bad, TransportImageEncoding::Lossless).unwrap_err();
        assert!(!error.contains("private-secret"));
        assert!(!error.contains("base64") || error == "Invalid image base64");
    }
}

#[test]
fn jpeg_transport_copy_preserves_exif_orientation() {
    let source = image::load_from_memory(&noisy_png(/*alpha*/ 255))
        .unwrap()
        .to_rgb8();
    let exif = vec![
        0x49, 0x49, 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, /*quality*/ 100);
    encoder.set_exif_metadata(exif).unwrap();
    encoder.encode_image(&source).unwrap();
    let budget = 200 * 1024;
    assert!(bytes.len() > budget);
    let prepared = optimize_data_url_for_transport(
        &data_url_from_bytes("image/jpeg", &bytes),
        TransportImageEncoding::Jpeg85,
    )
    .unwrap();
    let decoded = BASE64_STANDARD
        .decode(prepared.split_once(',').unwrap().1)
        .unwrap();
    let mut decoder = ImageReader::with_format(Cursor::new(decoded), ImageFormat::Jpeg)
        .into_decoder()
        .unwrap();
    assert_eq!(decoder.dimensions(), (400, 300));
    assert_eq!(
        decoder.orientation().unwrap(),
        image::metadata::Orientation::Rotate90
    );
}

#[test]
fn lossless_original_png_retains_sixteen_bit_pixels_and_alpha() {
    let pixels = image::ImageBuffer::from_pixel(8, 8, image::Rgba([12345u16, 23456, 34567, 45678]));
    let mut source = Cursor::new(Vec::new());
    DynamicImage::ImageRgba16(pixels.clone())
        .write_to(&mut source, ImageFormat::Png)
        .unwrap();
    let mut bytes = source.into_inner();
    bytes.resize(bytes.len() + 4096, 0);
    let url = data_url_from_bytes("image/png", &bytes);
    let prepared = optimize_data_url_for_transport(&url, TransportImageEncoding::Lossless).unwrap();
    assert!(prepared.len() < url.len());
    let bytes = BASE64_STANDARD
        .decode(prepared.split_once(',').unwrap().1)
        .unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!(decoded.color(), image::ColorType::Rgba16);
    assert_eq!(decoded.to_rgba16(), pixels);
}

#[test]
fn almost_opaque_sixteen_bit_alpha_is_not_flattened_by_jpeg() {
    let pixels = image::ImageBuffer::from_pixel(8, 8, image::Rgba([12345u16, 23456, 34567, 65534]));
    let mut source = Cursor::new(Vec::new());
    DynamicImage::ImageRgba16(pixels)
        .write_to(&mut source, ImageFormat::Png)
        .unwrap();
    let mut bytes = source.into_inner();
    bytes.resize(bytes.len() + 4096, 0);
    let url = data_url_from_bytes("image/png", &bytes);
    assert_eq!(
        optimize_data_url_for_transport(&url, TransportImageEncoding::Jpeg65).unwrap(),
        url
    );
}

#[test]
fn grayscale_profile_survives_lossless_encoding_and_prevents_rgb_conversion() {
    let mut profile = vec![0u8; 132];
    profile[..4].copy_from_slice(&132u32.to_be_bytes());
    profile[12..16].copy_from_slice(b"mntr");
    profile[16..20].copy_from_slice(b"GRAY");
    profile[20..24].copy_from_slice(b"XYZ ");
    profile[36..40].copy_from_slice(b"acsp");
    let pixels = image::GrayImage::from_pixel(8, 8, image::Luma([128]));
    let mut bytes = Vec::new();
    let mut encoder = PngEncoder::new(&mut bytes);
    encoder.set_icc_profile(profile.clone()).unwrap();
    encoder
        .write_image(pixels.as_raw(), 8, 8, image::ExtendedColorType::L8)
        .unwrap();
    bytes.resize(bytes.len() + 4096, 0);
    let url = data_url_from_bytes("image/png", &bytes);
    let prepared = optimize_data_url_for_transport(&url, TransportImageEncoding::Lossless).unwrap();
    assert!(prepared.len() < url.len());
    let bytes = BASE64_STANDARD
        .decode(prepared.split_once(',').unwrap().1)
        .unwrap();
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
        .into_decoder()
        .unwrap();
    assert_eq!(decoder.icc_profile().unwrap(), Some(profile));
    assert_eq!(
        DynamicImage::from_decoder(decoder).unwrap().to_luma8(),
        pixels
    );
    assert_eq!(
        optimize_data_url_for_transport(&url, TransportImageEncoding::Jpeg65).unwrap(),
        url
    );
}

#[test]
fn png_gamma_is_not_discarded_by_either_encoding_path() {
    let mut bytes = noisy_png(/*alpha*/ 255);
    // gAMA=1.0, including the PNG chunk CRC. Insert after IHDR.
    bytes.splice(
        33..33,
        [
            0, 0, 0, 4, 103, 65, 77, 65, 0, 1, 134, 160, 49, 232, 150, 95,
        ],
    );
    assert!(image::load_from_memory(&bytes).is_ok());
    bytes.resize(bytes.len() + 4096, 0);
    let url = data_url_from_bytes("image/png", &bytes);
    for encoding in [
        TransportImageEncoding::Lossless,
        TransportImageEncoding::Jpeg65,
    ] {
        assert_eq!(
            optimize_data_url_for_transport(&url, encoding).unwrap(),
            url
        );
    }
}

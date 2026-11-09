use image::{DynamicImage, ImageBuffer, Rgba};

pub fn decode_highres(bytes: &[u8]) -> Result<DynamicImage, String> {
    let parsed = vtf::from_bytes(bytes).map_err(|error| format!("VTF parse failed: {error}"))?;
    if parsed.highres_image.format != vtf::ImageFormat::Bgrx8888 {
        return parsed
            .highres_image
            .decode(0)
            .map_err(|error| format!("VTF decode failed: {error}"));
    }
    let header = &parsed.header;
    if header.version > [7, 2] {
        return Err("Bgrx8888 VTF resources are not supported by this decoder.".into());
    }
    let lowres_size = header
        .lowres_image_format
        .frame_size(
            header.lowres_image_width as u32,
            header.lowres_image_height as u32,
        )
        .map_err(|error| error.to_string())? as usize;
    let mut offset = header.header_size as usize + lowres_size;
    for level in 1..header.mipmap_count as u32 {
        let width = (header.width as u32 >> level).max(1);
        let height = (header.height as u32 >> level).max(1);
        offset += (width * height * 4) as usize;
    }
    let source = bytes
        .get(offset..offset + header.width as usize * header.height as usize * 4)
        .ok_or("Truncated BGRX VTF")?;
    let mut rgba = Vec::with_capacity(source.len());
    for pixel in source.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    ImageBuffer::<Rgba<u8>, _>::from_raw(header.width as u32, header.height as u32, rgba)
        .map(DynamicImage::ImageRgba8)
        .ok_or("Invalid BGRX dimensions".into())
}

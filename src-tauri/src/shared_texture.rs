use super::*;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentTexture {
    pub part_path: String,
    pub material_index: usize,
    pub png: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CombinedTexture {
    pub png: String,
    conflicts: Option<String>,
    pub conflict_pixels: usize,
    overlap_pixels: usize,
}

struct Layer {
    pixels: image::RgbaImage,
    uvs: Vec<f32>,
}

fn png(image: &image::RgbaImage) -> Result<String, String> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).map_err(|error| error.to_string())?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes.into_inner())))
}

fn decode_png(data: &str) -> Result<image::RgbaImage, String> {
    if data.len() > 96 * 1024 * 1024 { return Err("Component PNG exceeds 72 MiB".into()); }
    let bytes = STANDARD.decode(data.strip_prefix("data:image/png;base64,").unwrap_or(data))
        .map_err(|error| error.to_string())?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode().map_err(|error| format!("Invalid component PNG: {error}"))?.to_rgba8())
}

fn edge(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
}

#[cfg(test)]
fn rasterize(uvs: &[f32], width: u32, height: u32) -> Result<Vec<u8>, String> {
    rasterize_with_budget(uvs, width, height, &mut 0)
}

fn rasterize_with_budget(uvs: &[f32], width: u32, height: u32, work: &mut u64) -> Result<Vec<u8>, String> {
    if uvs.len() % 6 != 0 || uvs.iter().any(|value| !value.is_finite() || value.abs() > 128.) {
        return Err("Component has invalid or excessively tiled UV coordinates".into());
    }
    let mut mask = vec![0; width as usize * height as usize];
    for triangle in uvs.chunks_exact(6) {
        let points = [[triangle[0] as f64 * width as f64, triangle[1] as f64 * height as f64],
            [triangle[2] as f64 * width as f64, triangle[3] as f64 * height as f64],
            [triangle[4] as f64 * width as f64, triangle[5] as f64 * height as f64]];
        let area = edge(points[0], points[1], points[2]);
        if area.abs() < 1e-10 { continue; }
        let min_x = points.iter().map(|point| point[0]).fold(f64::INFINITY, f64::min).floor() as i32;
        let max_x = points.iter().map(|point| point[0]).fold(f64::NEG_INFINITY, f64::max).ceil() as i32;
        let min_y = points.iter().map(|point| point[1]).fold(f64::INFINITY, f64::min).floor() as i32;
        let max_y = points.iter().map(|point| point[1]).fold(f64::NEG_INFINITY, f64::max).ceil() as i32;
        *work += (max_x - min_x) as u64 * (max_y - min_y) as u64;
        if *work > 250_000_000 { return Err("UV composition is too large; excessively repeated UVs cannot be combined".into()); }
        let edge_radii = [(points[1][0] - points[0][0]).abs() + (points[1][1] - points[0][1]).abs(),
            (points[2][0] - points[1][0]).abs() + (points[2][1] - points[1][1]).abs(),
            (points[0][0] - points[2][0]).abs() + (points[0][1] - points[2][1]).abs()];
        for y in min_y..max_y {
            for x in min_x..max_x {
                let point = [x as f64 + 0.5, y as f64 + 0.5];
                let edges = [edge(points[0], points[1], point), edge(points[1], points[2], point), edge(points[2], points[0], point)];
                // Include every texel square touched by a triangle, even when its centre lies outside a thin UV island.
                if edges.iter().zip(edge_radii).all(|(edge, radius)| edge * area.signum() + radius * 0.5 > 1e-8) {
                    let x = x.rem_euclid(width as i32) as usize;
                    let y = y.rem_euclid(height as i32) as usize;
                    mask[y * width as usize + x] = 1;
                }
            }
        }
    }
    Ok(mask)
}

pub(super) fn uv_pixels_differ(before: &image::RgbaImage, after: &image::RgbaImage, uvs: &[f32]) -> Result<bool, String> {
    if before == after { return Ok(false); }
    let width = before.width().max(after.width());
    let height = before.height().max(after.height());
    textures::validate_replacement_dimensions(width, height, before.width(), before.height())?;
    textures::validate_replacement_dimensions(width, height, after.width(), after.height())?;
    let mask = rasterize_with_budget(uvs, width, height, &mut 0)?;
    Ok(mask.into_iter().enumerate().any(|(index, used)| {
        if used == 0 { return false; }
        let (x, y) = (index as u32 % width, index as u32 / width);
        before.get_pixel(x * before.width() / width, y * before.height() / height)
            != after.get_pixel(x * after.width() / width, y * after.height() / height)
    }))
}

fn compose(mut output: image::RgbaImage, layers: &[Layer]) -> Result<CombinedTexture, String> {
    if layers.is_empty() || layers.len() > 32 {
        return Err("Choose 1-32 components with texture dimensions up to 4096 x 4096".into());
    }
    let (base_width, base_height) = output.dimensions();
    textures::validate_replacement_dimensions(base_width, base_height, base_width, base_height)?;
    let (mut width, mut height) = (base_width, base_height);
    for layer in layers {
        textures::validate_replacement_dimensions(layer.pixels.width(), layer.pixels.height(), base_width, base_height)?;
        width = width.max(layer.pixels.width());
        height = height.max(layer.pixels.height());
    }
    if width as u64 * height as u64 * layers.len() as u64 > 64 * 1024 * 1024 {
        return Err("Combined component textures exceed the 64-million-pixel working limit".into());
    }
    if output.dimensions() != (width, height) {
        output = image::imageops::resize(&output, width, height, image::imageops::FilterType::Nearest);
    }
    // Integer nearest-neighbour scaling preserves alpha and hidden RGB without allocating upscaled sources.
    let sample = |source: &image::RgbaImage, x: u32, y: u32| {
        *source.get_pixel(x * source.width() / width, y * source.height() / height)
    };
    let count = width as usize * height as usize;
    let mut owner = vec![u8::MAX; count];
    let mut overlaps = vec![false; count];
    let mut conflicts = image::RgbaImage::new(width, height);
    let mut conflict_pixels = 0;
    let mut overlap_pixels = 0;
    let mut work = 0;
    for (layer_index, layer) in layers.iter().enumerate() {
        let mask = rasterize_with_budget(&layer.uvs, width, height, &mut work)?;
        for (index, used) in mask.iter().enumerate() {
            if *used == 0 { continue; }
            let x = index as u32 % width;
            let y = index as u32 / width;
            let pixel = sample(&layer.pixels, x, y);
            if owner[index] == u8::MAX {
                owner[index] = layer_index as u8;
                output.put_pixel(x, y, pixel);
            } else {
                if !overlaps[index] { overlaps[index] = true; overlap_pixels += 1; }
                if *output.get_pixel(x, y) != pixel && conflicts.get_pixel(x, y)[3] == 0 {
                    conflicts.put_pixel(x, y, image::Rgba([255, 0, 170, 255]));
                    conflict_pixels += 1;
                }
            }
        }
    }
    // Two texels of padding reduce filtering seams without overwriting another component's UVs.
    let mut padded = output.clone();
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            if owner[index] != u8::MAX { continue; }
            let mut selected = None;
            'padding: for radius in 1..=2_i32 {
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        if dx.abs().max(dy.abs()) != radius { continue; }
                        let sx = (x as i32 + dx).rem_euclid(width as i32) as u32;
                        let sy = (y as i32 + dy).rem_euclid(height as i32) as u32;
                        let source = owner[(sy * width + sx) as usize];
                        if source != u8::MAX { selected = Some(source); break 'padding; }
                    }
                }
            }
            if let Some(source) = selected { padded.put_pixel(x, y, sample(&layers[source as usize].pixels, x, y)); }
        }
    }
    Ok(CombinedTexture {png:png(&padded)?, conflicts:if conflict_pixels > 0 { Some(png(&conflicts)?) } else { None },
        conflict_pixels, overlap_pixels})
}

#[tauri::command]
pub async fn combine_shared_texture(
    vtf_path: String, base_png: String, components: Vec<ComponentTexture>, state: State<'_, AppState>,
) -> Result<CombinedTexture, String> {
    let index = state.0.lock().map_err(|_| "Index lock failed")?.clone();
    tauri::async_runtime::spawn_blocking(move || combine(&index, &vtf_path, &base_png, &components))
        .await.map_err(|error| format!("Shared texture composition failed: {error}"))?
}

fn combine(index: &ContentIndex, vtf_path: &str, base_png: &str, components: &[ComponentTexture]) -> Result<CombinedTexture, String> {
    if components.is_empty() || components.len() > 32 { return Err("Choose 1-32 components sharing one VTF".into()); }
    let base = decode_png(base_png)?;
    let layers = {
        let mut layers = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut source_pixels = 0_u64;
        for component in components {
            if !seen.insert((component.part_path.clone(), component.material_index)) {
                return Err("A component material was selected more than once".into());
            }
            let detail = build_model(&index, &component.part_path)?;
            let material = detail.materials.get(component.material_index).ok_or("Component material no longer exists")?;
            if material.vtf_path.as_deref() != Some(vtf_path) { return Err("Every component must use the selected shared VTF".into()); }
            let pixels = decode_png(&component.png)?;
            let (width, height) = (material.width.unwrap_or(0), material.height.unwrap_or(0));
            textures::validate_replacement_dimensions(pixels.width(), pixels.height(), width, height)
                .map_err(|error| format!("{}: {error}", component.part_path))?;
            if layers.is_empty() {
                textures::validate_replacement_dimensions(base.width(), base.height(), width, height)?;
            }
            source_pixels += pixels.width() as u64 * pixels.height() as u64;
            if source_pixels > 64 * 1024 * 1024 {
                return Err("Combined component textures exceed the 64-million-pixel working limit".into());
            }
            let uvs = detail.meshes.iter().filter(|mesh| mesh.material_index == component.material_index)
                .flat_map(|mesh| mesh.uvs.iter().copied()).collect::<Vec<_>>();
            if uvs.is_empty() { return Err(format!("{} has no UV triangles for this material", component.part_path)); }
            layers.push(Layer {pixels, uvs});
        }
        layers
    };
    compose(base, &layers)
}

pub(super) fn flatten_draft(index: &ContentIndex, draft: &drafts::Draft) -> Result<drafts::Draft, String> {
    let mut output = draft.clone();
    let mut groups = std::collections::BTreeMap::<String, Vec<ComponentTexture>>::new();
    let mut seen = std::collections::HashSet::new();
    for component in &draft.component_textures {
        if !seen.insert((key(&component.part_path), component.material_index)) {
            return Err("Draft contains a duplicate component texture".into());
        }
        let detail = build_model(index, &component.part_path)?;
        let material = detail.materials.get(component.material_index).ok_or("Component material no longer exists")?;
        let vtf = material.vtf_path.as_ref().ok_or("Component has no original VTF")?;
        if !draft.textures.iter().any(|texture| texture.vtf_path.as_ref() == Some(vtf)) {
            return Err("Component has no shared base texture in this draft".into());
        }
        groups.entry(vtf.clone()).or_default().push(component.clone());
    }
    for (vtf, components) in groups {
        let base = draft.textures.iter().find(|texture| texture.vtf_path.as_ref() == Some(&vtf)).unwrap();
        let result = combine(index, &vtf, &base.png, &components)?;
        if result.conflict_pixels > 0 {
            return Err(format!("{}: {} UV pixels have conflicting component colours or alpha. Choose compatible component textures before installing, exporting or publishing.",
                vtf.rsplit('/').next().unwrap_or(&vtf), result.conflict_pixels));
        }
        for texture in output.textures.iter_mut().filter(|texture| texture.vtf_path.as_ref() == Some(&vtf)) {
            texture.png = result.png.clone();
        }
    }
    output.component_textures.clear();
    output.component_error = None;
    Ok(output)
}

fn merge_regions(base: image::RgbaImage, layers: &[Layer], protected_uvs: &[Vec<f32>]) -> Result<image::RgbaImage, String> {
    if layers.is_empty() || layers.len() > 32 { return Err("Choose 1-32 component sources to merge".into()); }
    let (mut width, mut height) = base.dimensions();
    textures::validate_replacement_dimensions(width, height, width, height)?;
    for layer in layers {
        textures::validate_replacement_dimensions(layer.pixels.width(), layer.pixels.height(), width, height)?;
        width = width.max(layer.pixels.width());
        height = height.max(layer.pixels.height());
    }
    if width as u64 * height as u64 * layers.len() as u64 > 64 * 1024 * 1024 {
        return Err("Merged component textures exceed the 64-million-pixel working limit".into());
    }
    let mut output = if base.dimensions() == (width, height) { base }
        else { image::imageops::resize(&base, width, height, image::imageops::FilterType::Nearest) };
    let count = width as usize * height as usize;
    let mut protected = vec![0_u8; count];
    let mut work = 0;
    for uvs in protected_uvs {
        let mask = rasterize_with_budget(uvs, width, height, &mut work)?;
        for (used, value) in protected.iter_mut().zip(mask) { *used |= value; }
    }
    let mut owners = vec![false; count];
    let mut conflicts = 0;
    for layer in layers {
        let mask = rasterize_with_budget(&layer.uvs, width, height, &mut work)?;
        for (index, used) in mask.into_iter().enumerate() {
            if used == 0 { continue; }
            let (x, y) = (index as u32 % width, index as u32 / width);
            let pixel = *layer.pixels.get_pixel(x * layer.pixels.width() / width, y * layer.pixels.height() / height);
            if (protected[index] != 0 || owners[index]) && *output.get_pixel(x, y) != pixel {
                conflicts += 1;
            } else {
                output.put_pixel(x, y, pixel);
                owners[index] = true;
            }
        }
    }
    if conflicts > 0 {
        return Err(format!("Merge blocked: {conflicts} UV pixels conflict with another component. The installed skin was not changed. Use compatible textures or explicitly replace the entire atlas."));
    }
    // No padding: even neighbouring texels outside the selected UVs must retain the installed pixels.
    Ok(output)
}

pub(super) fn merge_components(index: &ContentIndex, vtf: &str, base: image::RgbaImage,
    components: &[ComponentTexture]) -> Result<String, String> {
    let mut selected = std::collections::HashSet::new();
    let mut layers = Vec::new();
    for component in components {
        let part_path = key(&component.part_path);
        if !selected.insert((part_path.clone(), component.material_index)) { return Err("Duplicate merge component".into()); }
        let detail = build_model(index, &part_path)?;
        let material = detail.materials.get(component.material_index).ok_or("Merge component material is missing")?;
        if material.vtf_path.as_deref() != Some(vtf) { return Err("Merge component uses a different VTF".into()); }
        textures::validate_replacement_dimensions(base.width(), base.height(), material.width.unwrap_or(0), material.height.unwrap_or(0))?;
        let pixels = decode_png(&component.png)?;
        textures::validate_replacement_dimensions(pixels.width(), pixels.height(), material.width.unwrap_or(0), material.height.unwrap_or(0))?;
        let uvs: Vec<_> = detail.meshes.iter().filter(|mesh| mesh.material_index == component.material_index)
            .flat_map(|mesh| mesh.uvs.iter().copied()).collect();
        if uvs.is_empty() { return Err(format!("{} has no UV geometry to merge", component.part_path)); }
        layers.push(Layer {pixels, uvs});
    }
    let mut protected = Vec::new();
    let texture_re = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+(?:"([^"\r\n]+)"|([^"\s]+))"#).unwrap();
    for (path, source) in index.files.iter().filter(|(path, _)| path.ends_with(".mdl") && is_skin_target_model(path)) {
        let mdl = model::mdl::read_mdl_bytes(&source.read()?).map_err(|error| format!("Could not inspect {path}: {error}"))?;
        let mut uses_vtf = false;
        for texture in &mdl.textures {
            let material = material_candidates(&texture.name, &mdl.cd_texture_paths).into_iter()
                .find_map(|candidate| find(index, &candidate));
            let Some(material) = material else { continue; };
            let text = String::from_utf8(material.read()?).map_err(|error| error.to_string())?;
            if texture_re.captures(&text).and_then(|capture| capture.get(1).or_else(|| capture.get(2)))
                .is_some_and(|reference| key(&base_texture_path(reference.as_str())) == vtf) {
                uses_vtf = true; break;
            }
        }
        if !uses_vtf { continue; }
        let detail = build_model(index, path)?;
        for (material_index, material) in detail.materials.iter().enumerate() {
            if material.vtf_path.as_deref() != Some(vtf) || selected.contains(&(key(path), material_index)) { continue; }
            let uvs = detail.meshes.iter().filter(|mesh| mesh.material_index == material_index)
                .flat_map(|mesh| mesh.uvs.iter().copied()).collect::<Vec<_>>();
            if !uvs.is_empty() { protected.push(uvs); }
        }
    }
    png(&merge_regions(base, &layers, &protected)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(left: f32, right: f32) -> Vec<f32> {
        vec![left, 0., right, 0., right, 1., left, 0., right, 1., left, 1.]
    }

    fn layer(color: [u8; 4], uvs: Vec<f32>) -> Layer {
        Layer {pixels:image::RgbaImage::from_pixel(16, 8, image::Rgba(color)), uvs}
    }

    #[test]
    fn install_impact_compares_only_used_uv_pixels_at_normalized_resolutions() {
        let before = image::RgbaImage::from_pixel(16, 8, image::Rgba([5, 6, 7, 42]));
        let mut after = image::RgbaImage::from_pixel(32, 16, image::Rgba([5, 6, 7, 42]));
        assert!(!uv_pixels_differ(&before, &after, &rectangle(0., 1.)).unwrap());
        after.put_pixel(28, 8, image::Rgba([20, 30, 40, 42]));
        assert!(!uv_pixels_differ(&before, &after, &rectangle(0., 0.25)).unwrap());
        assert!(uv_pixels_differ(&before, &after, &rectangle(0.75, 1.)).unwrap());
        assert!(uv_pixels_differ(&before, &after, &rectangle(1.75, 2.)).unwrap());
        after.put_pixel(28, 8, image::Rgba([5, 6, 7, 0]));
        assert!(uv_pixels_differ(&before, &after, &rectangle(0.75, 1.)).unwrap(), "Alpha-only changes count");
        assert!(!uv_pixels_differ(&before, &after, &[]).unwrap());
        assert!(uv_pixels_differ(&before, &after, &[f32::NAN; 6]).is_err());
    }

    #[test]
    fn combines_disjoint_regions_preserving_hidden_rgb_alpha_and_base() {
        let base = image::RgbaImage::from_pixel(16, 8, image::Rgba([5, 6, 7, 255]));
        let result = compose(base, &[layer([200, 30, 40, 0], rectangle(0., 0.25)),
            layer([20, 50, 210, 170], rectangle(0.75, 1.))]).unwrap();
        assert_eq!(result.conflict_pixels, 0);
        assert_eq!(result.overlap_pixels, 0);
        let image = decode_png(&result.png).unwrap();
        assert_eq!(image.get_pixel(1, 4).0, [200, 30, 40, 0]);
        assert_eq!(image.get_pixel(14, 4).0, [20, 50, 210, 170]);
        assert_eq!(image.get_pixel(7, 4).0, [5, 6, 7, 255]);
        assert_eq!(image.get_pixel(4, 4).0, [200, 30, 40, 0]);
    }

    #[test]
    fn install_merge_preserves_unselected_pixels_and_blocks_other_component_uv_conflicts() {
        let base = image::RgbaImage::from_pixel(16, 8, image::Rgba([5, 6, 7, 42]));
        let incoming = Layer {pixels:image::RgbaImage::from_pixel(32, 16, image::Rgba([200, 30, 40, 0])),
            uvs:rectangle(0., 0.25)};
        let merged = merge_regions(base.clone(), &[incoming], &[rectangle(0.75, 1.)]).unwrap();
        assert_eq!(merged.dimensions(), (32, 16));
        for y in 0..16 {
            for x in 0..32 {
                assert_eq!(merged.get_pixel(x, y).0, if x < 8 { [200, 30, 40, 0] } else { [5, 6, 7, 42] });
            }
        }
        let overlapping = Layer {pixels:image::RgbaImage::from_pixel(32, 16, image::Rgba([200, 30, 40, 0])),
            uvs:rectangle(0., 0.75)};
        assert!(merge_regions(base.clone(), &[overlapping], &[rectangle(0.5, 1.)]).unwrap_err().contains("Merge blocked"));
        let identical = layer([5, 6, 7, 42], rectangle(0., 1.));
        assert_eq!(merge_regions(base.clone(), &[identical], &[rectangle(0., 1.)]).unwrap(), base);
        assert!(merge_regions(base.clone(), &[
            layer([200, 0, 0, 255], rectangle(0., 0.5)),
            layer([0, 200, 0, 255], rectangle(0., 0.5)),
        ], &[]).is_err());
        let high = image::RgbaImage::from_pixel(64, 32, image::Rgba([9, 8, 7, 0]));
        let result = merge_regions(high, &[layer([4, 3, 2, 1], rectangle(0., 0.25))], &[]).unwrap();
        assert_eq!(result.dimensions(), (64, 32));
        assert_eq!(result.get_pixel(63, 31).0, [9, 8, 7, 0]);
    }

    #[test]
    fn mixed_resolutions_use_largest_source_and_preserve_rgba_and_uv_positions() {
        let base = image::RgbaImage::from_pixel(16, 8, image::Rgba([5, 6, 7, 255]));
        let mut small = image::RgbaImage::from_pixel(8, 4, image::Rgba([200, 30, 40, 0]));
        small.put_pixel(1, 2, image::Rgba([17, 18, 19, 70]));
        let mut large = image::RgbaImage::from_pixel(32, 16, image::Rgba([20, 50, 210, 170]));
        large.put_pixel(30, 8, image::Rgba([9, 8, 7, 6]));
        let result = compose(base, &[
            Layer {pixels:small.clone(), uvs:rectangle(0., 0.25)},
            Layer {pixels:large.clone(), uvs:rectangle(0.75, 1.)},
        ]).unwrap();
        assert_eq!(result.conflict_pixels, 0);
        let image = decode_png(&result.png).unwrap();
        assert_eq!(image.dimensions(), (32, 16));
        assert_eq!(image.get_pixel(0, 8).0, [200, 30, 40, 0]);
        assert_eq!(image.get_pixel(4, 8).0, [17, 18, 19, 70]);
        assert_eq!(image.get_pixel(30, 8).0, [9, 8, 7, 6]);
        assert_eq!(image.get_pixel(16, 8).0, [5, 6, 7, 255]);
        let high_base = compose(image::RgbaImage::new(64, 32),
            &[Layer {pixels:small.clone(), uvs:rectangle(0., 1.)}]).unwrap();
        assert_eq!(decode_png(&high_base.png).unwrap().dimensions(), (64, 32));
        let upscaled = image::imageops::resize(&small, 32, 16, image::imageops::FilterType::Nearest);
        let identical = compose(image::RgbaImage::new(16, 8), &[
            Layer {pixels:small.clone(), uvs:rectangle(0., 1.)},
            Layer {pixels:upscaled, uvs:rectangle(0., 1.)},
        ]).unwrap();
        assert_eq!(identical.conflict_pixels, 0);
        let conflict = compose(image::RgbaImage::new(16, 8), &[
            Layer {pixels:small, uvs:rectangle(0., 1.)},
            Layer {pixels:large, uvs:rectangle(0.5, 1.)},
        ]).unwrap();
        assert_eq!(conflict.conflict_pixels, 16 * 16);
        let vtf = encode_vtf(&DynamicImage::ImageRgba8(image.clone())).unwrap();
        assert_eq!(vtf_decode::decode_highres(&vtf).unwrap().to_rgba8(), image);
    }

    #[test]
    fn conflicts_count_pixels_once_and_identical_overlaps_are_safe() {
        let base = image::RgbaImage::new(16, 8);
        let red = [200, 0, 0, 255];
        let same = compose(base.clone(), &[layer(red, rectangle(0., 1.)), layer(red, rectangle(0., 1.))]).unwrap();
        assert_eq!(same.overlap_pixels, 128);
        assert_eq!(same.conflict_pixels, 0);
        let conflict = compose(base, &[layer(red, rectangle(0., 1.)),
            layer([0, 0, 200, 255], rectangle(0.5, 1.)),
            layer([0, 200, 0, 255], rectangle(0.5, 1.))]).unwrap();
        assert_eq!(conflict.conflict_pixels, 64);
        assert_eq!(decode_png(conflict.conflicts.as_ref().unwrap()).unwrap().get_pixel(10, 4).0, [255, 0, 170, 255]);
    }

    #[test]
    fn repeating_uvs_wrap_and_invalid_dimensions_and_uvs_are_rejected() {
        let mask = rasterize(&rectangle(1., 1.5), 16, 8).unwrap();
        assert_eq!(mask.iter().filter(|pixel| **pixel == 1).count(), 64);
        assert!(mask[0..8].iter().all(|pixel| *pixel == 1));
        assert!(rasterize(&[f32::NAN; 6], 16, 8).is_err());
        assert!(rasterize(&rectangle(0., 129.), 16, 8).is_err());
        assert!(compose(image::RgbaImage::new(8, 8), &[layer([0; 4], rectangle(0., 1.))]).is_err());
        let thin = rasterize(&[0.01, 0.01, 0.02, 0.01, 0.01, 0.02], 16, 8).unwrap();
        assert_eq!(thin.iter().filter(|used| **used != 0).count(), 1);
        assert_eq!(thin[0], 1, "Subpixel UV islands must not silently keep the base texture");
        let mut exhausted = 250_000_000;
        assert!(rasterize_with_budget(&rectangle(0., 1.), 16, 8, &mut exhausted).is_err());
    }

    #[test]
    fn actual_ak74u_and_stock_combine_shared_uvs_and_export_one_vtf() {
        let Ok(content) = std::env::var("PERP_TEST_CONTENT") else { return; };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&content), &mut index.files);
        let gun = build_model(&index, "models/perpheads/weapons/c_ak74u.mdl").unwrap();
        let stock = build_model(&index, "models/perpheads/weapons/attachments/stock_ak74.mdl").unwrap();
        println!("Gun materials: {:?}", gun.materials.iter().map(|material| &material.vtf_path).collect::<Vec<_>>());
        println!("Stock materials: {:?}", stock.materials.iter().map(|material| &material.vtf_path).collect::<Vec<_>>());
        let mut checked = 0;
        for (gun_index, material) in gun.materials.iter().enumerate() {
            let Some((stock_index, _)) = stock.materials.iter().enumerate().find(|(_, other)|
                material.vtf_path.is_some() && material.vtf_path == other.vtf_path) else { continue; };
            let (width, height) = (material.width.unwrap(), material.height.unwrap());
            let gun_uvs: Vec<_> = gun.meshes.iter().filter(|mesh| mesh.material_index == gun_index).flat_map(|mesh| mesh.uvs.clone()).collect();
            let stock_uvs: Vec<_> = stock.meshes.iter().filter(|mesh| mesh.material_index == stock_index).flat_map(|mesh| mesh.uvs.clone()).collect();
            if gun_uvs.is_empty() || stock_uvs.is_empty() { continue; }
            let red = [210, 35, 40, 100];
            let blue = [20, 70, 220, 255];
            let gun_mask = rasterize(&gun_uvs, width, height).unwrap();
            let stock_mask = rasterize(&stock_uvs, width, height).unwrap();
            let overlap = gun_mask.iter().zip(&stock_mask).filter(|(gun, stock)| **gun != 0 && **stock != 0).count();
            let baseline = image::RgbaImage::from_pixel(width, height, image::Rgba([90, 90, 90, 255]));
            let result = compose(baseline, &[
                Layer {pixels:image::RgbaImage::from_pixel(width, height, image::Rgba(red)), uvs:gun_uvs},
                Layer {pixels:image::RgbaImage::from_pixel(width, height, image::Rgba(blue)), uvs:stock_uvs},
            ]).unwrap();
            println!("{}: {} shared UV pixels, {} conflicts", material.vtf_path.as_ref().unwrap(), result.overlap_pixels, result.conflict_pixels);
            assert_eq!(result.conflict_pixels, overlap);
            let combined = decode_png(&result.png).unwrap();
            for (pixel, (gun, stock)) in combined.pixels().zip(gun_mask.iter().zip(&stock_mask)) {
                if *gun != 0 { assert_eq!(pixel.0, red); }
                else if *stock != 0 { assert_eq!(pixel.0, blue); }
            }
            let vtf = encode_vtf(&DynamicImage::ImageRgba8(combined.clone())).unwrap();
            assert_eq!(vtf_decode::decode_highres(&vtf).unwrap().to_rgba8(), combined);
            checked += 1;
        }
        assert!(checked > 0, "The actual gun and stock must share at least one rendered VTF");
    }
}

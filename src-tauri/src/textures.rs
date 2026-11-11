use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Write};

const MAX_BYTES: usize = 96 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextureInput {
    name: String,
    vmt_path: String,
    vtf_path: String,
    vmt_text: String,
    png: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTexture {
    version: u32,
    id: String,
    name: String,
    #[serde(default)]
    name_is_custom: bool,
    vmt_path: String,
    vtf_path: String,
    vmt_text: String,
    vtf: String,
    preview: String,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureVariant {
    id: String,
    name: String,
    vmt_path: String,
    vtf_path: String,
    vmt_text: String,
    width: u32,
    height: u32,
    png: String,
    preview: String,
    has_alpha: bool,
}

#[derive(Serialize)]
pub struct TextureLibrary {
    variants: Vec<TextureVariant>,
    warnings: Vec<String>,
}

static DISCOVERY_LOCK: Mutex<()> = Mutex::new(());

fn storage(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|error| error.to_string())?.join("textures"))
}

fn texture_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid texture variant ID".into());
    }
    Ok(root.join(format!("{id}.json")))
}

fn material_path(path: &str, extension: &str) -> Result<String, String> {
    let path = key(path);
    if !path.starts_with("materials/") || !path.ends_with(extension) || path.contains(':')
        || path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("Texture variants require safe materials paths for both VMT and VTF".into());
    }
    Ok(path)
}

fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    let encoded = data.split_once(',').map(|(_, data)| data).unwrap_or(data);
    if encoded.len() > MAX_BYTES * 4 / 3 + 4 { return Err("Texture exceeds 96 MiB".into()); }
    let bytes = STANDARD.decode(encoded).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_BYTES { return Err("Texture exceeds 96 MiB".into()); }
    Ok(bytes)
}

fn valid_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION
        || !width.is_power_of_two() || !height.is_power_of_two() {
        return Err("Saved VMT/VTF variants require power-of-two dimensions up to 4096 x 4096".into());
    }
    Ok(())
}

pub(super) fn decode_texture(bytes: &[u8]) -> Result<DynamicImage, String> {
    if bytes.len() > MAX_BYTES { return Err("Texture exceeds 96 MiB".into()); }
    let image = if bytes.starts_with(b"VTF\0") {
        if bytes.len() < 65 { return Err("Truncated VTF header".into()); }
        let u16_at = |offset| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        valid_dimensions(u16_at(16) as u32, u16_at(18) as u32)?;
        let flags = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        if u16_at(24) != 1 || u16_at(63) > 1 || flags & 0x4000 != 0
            || bytes[56] == 0 || bytes[56] > 13 {
            return Err("Only single-frame, 2D VTF textures are supported as variants".into());
        }
        vtf_decode::decode_highres(bytes)?
    } else if bytes.starts_with(b"DDS ") {
        if bytes.len() < 128 { return Err("Truncated DDS header".into()); }
        valid_dimensions(u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()))?;
        decode_dds(bytes)?
    } else {
        let mut reader = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_DIMENSION);
        limits.max_image_height = Some(MAX_DIMENSION);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().map_err(|error| format!("Invalid PNG: {error}"))?
    };
    valid_dimensions(image.width(), image.height())?;
    Ok(image)
}

fn png(image: &DynamicImage) -> Result<String, String> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).map_err(|error| error.to_string())?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes.into_inner())))
}

fn material_script(text: &str, vtf_path: &str) -> Result<String, String> {
    if text.len() > 128 * 1024 { return Err("VMT exceeds 128 KiB".into()); }
    if !Regex::new(r#"(?i)^\s*"?[a-z][a-z0-9_]*"?\s*\{"#).unwrap().is_match(text)
        || !text.trim_end().ends_with('}') {
        return Err("A variant requires a complete VMT material with an explicit $basetexture".into());
    }
    let expression = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+"?([^"\r\n]+)"?"#).unwrap();
    if expression.captures_iter(text).count() != 1 {
        return Err("VMT must contain exactly one $basetexture".into());
    }
    let reference = vtf_path.trim_start_matches("materials/").trim_end_matches(".vtf");
    Ok(expression.replace(text, |_: &regex::Captures<'_>|
        format!("\"$basetexture\" \"{reference}\"")).into_owned())
}

fn texture_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err("Texture name must contain 1-200 printable characters".into());
    }
    Ok(name.to_owned())
}

fn prepare(input: TextureInput) -> Result<StoredTexture, String> {
    let name = texture_name(&input.name)?;
    let vmt_path = material_path(&input.vmt_path, ".vmt")?;
    let vtf_path = material_path(&input.vtf_path, ".vtf")?;
    let vmt_text = material_script(&input.vmt_text, &vtf_path)?;
    let image = decode_texture(&decode_base64(&input.png)?)?;
    let vtf = encode_vtf(&image)?;
    let mut hash = Sha256::new();
    for data in [vmt_path.as_bytes(), vtf_path.as_bytes(), vmt_text.as_bytes(), &vtf] {
        hash.update((data.len() as u64).to_le_bytes());
        hash.update(data);
    }
    let id = format!("{:x}", hash.finalize());
    Ok(StoredTexture {version:1, id, name, name_is_custom:false, vmt_path, vtf_path, vmt_text,
        vtf:STANDARD.encode(vtf), preview:png(&image.thumbnail(160, 160))?,
        width:image.width(), height:image.height()})
}

fn read_at(root: &Path, id: &str) -> Result<StoredTexture, String> {
    let bytes = fs::read(texture_path(root, id)?).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_BYTES * 2 { return Err("Saved variant is too large".into()); }
    let texture: StoredTexture = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if texture.version != 1 || texture.id != id { return Err("Invalid saved texture version or ID".into()); }
    material_path(&texture.vmt_path, ".vmt")?;
    material_path(&texture.vtf_path, ".vtf")?;
    valid_dimensions(texture.width, texture.height)?;
    Ok(texture)
}

fn view(texture: StoredTexture, full: bool) -> Result<TextureVariant, String> {
    let decoded = if full {
        let image = decode_texture(&decode_base64(&texture.vtf)?)?;
        if image.dimensions() != (texture.width, texture.height) { return Err("Saved texture dimensions do not match its VTF".into()); }
        image
    } else {
        image::load_from_memory_with_format(&decode_base64(&texture.preview)?, ImageFormat::Png)
            .map_err(|error| format!("Saved texture preview could not be decoded: {error}"))?
    };
    let has_alpha = decoded.to_rgba8().pixels().any(|pixel| pixel[3] < 255);
    let image = if full { png(&decoded)? } else { texture.preview };
    let mut opaque = decoded.to_rgba8();
    for pixel in opaque.pixels_mut() { pixel[3] = 255; }
    let preview = png(&DynamicImage::ImageRgba8(opaque))?;
    Ok(TextureVariant {id:texture.id, name:texture.name, vmt_path:texture.vmt_path,
        vtf_path:texture.vtf_path, vmt_text:texture.vmt_text, width:texture.width, height:texture.height,
        png:image, preview, has_alpha})
}

fn save_at(root: &Path, texture: StoredTexture) -> Result<TextureVariant, String> {
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let target = texture_path(root, &texture.id)?;
    if target.exists() {
        let existing = read_at(root, &texture.id)?;
        if existing.name_is_custom && !texture.name_is_custom { return view(existing, true); }
        if existing.name == texture.name && existing.name_is_custom == texture.name_is_custom { return view(existing, true); }
    }
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
    temp.write_all(&serde_json::to_vec(&texture).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    temp.as_file().sync_all().map_err(|error| error.to_string())?;
    temp.persist(&target).map_err(|error| error.to_string())?;
    view(texture, true)
}

pub(super) fn draft_variant_ids(app: &tauri::AppHandle, draft: &drafts::Draft) -> Result<BTreeMap<String, String>, String> {
    let root = storage(app)?;
    let mut ids = BTreeMap::new();
    for texture in &draft.textures {
        let (Some(vmt), Some(vtf), Some(text)) = (&texture.vmt_path, &texture.vtf_path, &texture.vmt_text) else { continue; };
        let prepared = prepare(TextureInput {name:"Draft texture".into(), vmt_path:vmt.clone(),
            vtf_path:vtf.clone(), vmt_text:text.clone(), png:texture.png.clone()})?;
        if texture_path(&root, &prepared.id)?.exists() { ids.insert(texture.key.clone(), prepared.id); }
    }
    Ok(ids)
}

pub(super) fn component_variant_ids(app: &tauri::AppHandle, index: &ContentIndex, draft: &drafts::Draft) -> Result<BTreeMap<String, String>, String> {
    let root = storage(app)?;
    let mut ids = BTreeMap::new();
    for component in &draft.component_textures {
        let detail = build_model(index, &component.part_path)?;
        let material = detail.materials.get(component.material_index).ok_or("Component material no longer exists")?;
        let (Some(vmt), Some(vtf), Some(text)) = (&material.vmt_path, &material.vtf_path, &material.vmt_text) else { continue; };
        let text = draft.textures.iter().find(|texture| texture.vmt_path.as_ref() == Some(vmt))
            .and_then(|texture| texture.vmt_text.as_ref()).unwrap_or(text);
        let prepared = prepare(TextureInput {name:"Component texture".into(), vmt_path:vmt.clone(),
            vtf_path:vtf.clone(), vmt_text:text.clone(), png:component.png.clone()})?;
        if texture_path(&root, &prepared.id)?.exists() {
            ids.insert(format!("{}:{}", component.part_path, component.material_index), prepared.id);
        } else if root.exists() {
            let pixels = decode_texture(&decode_base64(&component.png)?)?.to_rgba8();
            let mut candidates = fs::read_dir(&root).map_err(|error| error.to_string())?
                .map(|entry| entry.map(|entry| entry.path())).collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
            candidates.sort();
            for path in candidates {
                if path.extension().and_then(|value| value.to_str()) != Some("json") { continue; }
                let id = path.file_stem().and_then(|value| value.to_str()).ok_or("Invalid texture filename")?;
                let candidate = read_at(&root, id)?;
                if candidate.vmt_path == *vmt && candidate.vtf_path == *vtf
                    && decode_texture(&decode_base64(&candidate.vtf)?)?.to_rgba8() == pixels {
                    ids.insert(format!("{}:{}", component.part_path, component.material_index), candidate.id);
                    break;
                }
            }
        }
    }
    Ok(ids)
}

pub(super) fn import_draft_textures(app: &tauri::AppHandle, index: &ContentIndex, draft: &drafts::Draft) -> Result<(), String> {
    import_draft_textures_at(&storage(app)?, index, draft)
}

fn import_draft_textures_at(root: &Path, index: &ContentIndex, draft: &drafts::Draft) -> Result<(), String> {
    let output = shared_texture::flatten_draft(index, draft)?;
    let mut inputs = Vec::new();
    for texture in &output.textures {
        let (Some(vmt), Some(vtf), Some(text)) = (&texture.vmt_path, &texture.vtf_path, &texture.vmt_text) else { continue; };
        inputs.push(TextureInput {name:format!("{} - {}", draft.name, vmt.rsplit('/').next().unwrap_or(vmt)).chars().take(100).collect(),
            vmt_path:vmt.clone(), vtf_path:vtf.clone(), vmt_text:text.clone(), png:texture.png.clone()});
    }
    for material in draft.archived_files.iter().filter(|file| file.root == "vmt") {
        let text = String::from_utf8(decode_base64(&material.bytes)?).map_err(|error| error.to_string())?;
        let path = texture_reference(&text)?;
        let file = draft.archived_files.iter().find(|file| file.root == "vtf"
            && format!("materials/{}", key(&file.relative)) == path)
            .ok_or("Imported material has no matching archived VTF")?;
        inputs.push(paired_input(index, draft.name.clone(), format!("materials/{}", key(&material.relative)),
            text, &decode_base64(&file.bytes)?)?);
    }
    let prepared = inputs.into_iter().map(prepare).collect::<Result<Vec<_>, _>>()?;
    for texture in prepared {
        if !is_original(index, &texture)? && !texture_path(root, &texture.id)?.exists() {
            save_at(root, texture)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn save_texture_variant(texture: TextureInput, app: tauri::AppHandle) -> Result<TextureVariant, String> {
    let mut prepared = prepare(texture)?;
    prepared.name_is_custom = true;
    save_at(&storage(&app)?, prepared)
}

fn rename_at(root: &Path, id: &str, name: &str) -> Result<TextureVariant, String> {
    let name = texture_name(name)?;
    let mut texture = read_at(root, id)?;
    texture.name = name;
    texture.name_is_custom = true;
    save_at(root, texture)
}

#[tauri::command]
pub fn rename_texture_variant(id: String, name: String, app: tauri::AppHandle) -> Result<TextureVariant, String> {
    rename_at(&storage(&app)?, &id, &name)
}

fn is_original(index: &ContentIndex, texture: &StoredTexture) -> Result<bool, String> {
    let (Some(vmt), Some(vtf)) = (find(index, &texture.vmt_path), find(index, &texture.vtf_path)) else { return Ok(false); };
    let text = String::from_utf8(vmt.read()?).map_err(|error| error.to_string())?;
    if material_script(&text, &texture.vtf_path)? != texture.vmt_text { return Ok(false); }
    Ok(vtf_decode::decode_highres(&vtf.read()?)?.to_rgba8()
        == decode_texture(&decode_base64(&texture.vtf)?)?.to_rgba8())
}

#[tauri::command]
pub fn list_texture_variants(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<TextureLibrary, String> {
    let _guard = DISCOVERY_LOCK.lock().map_err(|_| "Texture discovery lock failed")?;
    let index = state.0.lock().map_err(|_| "Index lock failed")?;
    discover_at(&storage(&app)?, &index)
}

fn texture_reference(text: &str) -> Result<String, String> {
    let expression = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+(?:"([^"\r\n]+)"|([^"\s]+))"#).unwrap();
    let capture = expression.captures(text).ok_or("VMT has no explicit $basetexture")?;
    let reference = capture.get(1).or_else(|| capture.get(2)).ok_or("VMT has no texture reference")?.as_str();
    material_path(&base_texture_path(reference), ".vtf")
}

fn paired_input(index: &ContentIndex, name: String, vmt_path: String, text: String, bytes: &[u8]) -> Result<TextureInput, String> {
    let vtf_path = if let Some(original) = find(index, &vmt_path) {
        let original = String::from_utf8(original.read()?).map_err(|error| error.to_string())?;
        texture_reference(&original)?
    } else {
        texture_reference(&text)?
    };
    Ok(TextureInput {name:name.chars().take(100).collect(), vmt_path, vtf_path,
        vmt_text:text, png:png(&decode_texture(bytes)?)?})
}

fn discover_one(
    root: &Path, index: &ContentIndex, seen: &mut BTreeMap<String, String>, source: String, signature: String,
    input: impl FnOnce() -> Result<TextureInput, String>,
) -> Result<(), String> {
    if seen.get(&source) == Some(&signature) { return Ok(()); }
    let texture = prepare(input()?)?;
    if !is_original(index, &texture)? && !texture_path(root, &texture.id)?.exists() { save_at(root, texture)?; }
    seen.insert(source, signature);
    Ok(())
}

fn signature(parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    format!("{:x}", hash.finalize())
}

fn pair_signature(index: &ContentIndex, vmt_path: &str, text: &str, bytes: &[u8]) -> Result<String, String> {
    let original = find(index, vmt_path).map(Source::read).transpose()?;
    Ok(signature(&[vmt_path.as_bytes(), text.as_bytes(), bytes, original.as_deref().unwrap_or_default()]))
}

fn discover_at(root: &Path, index: &ContentIndex) -> Result<TextureLibrary, String> {
    let manifest = root.join("discovered-sources");
    let mut seen: BTreeMap<String, String> = if manifest.exists() {
        serde_json::from_slice(&fs::read(&manifest).map_err(|error| error.to_string())?)
            .map_err(|error| format!("Texture discovery history could not be read: {error}"))?
    } else { BTreeMap::new() };
    let before = seen.clone();
    let mut warnings = Vec::new();
    if let Some(install) = &index.install_root {
        for skin in custom_skin_inventory(install) {
            let result = (|| {
                let vmt = skin.vmt_path.as_deref().ok_or("VMT missing; texture cannot be paired")?;
                let vtf = skin.vtf_path.as_deref().ok_or("VTF reference missing")?;
                let text = fs::read_to_string(vmt).map_err(|error| error.to_string())?;
                let bytes = fs::read(vtf).map_err(|error| error.to_string())?;
                let path = Path::new(vmt).strip_prefix(install.join("data").join("phcustomtextures"))
                    .map_err(|error| error.to_string())?;
                let vmt_path = format!("materials/{}", key(&path.to_string_lossy()));
                let fingerprint = pair_signature(index, &vmt_path, &text, &bytes)?;
                discover_one(root, index, &mut seen, format!("installed:{}:{}", install.display(), skin.id), fingerprint,
                    || paired_input(index, format!("Installed - {}", skin.name), vmt_path, text, &bytes))
            })();
            if let Err(error) = result { warnings.push(format!("Installed {}: {error}", skin.name)); }
        }
    }
    if seen != before {
        fs::create_dir_all(root).map_err(|error| error.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
        temp.write_all(&serde_json::to_vec(&seen).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
        temp.as_file().sync_all().map_err(|error| error.to_string())?;
        temp.persist(&manifest).map_err(|error| error.to_string())?;
    }
    let mut variants = Vec::new();
    if root.exists() {
        for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if !path.extension().is_some_and(|extension| extension == "json") { continue; }
            let id = path.file_stem().and_then(|id| id.to_str()).ok_or("Invalid texture filename")?;
            let texture = read_at(root, id)?;
            match is_original(index, &texture) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => warnings.push(format!("Default comparison for {} failed: {error}", texture.name)),
            }
            variants.push(view(texture, false)?);
        }
    }
    variants.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(TextureLibrary {variants, warnings})
}

#[cfg(test)]
fn list_at(root: &Path) -> Result<Vec<TextureVariant>, String> {
    if !root.exists() { return Ok(Vec::new()); }
    let mut variants = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().is_some_and(|extension| extension == "json") {
            let id = path.file_stem().and_then(|id| id.to_str()).ok_or("Invalid texture filename")?;
            variants.push(view(read_at(root, id)?, false)?);
        }
    }
    variants.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(variants)
}

#[tauri::command]
pub fn load_texture_variant(id: String, app: tauri::AppHandle) -> Result<TextureVariant, String> {
    view(read_at(&storage(&app)?, &id)?, true)
}

#[tauri::command]
pub fn delete_texture_variant(id: String, app: tauri::AppHandle) -> Result<(), String> {
    fs::remove_file(texture_path(&storage(&app)?, &id)?).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(color: [u8; 4]) -> TextureInput {
        TextureInput {name:"Suppressor blue".into(),
            vmt_path:"materials/models/weapons/v_models/test/suppressor.vmt".into(),
            vtf_path:"materials/models/weapons/v_models/test/suppressor.vtf".into(),
            vmt_text:"VertexLitGeneric\n{\n\"$basetexture\" \"old/path_custom\"\n\"$phong\" \"1\"\n}".into(),
            png:png(&DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 4, image::Rgba(color)))).unwrap()}
    }

    #[test]
    fn imported_drafts_add_pairs_once_and_keep_custom_library_names() {
        let library = tempfile::tempdir().unwrap();
        let input = sample([20, 60, 200, 80]);
        let draft = drafts::Draft {version:1, id:None, name:"Downloaded skin".into(),
            model_path:"models/perpheads/weapons/c_ak101.mdl".into(), slot_selections:BTreeMap::new(),
            active_part_path:"models/perpheads/weapons/c_ak101.mdl".into(), active_material_key:Some(input.vmt_path.clone()),
            textures:vec![drafts::DraftTexture {key:input.vmt_path.clone(),
                part_path:"models/perpheads/weapons/c_ak101.mdl".into(), vmt_path:Some(input.vmt_path.clone()),
                vtf_path:Some(input.vtf_path.clone()), vmt_text:Some(input.vmt_text.clone()), png:input.png.clone(), strip_alpha:false}],
            component_textures:Vec::new(), component_error:None, ai_originals:Vec::new(), workshop_source:None, archived_files:Vec::new()};
        let index = ContentIndex::default();
        import_draft_textures_at(library.path(), &index, &draft).unwrap();
        let variants = list_at(library.path()).unwrap();
        assert_eq!(variants.len(), 1);
        assert_eq!(view(read_at(library.path(), &variants[0].id).unwrap(), true).unwrap().png, input.png);
        rename_at(library.path(), &variants[0].id, "My chosen name").unwrap();
        import_draft_textures_at(library.path(), &index, &draft).unwrap();
        assert_eq!(list_at(library.path()).unwrap().len(), 1);
        assert_eq!(list_at(library.path()).unwrap()[0].name, "My chosen name");
        let mut archived = draft.clone();
        archived.textures.clear();
        archived.archived_files = vec![
            drafts::SavedFile {root:"vmt".into(), relative:input.vmt_path.trim_start_matches("materials/").into(),
                bytes:STANDARD.encode(variants[0].vmt_text.as_bytes())},
            drafts::SavedFile {root:"vtf".into(), relative:input.vtf_path.trim_start_matches("materials/").into(),
                bytes:STANDARD.encode(encode_vtf(&decode_texture(&decode_base64(&input.png).unwrap()).unwrap()).unwrap())},
        ];
        import_draft_textures_at(library.path(), &index, &archived).unwrap();
        assert_eq!(list_at(library.path()).unwrap().len(), 1);
    }

    #[test]
    fn variants_persist_pairs_deduplicate_and_delete_independently() {
        let root = tempfile::tempdir().unwrap();
        let saved = save_at(root.path(), prepare(sample([20, 60, 200, 80])).unwrap()).unwrap();
        assert_eq!((saved.width, saved.height), (8, 4));
        assert!(saved.vmt_text.contains("$phong"));
        assert!(saved.vmt_text.contains("test/suppressor\""));
        let record = read_at(root.path(), &saved.id).unwrap();
        assert_eq!(decode_texture(&decode_base64(&record.vtf).unwrap()).unwrap().to_rgba8().get_pixel(0, 0).0, [20, 60, 200, 80]);
        let mut renamed = sample([20, 60, 200, 80]);
        renamed.name = "Different file name".into();
        assert_eq!(save_at(root.path(), prepare(renamed).unwrap()).unwrap().id, saved.id);
        let red = save_at(root.path(), prepare(sample([200, 20, 30, 255])).unwrap()).unwrap();
        assert_ne!(saved.id, red.id);
        assert_eq!(list_at(root.path()).unwrap().len(), 2);
        fs::remove_file(texture_path(root.path(), &saved.id).unwrap()).unwrap();
        assert_eq!(list_at(root.path()).unwrap().len(), 1);
        assert!(read_at(root.path(), &saved.id).is_err());
        assert!(texture_path(root.path(), "../drafts/test").is_err());
    }

    #[test]
    fn renaming_changes_only_the_label_and_survives_automatic_discovery() {
        let root = tempfile::tempdir().unwrap();
        let saved = save_at(root.path(), prepare(sample([20, 60, 200, 80])).unwrap()).unwrap();
        let before = serde_json::to_value(read_at(root.path(), &saved.id).unwrap()).unwrap();
        let renamed = rename_at(root.path(), &saved.id, "  Blue suppressor  ").unwrap();
        assert_eq!(renamed.name, "Blue suppressor");
        assert_eq!(renamed.id, saved.id);
        assert_eq!(renamed.png, saved.png);
        let mut after = serde_json::to_value(read_at(root.path(), &saved.id).unwrap()).unwrap();
        after["name"] = before["name"].clone();
        after["nameIsCustom"] = before["nameIsCustom"].clone();
        assert_eq!(before, after);
        save_at(root.path(), prepare(sample([20, 60, 200, 80])).unwrap()).unwrap();
        assert_eq!(list_at(root.path()).unwrap()[0].name, "Blue suppressor");
        for invalid in ["".into(), "   ".into(), "x".repeat(201), "bad\nname".into()] {
            assert!(rename_at(root.path(), &saved.id, &invalid).is_err());
        }
        assert!(rename_at(root.path(), "../unsafe", "Name").is_err());
        assert!(rename_at(root.path(), &"a".repeat(64), "Name").is_err());
        assert_eq!(read_at(root.path(), &saved.id).unwrap().name, "Blue suppressor");
    }

    #[test]
    fn variants_reject_invalid_materials_dimensions_and_texture_headers() {
        let mut input = sample([1, 2, 3, 255]);
        input.vtf_path = "materials/../unsafe.vtf".into();
        assert!(prepare(input).is_err());
        let mut input = sample([1, 2, 3, 255]);
        input.vmt_text = "Patch { }".into();
        assert!(prepare(input).is_err());
        let mut input = sample([1, 2, 3, 255]);
        input.vmt_text = "VertexLitGeneric\n{\n$basetexture a\n$basetexture b\n}".into();
        assert!(prepare(input).is_err());
        assert!(decode_texture(b"VTF\0").is_err());
        let mut vtf = encode_vtf(&DynamicImage::new_rgba8(8, 4)).unwrap();
        vtf[16..18].copy_from_slice(&8192_u16.to_le_bytes());
        assert!(decode_texture(&vtf).is_err());
        assert!(valid_dimensions(7, 4).is_err());
    }

    #[test]
    fn variants_decode_png_dds_vtf_and_retain_material_variations() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 4, image::Rgba([32, 90, 180, 77])));
        let png_bytes = decode_base64(&png(&image).unwrap()).unwrap();
        for bytes in [png_bytes, encode_dds(&image), encode_vtf(&image).unwrap()] {
            let decoded = import_image(STANDARD.encode(&bytes)).unwrap();
            assert_eq!(decode_texture(&decode_base64(&decoded).unwrap()).unwrap().to_rgba8(), image.to_rgba8());
        }
        let lit = prepare(sample([1, 2, 3, 255])).unwrap();
        let mut unlit = sample([1, 2, 3, 255]);
        unlit.vmt_text = unlit.vmt_text.replace("VertexLitGeneric", "UnlitGeneric");
        assert_ne!(prepare(unlit).unwrap().id, lit.id);
        let mut other_material = sample([1, 2, 3, 255]);
        other_material.vmt_path = other_material.vmt_path.replace("suppressor.vmt", "body.vmt");
        assert_ne!(prepare(other_material).unwrap().id, lit.id);
    }

    #[test]
    fn variant_previews_ignore_alpha_without_changing_pixels_or_creating_copies() {
        let root = tempfile::tempdir().unwrap();
        let input = sample([32, 90, 180, 0]);
        let saved = save_at(root.path(), prepare(input).unwrap()).unwrap();
        for variant in [view(read_at(root.path(), &saved.id).unwrap(), true).unwrap(),
            list_at(root.path()).unwrap().remove(0)] {
            assert!(variant.has_alpha);
            let original = image::load_from_memory(&decode_base64(&variant.png).unwrap()).unwrap().to_rgba8();
            let preview = image::load_from_memory(&decode_base64(&variant.preview).unwrap()).unwrap().to_rgba8();
            assert_eq!(original.get_pixel(0, 0).0, [32, 90, 180, 0]);
            assert_eq!(preview.get_pixel(0, 0).0, [32, 90, 180, 255]);
        }
        let restored = prepare(TextureInput {name:saved.name, vmt_path:saved.vmt_path,
            vtf_path:saved.vtf_path, vmt_text:saved.vmt_text, png:saved.png}).unwrap();
        assert_eq!(restored.id, saved.id, "Applying and resaving a variant must reuse its identity");
        save_at(root.path(), restored).unwrap();
        assert_eq!(list_at(root.path()).unwrap().len(), 1);
        save_at(root.path(), prepare(sample([32, 90, 180, 255])).unwrap()).unwrap();
        assert_eq!(list_at(root.path()).unwrap().len(), 2, "Different alpha data must remain distinct");
    }

    #[test]
    fn reused_variant_identity_is_shared_and_defaults_are_not_listed() {
        let library = tempfile::tempdir().unwrap();
        let drafts = tempfile::tempdir().unwrap();
        let sources = tempfile::tempdir().unwrap();
        let input = sample([20, 40, 200, 255]);
        let saved = save_at(library.path(), prepare(input).unwrap()).unwrap();
        let original_image = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(8, 4,
            |x, y| image::Rgba([x as u8 * 20, y as u8 * 40, 70, 255])));
        let original_vmt = sources.path().join("original.vmt");
        let original_vtf = sources.path().join("original.vtf");
        fs::write(&original_vmt, &saved.vmt_text).unwrap();
        fs::write(&original_vtf, encode_vtf(&original_image).unwrap()).unwrap();
        let mut index = ContentIndex::default();
        index.files.insert(saved.vmt_path.clone(), Source::Loose(original_vmt));
        index.files.insert(saved.vtf_path.clone(), Source::Loose(original_vtf));
        let draft_texture = drafts::DraftTexture {key:saved.vmt_path.clone(),
            part_path:"models/perpheads/weapons/c_ak101.mdl".into(),
            vmt_path:Some(saved.vmt_path.clone()), vtf_path:Some(saved.vtf_path.clone()),
            vmt_text:Some(saved.vmt_text.clone()), png:saved.png.clone(), strip_alpha:false};
        for id in ["first", "second"] {
            let draft = drafts::Draft {version:1, id:Some(id.into()), name:format!("{id} draft"),
                model_path:"models/perpheads/weapons/c_ak101.mdl".into(), slot_selections:BTreeMap::new(),
                active_part_path:"models/perpheads/weapons/c_ak101.mdl".into(),
                active_material_key:Some(saved.vmt_path.clone()), textures:vec![draft_texture.clone()],
                component_textures:Vec::new(), component_error:None, ai_originals:Vec::new(), workshop_source:None, archived_files:Vec::new()};
            fs::write(drafts.path().join(format!("{id}.json")), serde_json::to_vec(&draft).unwrap()).unwrap();
        }
        for _ in 0..3 {
            let result = discover_at(library.path(), &index).unwrap();
            assert!(result.warnings.is_empty(), "{:?}", result.warnings);
            assert_eq!(result.variants.len(), 1);
            assert_eq!(result.variants[0].id, saved.id);
        }
        let baseline = TextureInput {name:"Default copied into old library".into(), vmt_path:saved.vmt_path,
            vtf_path:saved.vtf_path, vmt_text:saved.vmt_text, png:png(&original_image).unwrap()};
        let baseline = save_at(library.path(), prepare(baseline).unwrap()).unwrap();
        let result = discover_at(library.path(), &index).unwrap();
        assert_eq!(result.variants.len(), 1, "Original / default must not also appear as a named variant");
        assert!(texture_path(library.path(), &baseline.id).unwrap().exists(), "Do not destroy existing library files");
        let loaded = view(read_at(library.path(), &saved.id).unwrap(), true).unwrap();
        let reused = save_at(library.path(), prepare(TextureInput {name:"Third draft".into(),
            vmt_path:loaded.vmt_path, vtf_path:loaded.vtf_path, vmt_text:loaded.vmt_text, png:loaded.png}).unwrap()).unwrap();
        assert_eq!(reused.id, saved.id);
        assert_eq!(discover_at(library.path(), &index).unwrap().variants.len(), 1);
    }

    #[test]
    fn discovery_backfills_installed_pairs_but_never_draft_textures() {
        let library = tempfile::tempdir().unwrap();
        let saved = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        fs::create_dir(game.path().join("addons")).unwrap();
        fs::create_dir(game.path().join("data")).unwrap();
        let input = sample([20, 40, 200, 255]);
        let draft_id = "old-draft";
        let draft = drafts::Draft {version:1, id:Some(draft_id.into()), name:"Existing draft".into(),
            model_path:"models/perpheads/weapons/c_ak101.mdl".into(),
            slot_selections:BTreeMap::new(), active_part_path:"models/perpheads/weapons/c_ak101.mdl".into(),
            active_material_key:Some(input.vmt_path.clone()),
            textures:vec![drafts::DraftTexture {key:input.vmt_path.clone(),
                part_path:"models/perpheads/weapons/c_ak101.mdl".into(),
                vmt_path:Some(input.vmt_path.clone()), vtf_path:Some(input.vtf_path.clone()),
                vmt_text:Some(input.vmt_text.clone()), png:input.png.clone(), strip_alpha:false}],
            component_textures:Vec::new(), component_error:None, ai_originals:Vec::new(), workshop_source:None, archived_files:Vec::new()};
        let draft_file = saved.path().join(format!("{draft_id}.json"));
        let draft_bytes = serde_json::to_vec(&draft).unwrap();
        fs::write(&draft_file, &draft_bytes).unwrap();
        let installed_image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 4, image::Rgba([200, 20, 40, 255])));
        write_custom_skin(game.path(), &input.vtf_path, &input.vmt_path, &input.vmt_text,
            &encode_vtf(&installed_image).unwrap()).unwrap();
        let inventory = custom_skin_inventory(game.path());
        let installed_vmt = inventory[0].vmt_path.as_ref().unwrap();
        let installed_vtf = inventory[0].vtf_path.as_ref().unwrap();
        let vmt_before = fs::read(installed_vmt).unwrap();
        let vtf_before = fs::read(installed_vtf).unwrap();
        let original = game.path().join("original.vmt");
        fs::write(&original, format!("VertexLitGeneric\n{{\n$basetexture \"{}\"\n}}",
            input.vtf_path.trim_start_matches("materials/").trim_end_matches(".vtf"))).unwrap();
        let mut index = ContentIndex {install_root:Some(game.path().to_path_buf()), ..ContentIndex::default()};
        index.files.insert(input.vmt_path.clone(), Source::Loose(original));
        let found = discover_at(library.path(), &index).unwrap();
        assert!(found.warnings.is_empty(), "{:?}", found.warnings);
        assert_eq!(found.variants.len(), 1);
        assert!(found.variants.iter().all(|variant| variant.vtf_path == input.vtf_path));
        assert!(found.variants.iter().any(|variant| variant.name.starts_with("Installed")));
        let removed = &found.variants[0].id;
        fs::remove_file(texture_path(library.path(), removed).unwrap()).unwrap();
        let again = discover_at(library.path(), &index).unwrap();
        assert!(again.variants.is_empty(), "Deleted installed variants must not reappear on refresh");
        assert_eq!(fs::read(&draft_file).unwrap(), draft_bytes);
        assert_eq!(fs::read(installed_vmt).unwrap(), vmt_before);
        assert_eq!(fs::read(installed_vtf).unwrap(), vtf_before);
        fs::write(installed_vtf, b"broken VTF").unwrap();
        fs::write(saved.path().join("broken.json"), b"broken draft").unwrap();
        let partial = discover_at(library.path(), &index).unwrap();
        assert!(partial.variants.is_empty());
        assert_eq!(partial.warnings.len(), 1, "{:?}", partial.warnings);
    }
}

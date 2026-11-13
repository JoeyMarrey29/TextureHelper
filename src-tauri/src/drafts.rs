use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::{Read, Write}, time::{SystemTime, UNIX_EPOCH}};

const MAX_SHARE_BYTES: u64 = 256 * 1024 * 1024;
const SHARE_FORMAT: &str = "perpheads-texture-helper-skin";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkinPackage {
    format: String,
    version: u32,
    draft: Draft,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftTexture {
    pub key: String,
    pub part_path: String,
    pub vmt_path: Option<String>,
    pub vtf_path: Option<String>,
    pub vmt_text: Option<String>,
    pub png: String,
    #[serde(default)]
    pub strip_alpha: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedFile {
    pub(super) root: String,
    pub(super) relative: String,
    pub(super) bytes: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiOriginal {
    pub id: String,
    pub label: String,
    pub png: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkshopSource {
    pub id: String,
    pub steam_id: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_steam_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub version: u32,
    pub id: Option<String>,
    pub name: String,
    pub model_path: String,
    pub slot_selections: BTreeMap<String, Option<String>>,
    pub active_part_path: String,
    pub active_material_key: Option<String>,
    pub textures: Vec<DraftTexture>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_textures: Vec<shared_texture::ComponentTexture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_originals: Vec<AiOriginal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_source: Option<WorkshopSource>,
    #[serde(default)]
    pub(super) archived_files: Vec<SavedFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSummary {
    id: String,
    name: String,
    model_path: String,
    texture_count: usize,
    installed_count: usize,
    installable_count: usize,
    archived: bool,
    installed_ids: Vec<String>,
    component_error: Option<String>,
}

pub(super) fn storage(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|error| error.to_string())?.join("drafts"))
}

fn draft_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err("Invalid draft ID".into());
    }
    Ok(root.join(format!("{id}.json")))
}

fn png_bytes(png: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(png.strip_prefix("data:image/png;base64,").unwrap_or(png))
        .map_err(|error| error.to_string())
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty() && !path.contains('\\') && !path.contains(':')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn file_target(install: &Path, file: &SavedFile) -> Result<PathBuf, String> {
    if !safe_relative(&file.relative) {
        return Err("Invalid saved skin path".into());
    }
    let root = match file.root.as_str() {
        "vmt" => install.join("data").join("phcustomtextures"),
        "vtf" => install.join("addons").join("skins").join("materials"),
        _ => return Err("Invalid saved skin folder".into()),
    };
    Ok(root.join(&file.relative))
}

fn validate(draft: &Draft) -> Result<(), String> {
    if draft.version != 1 {
        return Err(format!("Unsupported draft version {}", draft.version));
    }
    if draft.name.trim().is_empty() {
        return Err("Draft name cannot be empty".into());
    }
    if let Some(source) = &draft.workshop_source {
        if source.id.len() != 32 || !source.id.bytes().all(|c| c.is_ascii_hexdigit())
            || source.steam_id.len() != 17 || !source.steam_id.bytes().all(|c| c.is_ascii_digit())
            || source.license != "CC-BY-4.0" {
            return Err("Invalid workshop attribution".into());
        }
        match (&source.parent, &source.parent_steam_id) {
            (None, None) => {}
            (Some(id), Some(steam_id)) if id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
                && steam_id.len() == 17 && steam_id.bytes().all(|c| c.is_ascii_digit()) => {}
            _ => return Err("Invalid workshop remix attribution".into()),
        }
    }
    if draft.ai_originals.len() > 24 { return Err("Too many AI originals in this draft".into()); }
    if draft.component_textures.len() > 128 || (!draft.component_textures.is_empty() && !draft.archived_files.is_empty()) {
        return Err("Invalid component texture count or archived component textures".into());
    }
    let mut components = std::collections::HashSet::new();
    let mut component_pixels = 0_u64;
    for component in &draft.component_textures {
        if !is_skin_target_model(&component.part_path) || !safe_relative(&component.part_path)
            || !components.insert((key(&component.part_path), component.material_index)) {
            return Err("Invalid or duplicate component texture".into());
        }
        let image = textures::decode_texture(&png_bytes(&component.png)?)?;
        component_pixels += image.width() as u64 * image.height() as u64;
        if component_pixels > 64 * 1024 * 1024 { return Err("Component sources exceed the 64-million-pixel working limit".into()); }
    }
    let mut ids = std::collections::HashSet::new();
    for original in &draft.ai_originals {
        if original.id.is_empty() || original.id.len() > 512 || original.label.len() > 1024 || !ids.insert(&original.id) {
            return Err("Invalid or duplicated AI original identifier".into());
        }
        let data = original.png.strip_prefix("data:image/png;base64,").unwrap_or(&original.png);
        if data.len() > 32 * 1024 * 1024 * 4 / 3 + 4 {
            return Err("Saved original exceeds 32 MiB".into());
        }
        let mut reader = image::ImageReader::with_format(Cursor::new(png_bytes(data)?), ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().map_err(|error| format!("Invalid saved original PNG: {error}"))?;
    }
    if draft.archived_files.is_empty() {
        if !safe_relative(&draft.model_path) || !draft.model_path.starts_with("models/")
            || !draft.model_path.ends_with(".mdl") || draft.textures.is_empty() {
            return Err("Draft requires a model and at least one edited texture".into());
        }
        let mut keys = std::collections::HashSet::new();
        for texture in &draft.textures {
            if !keys.insert(&texture.key) || texture.key.is_empty() {
                return Err("Draft has duplicate or empty material keys".into());
            }
            image::load_from_memory_with_format(&png_bytes(&texture.png)?, ImageFormat::Png)
                .map_err(|error| format!("Invalid draft PNG: {error}"))?;
        }
    } else {
        for file in &draft.archived_files {
            file_target(Path::new(""), file)?;
            STANDARD.decode(&file.bytes).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn component_source_id(png: &str) -> String {
    format!("{:x}", Sha256::digest(png.as_bytes()))
}

fn compact_component_sources(draft: &mut Draft) {
    let mut seen = std::collections::HashSet::new();
    for component in &mut draft.component_textures {
        let source_id = component_source_id(&component.png);
        component.source_id = Some(source_id.clone());
        if !seen.insert(source_id) { component.png.clear(); }
    }
}

fn expand_component_sources(draft: &mut Draft) -> Result<(), String> {
    let mut sources = BTreeMap::new();
    for component in &draft.component_textures {
        let Some(source_id) = &component.source_id else { continue; };
        if component.png.is_empty() { continue; }
        if component_source_id(&component.png) != *source_id {
            return Err("Draft component texture source checksum does not match".into());
        }
        if let Some(previous) = sources.insert(source_id.clone(), component.png.clone()) {
            if previous != component.png { return Err("Draft component texture source ID is ambiguous".into()); }
        }
    }
    for component in &mut draft.component_textures {
        let Some(source_id) = &component.source_id else { continue; };
        if component.png.is_empty() {
            component.png = sources.get(source_id).ok_or("Draft component texture source is missing")?.clone();
        }
        component.source_id = None;
    }
    Ok(())
}

pub(super) fn read_at(root: &Path, id: &str) -> Result<Draft, String> {
    let bytes = fs::read(draft_path(root, id)?).map_err(|error| error.to_string())?;
    let mut draft: Draft = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if draft.id.as_deref() != Some(id) {
        return Err("Draft ID does not match its filename".into());
    }
    expand_component_sources(&mut draft)?;
    validate(&draft)?;
    Ok(draft)
}

fn save_at(root: &Path, mut draft: Draft) -> Result<Draft, String> {
    validate(&draft)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    if draft.id.is_none() {
        draft.id = Some(format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?.as_nanos()));
    } else if !draft_path(root, draft.id.as_deref().unwrap())?.is_file() {
        return Err("Draft no longer exists; reopen or start a new draft".into());
    }
    let target = draft_path(root, draft.id.as_deref().unwrap())?;
    let mut compact = draft.clone();
    compact_component_sources(&mut compact);
    let bytes = serde_json::to_vec(&compact).map_err(|error| error.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
    temp.write_all(&bytes).map_err(|error| error.to_string())?;
    temp.as_file().sync_all().map_err(|error| error.to_string())?;
    temp.persist(target).map_err(|error| error.to_string())?;
    Ok(draft)
}

fn expected_files(texture: &DraftTexture) -> Result<Vec<SavedFile>, String> {
    let vmt = texture.vmt_path.as_deref().ok_or("Draft texture has no VMT")?;
    let vtf = texture.vtf_path.as_deref().ok_or("Draft texture has no VTF")?;
    if !is_skin_material_path(vmt) || !is_skin_material_path(vtf)
        || !safe_relative(vmt) || !safe_relative(vtf) || !vmt.ends_with(".vmt") || !vtf.ends_with(".vtf") {
        return Err("Draft texture is not a supported first-person skin".into());
    }
    let image = image::load_from_memory_with_format(&png_bytes(&texture.png)?, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    let reference = format!("{}_custom", vtf.trim_start_matches("materials/").trim_end_matches(".vtf"));
    let text = texture.vmt_text.as_deref().ok_or("Draft texture has no VMT text")?;
    let re = Regex::new(r#"(?im)^(\s*"?\$basetexture"?\s+)(?:"[^"\r\n]+"|[^"\s]+)"#).unwrap();
    if !re.is_match(text) {
        return Err("VMT has no $basetexture reference".into());
    }
    let rewritten = re.replace(text, |captures: &regex::Captures| format!("{}\"{}\"", &captures[1], reference));
    Ok(vec![
        SavedFile { root: "vmt".into(), relative: vmt.trim_start_matches("materials/").into(),
            bytes: STANDARD.encode(rewritten.as_bytes()) },
        SavedFile { root: "vtf".into(), relative: format!("{reference}.vtf"),
            bytes: STANDARD.encode(encode_skin_vtf(&image, texture.strip_alpha)?) },
    ])
}

fn supported(texture: &DraftTexture) -> bool {
    is_skin_target_model(&texture.part_path)
        && texture.vmt_path.as_deref().is_some_and(is_skin_material_path)
        && texture.vtf_path.as_deref().is_some_and(is_skin_material_path)
}

fn files_match(install: &Path, files: &[SavedFile]) -> Result<bool, String> {
    for file in files {
        let path = file_target(install, file)?;
        if !path.is_file() { return Ok(false); }
        let expected = STANDARD.decode(&file.bytes).map_err(|error| error.to_string())?;
        if fs::read(path).map_err(|error| error.to_string())? != expected { return Ok(false); }
    }
    Ok(true)
}

fn editable_file_matches(actual: &[u8], expected: &[u8], kind: &str) -> bool {
    if actual == expected { return true; }
    if kind == "vmt" {
        let (Ok(actual), Ok(expected)) = (std::str::from_utf8(actual), std::str::from_utf8(expected)) else { return false; };
        let re = Regex::new(r#"(?im)^(\s*"?\$basetexture"?\s+)(?:"([^"\r\n]+)"|([^"\s]+))"#).unwrap();
        let normalize = |text: &str| re.replace(text, |capture: &regex::Captures| {
            format!("{}\"{}\"", &capture[1], base_texture_path(capture.get(2).or_else(|| capture.get(3)).unwrap().as_str()))
        }).into_owned();
        return normalize(actual) == normalize(expected);
    }
    if kind != "vtf" { return false; }
    // Imported VTFs can use different compression/header flags but have identical pixels.
    match (vtf_decode::decode_highres(actual), vtf_decode::decode_highres(expected)) {
        (Ok(actual), Ok(expected)) => actual.to_rgba8() == expected.to_rgba8(),
        _ => false,
    }
}

fn editable_files_match(install: &Path, files: &[SavedFile]) -> Result<bool, String> {
    for file in files {
        let path = file_target(install, file)?;
        if !path.is_file() { return Ok(false); }
        let actual = fs::read(path).map_err(|error| error.to_string())?;
        let expected = STANDARD.decode(&file.bytes).map_err(|error| error.to_string())?;
        if !editable_file_matches(&actual, &expected, &file.root) { return Ok(false); }
    }
    Ok(true)
}

fn summary(draft: &Draft, install: Option<&Path>) -> Result<DraftSummary, String> {
    let archived = !draft.archived_files.is_empty();
    let (mut installed_count, mut installable_count) = (0, 0);
    let mut installed_ids = Vec::new();
    if archived {
        installable_count = 1;
        if let Some(root) = install {
            installed_count = usize::from(files_match(root, &draft.archived_files)?);
            if installed_count > 0 {
                let file = draft.archived_files.iter().find(|file| file.root == "vmt").unwrap_or(&draft.archived_files[0]);
                installed_ids.push(format!("{}:{}", file.root, file.relative));
            }
        }
    } else {
        for texture in draft.textures.iter().filter(|texture| supported(texture)) {
            installable_count += 1;
            if let Some(root) = install.filter(|_| draft.component_error.is_none()) {
                if editable_files_match(root, &expected_files(texture)?)? {
                    installed_count += 1;
                    installed_ids.push(format!("vmt:{}", texture.vmt_path.as_deref().unwrap().trim_start_matches("materials/")));
                }
            }
        }
    }
    Ok(DraftSummary { id: draft.id.clone().ok_or("Draft has no ID")?, name: draft.name.clone(),
        model_path: draft.model_path.clone(), texture_count: draft.textures.len(),
        installed_count, installable_count, archived, installed_ids, component_error:draft.component_error.clone() })
}

#[tauri::command]
pub fn save_draft(mut draft: Draft, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Draft, String> {
    if !draft.archived_files.is_empty() {
        return Err("Installed-skin backups cannot be edited as assembly drafts".into());
    }
    validate(&draft)?;
    if draft.component_textures.is_empty() {
        draft.component_error = None;
    } else {
        let previous = draft.id.as_ref().map(|id| read_at(&storage(&app)?, id)).transpose()?;
        if let Some(previous) = previous.filter(|previous| previous.component_textures == draft.component_textures
            && previous.textures == draft.textures && previous.model_path == draft.model_path
            && previous.slot_selections == draft.slot_selections) {
            // Selection/name-only autosaves reuse the atlas; every output independently recomposes and validates it.
            draft.component_error = previous.component_error;
        } else {
            let index = state.0.lock().map_err(|_| "Index lock failed")?;
            match shared_texture::flatten_draft(&index, &draft) {
                Ok(flattened) => { draft.textures = flattened.textures; draft.component_error = None; }
                Err(error) => draft.component_error = Some(error),
            }
        }
    }
    save_at(&storage(&app)?, draft)
}

#[tauri::command]
pub fn prepare_draft_output(id: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Draft, String> {
    let draft = read_at(&storage(&app)?, &id)?;
    shared_texture::flatten_draft(&*state.0.lock().map_err(|_| "Index lock failed")?, &draft)
}

#[tauri::command]
pub fn list_drafts(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Vec<DraftSummary>, String> {
    let root = storage(&app)?;
    if !root.exists() { return Ok(Vec::new()); }
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone();
    let mut summaries = Vec::new();
    for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.path().extension().is_some_and(|extension| extension == "json") {
            let path = entry.path();
            let id = path.file_stem().and_then(|stem| stem.to_str()).ok_or("Invalid draft filename")?;
            summaries.push(summary(&read_at(&root, id)?, install.as_deref())?);
        }
    }
    summaries.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(summaries)
}

#[tauri::command]
pub fn load_draft(id: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<LoadedDraft, String> {
    let draft = read_at(&storage(&app)?, &id)?;
    let (variant_ids, variant_warning) = match textures::draft_variant_ids(&app, &draft) {
        Ok(ids) => (ids, None),
        Err(error) => (BTreeMap::new(), Some(format!("Saved variant matching failed: {error}"))),
    };
    let (component_variant_ids, component_warning) = match textures::component_variant_ids(&app,
        &*state.0.lock().map_err(|_| "Index lock failed")?, &draft) {
        Ok(ids) => (ids, None),
        Err(error) => (BTreeMap::new(), Some(format!("Saved component variant matching failed: {error}"))),
    };
    let variant_warning = match (variant_warning, component_warning) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
        (a, b) => a.or(b),
    };
    Ok(LoadedDraft {draft, variant_ids, component_variant_ids, variant_warning})
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedDraft {
    #[serde(flatten)]
    draft: Draft,
    variant_ids: BTreeMap<String, String>,
    component_variant_ids: BTreeMap<String, String>,
    variant_warning: Option<String>,
}

#[tauri::command]
pub fn delete_draft(id: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone();
    delete_at(&storage(&app)?, install.as_deref(), &id)
}

fn delete_at(root: &Path, install: Option<&Path>, id: &str) -> Result<(), String> {
    let draft = read_at(root, id)?;
    if let Some(install) = install {
        let groups = if draft.archived_files.is_empty() {
            draft.textures.iter().filter(|texture| supported(texture))
                .map(expected_files).collect::<Result<Vec<_>, _>>()?
        } else { vec![draft.archived_files.clone()] };
        let mut files = BTreeMap::new();
        for file in groups.into_iter().flatten() {
            let target = file_target(install, &file)?;
            let bytes = STANDARD.decode(&file.bytes).map_err(|error| error.to_string())?;
            if target.is_file() && {
                let actual = fs::read(&target).map_err(|error| error.to_string())?;
                if draft.archived_files.is_empty() { editable_file_matches(&actual, &bytes, &file.root) }
                else { actual == bytes }
            } {
                files.insert((file.root, file.relative), target);
            }
        }
        // Remove matching materials first so shared textures remain until their last reference is gone.
        for ((kind, _), path) in files.iter().filter(|((kind, _), _)| kind == "vmt") {
            remove_in_root(path, &install.join("data").join("phcustomtextures"))
                .map_err(|error| format!("Could not remove installed {kind}; saved copy kept: {error}"))?;
        }
        let inventory = custom_skin_inventory(install);
        for ((_, _), path) in files.iter().filter(|((kind, _), _)| kind == "vtf") {
            if !inventory.iter().any(|skin| skin.vmt_path.is_some()
                && skin.vtf_path.as_deref().is_some_and(|other| key(other) == key(&path.to_string_lossy()))) {
                remove_in_root(path, &install.join("addons").join("skins").join("materials"))
                    .map_err(|error| format!("Could not remove installed texture; saved copy kept: {error}"))?;
            }
        }
    }
    fs::remove_file(draft_path(root, id)?).map_err(|error| error.to_string())
}

fn share_path(path: &str) -> bool {
    safe_relative(path) && path.len() <= 512 && path.split('/').all(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !part.ends_with(['.', ' ']) && !part.chars().any(|c| c.is_control() || "<>\"|?*".contains(c))
            && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !(stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    })
}

fn validate_share(draft: &Draft) -> Result<(), String> {
    if draft.name.len() > 1024 || draft.textures.len() > 256 || draft.archived_files.len() > 512
        || draft.slot_selections.len() > 128 {
        return Err("Skin package exceeds the name, texture, file or attachment limits".into());
    }
    if draft.archived_files.is_empty() {
        for path in std::iter::once(draft.model_path.as_str())
            .chain(std::iter::once(draft.active_part_path.as_str()).filter(|path| !path.is_empty()))
            .chain(draft.textures.iter().map(|texture| texture.part_path.as_str())) {
            if !share_path(path) || !path.starts_with("models/") || !path.ends_with(".mdl") {
                return Err("Skin package contains an invalid model path".into());
            }
        }
        for texture in &draft.textures {
            for (path, extension) in [(texture.vmt_path.as_deref(), ".vmt"), (texture.vtf_path.as_deref(), ".vtf")] {
                if let Some(path) = path {
                    if !share_path(path) || !path.starts_with("materials/") || !path.ends_with(extension) {
                        return Err("Skin package contains an invalid material path".into());
                    }
                }
            }
            let data = texture.png.strip_prefix("data:image/png;base64,").unwrap_or(&texture.png);
            if data.len() > 32 * 1024 * 1024 * 4 / 3 + 4 {
                return Err("Skin package texture exceeds 32 MiB".into());
            }
            let mut reader = image::ImageReader::with_format(Cursor::new(png_bytes(data)?), ImageFormat::Png);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(256 * 1024 * 1024);
            reader.limits(limits);
            reader.decode().map_err(|error| format!("Invalid skin package PNG: {error}"))?;
            if supported(texture) { expected_files(texture)?; }
        }
    } else {
        if !draft.textures.is_empty() { return Err("Skin package mixes an editable draft with a backup".into()); }
        let mut paths = std::collections::HashSet::new();
        for file in &draft.archived_files {
            let extension = match file.root.as_str() {
                "vmt" => ".vmt",
                "vtf" => ".vtf",
                _ => return Err("Skin package contains an unsupported backup folder".into()),
            };
            if !share_path(&file.relative) || !file.relative.ends_with(extension)
                || !paths.insert((file.root.clone(), file.relative.to_ascii_lowercase())) {
                return Err("Skin package contains an unsafe or duplicate backup path".into());
            }
        }
    }
    validate(draft)
}

fn package_bytes(mut draft: Draft) -> Result<Vec<u8>, String> {
    validate_share(&draft)?;
    draft.id = None;
    compact_component_sources(&mut draft);
    let bytes = serde_json::to_vec(&SkinPackage {format: SHARE_FORMAT.into(), version: 1, draft})
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_SHARE_BYTES { return Err("Skin package exceeds 256 MiB".into()); }
    Ok(bytes)
}

pub(super) fn import_package_at(root: &Path, bytes: &[u8]) -> Result<Draft, String> {
    if bytes.len() as u64 > MAX_SHARE_BYTES { return Err("Skin package exceeds 256 MiB".into()); }
    let mut package: SkinPackage = serde_json::from_slice(bytes).map_err(|error| format!("Invalid skin package: {error}"))?;
    if package.format != SHARE_FORMAT || package.version != 1 {
        return Err("Unsupported skin package format or version".into());
    }
    expand_component_sources(&mut package.draft).map_err(|error| format!("Invalid skin package: {error}"))?;
    validate_share(&package.draft)?;
    package.draft.id = None;
    save_at(root, package.draft)
}

fn export_package(draft: Draft) -> Result<Option<String>, String> {
    let filename: String = draft.name.chars().take(80)
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let bytes = package_bytes(draft)?;
    let Some(path) = rfd::FileDialog::new().set_file_name(format!("skin-{filename}.phskin"))
        .add_filter("Texture Helper skin", &["phskin"]).save_file() else { return Ok(None); };
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid export path")?)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.as_file().sync_all().map_err(|error| error.to_string())?;
    file.persist_noclobber(&path).map_err(|error| format!("Could not export skin (existing files are not overwritten): {error}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn export_draft(id: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    export_package(prepare_draft_output(id, app, state)?)
}

#[tauri::command]
pub fn import_skin(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("Texture Helper skin", &["phskin"]).pick_file()
        else { return Ok(None); };
    let mut bytes = Vec::new();
    fs::File::open(path).map_err(|error| error.to_string())?.take(MAX_SHARE_BYTES + 1)
        .read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    let draft = import_package_at(&storage(&app)?, &bytes)?;
    textures::import_draft_textures(&app, &*state.0.lock().map_err(|_| "Index lock failed")?, &draft)
        .map_err(|error| format!("Skin \"{}\" was imported, but its textures could not all be added to the library: {error}", draft.name))?;
    Ok(Some(draft.name))
}

fn uninstall_at(draft: &Draft, install: &Path) -> Result<usize, String> {
    let groups = if draft.archived_files.is_empty() {
        draft.textures.iter().filter(|texture| supported(texture))
            .map(expected_files).collect::<Result<Vec<_>, _>>()?
    } else { vec![draft.archived_files.clone()] };
    let mut ids = Vec::new();
    for files in groups {
        if if draft.archived_files.is_empty() { editable_files_match(install, &files)? } else { files_match(install, &files)? } {
            let file = files.iter().find(|file| file.root == "vmt").unwrap_or(&files[0]);
            ids.push(format!("{}:{}", file.root, file.relative));
        }
    }
    if ids.is_empty() {
        return Err("No installed files match this saved skin. Other or modified skins were left untouched.".into());
    }
    for id in &ids { delete_skin_at(install, id)?; }
    Ok(ids.len())
}

#[tauri::command]
pub fn uninstall_draft(id: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone()
        .ok_or("Choose your Garry's Mod folder first")?;
    uninstall_at(&read_at(&storage(&app)?, &id)?, &install)
}

fn installed_draft(install: &Path, id: &str) -> Result<Draft, String> {
    let skin = custom_skin_inventory(install).into_iter().find(|skin| skin.id == id)
        .ok_or("Installed skin no longer exists")?;
    let mut files = Vec::new();
    for (kind, path, folder) in [
        ("vmt", skin.vmt_path.as_ref(), install.join("data").join("phcustomtextures")),
        ("vtf", skin.vtf_path.as_ref().filter(|_| skin.has_vtf), install.join("addons").join("skins").join("materials")),
    ] {
        if let Some(path) = path {
            let path = Path::new(path);
            let relative = key(&path.strip_prefix(&folder).map_err(|error| error.to_string())?.to_string_lossy());
            files.push(SavedFile { root: kind.into(), relative,
                bytes: STANDARD.encode(fs::read(path).map_err(|error| error.to_string())?) });
        }
    }
    Ok(Draft { version: 1, id: None, name: skin.name, model_path: String::new(),
        slot_selections: BTreeMap::new(), active_part_path: String::new(), active_material_key: None,
        textures: Vec::new(), component_textures:Vec::new(), component_error:None, ai_originals: Vec::new(), workshop_source: None, archived_files: files })
}

fn installed_material(install: &Path, id: &str) -> Result<String, String> {
    let skin = custom_skin_inventory(install).into_iter().find(|skin| skin.id == id)
        .ok_or("Installed skin no longer exists")?;
    if !skin.has_vtf { return Err("Installed texture is missing; this skin cannot be edited".into()); }
    let path = skin.vmt_path.ok_or("This texture has no installed material, so its model cannot be identified")?;
    let relative = Path::new(&path).strip_prefix(install.join("data").join("phcustomtextures"))
        .map_err(|error| error.to_string())?.to_string_lossy().into_owned();
    let path = format!("materials/{}", key(&relative));
    if !is_skin_material_path(&path) { return Err("This is not a supported first-person skin material".into()); }
    Ok(path)
}

fn matching_models(index: &ContentIndex, material: &str) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for model in index.models.iter().filter(|model| model.compatible) {
        let source = find(index, &model.path).ok_or("Indexed model is missing; refresh models")?;
        let mdl = model::mdl::read_mdl_bytes(&source.read()?)
            .map_err(|error| format!("Could not inspect {}: {error}", model.path))?;
        if mdl.body_parts.iter().any(|body| body.models.first().is_some_and(|body| body.meshes.iter().any(|mesh| {
            mdl.texture_for_material(0, mesh.material).is_some_and(|texture| {
                material_candidates(&texture.name, &mdl.cd_texture_paths).into_iter()
                    .map(|path| key(&path)).find(|path| index.files.contains_key(path)).as_deref() == Some(material)
            })
        }))) { paths.push(model.path.clone()); }
    }
    paths.sort(); paths.dedup();
    Ok(paths)
}

#[tauri::command]
pub async fn installed_skin_models(id: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let index = state.0.lock().map_err(|_| "Index lock failed")?;
    let install = index.install_root.as_deref().ok_or("Choose your Garry's Mod folder first")?;
    matching_models(&index, &installed_material(install, &id)?)
}

fn editable_installed_at(root: &Path, index: &ContentIndex, id: &str, model_path: &str) -> Result<Draft, String> {
    let install = index.install_root.as_deref().ok_or("Choose your Garry's Mod folder first")?;
    let selected_material = installed_material(install, id)?;
    if !matching_models(index, &selected_material)?.iter().any(|path| path == model_path) {
        return Err("Selected model does not use this installed material. Refresh and choose a matching model.".into());
    }
    let detail = build_model(index, model_path)?;
    let inventory = custom_skin_inventory(install);
    let mut textures = Vec::new();
    for material in &detail.materials {
        let (Some(vmt), Some(vtf)) = (&material.vmt_path, &material.vtf_path) else { continue; };
        if !is_skin_material_path(vmt) || !is_skin_material_path(vtf) { continue; }
        let Some(skin) = inventory.iter().find(|skin| skin.id == format!("vmt:{}", vmt.trim_start_matches("materials/")) && skin.has_vtf)
            else { continue; };
        let text = fs::read_to_string(skin.vmt_path.as_ref().ok_or("Installed material is missing")?)
            .map_err(|error| error.to_string())?;
        let bytes = fs::read(skin.vtf_path.as_ref().ok_or("Installed texture is missing")?)
            .map_err(|error| error.to_string())?;
        let image = vtf_decode::decode_highres(&bytes).map_err(|error| format!("Could not decode installed {vmt}: {error}"))?;
        textures::validate_replacement_dimensions(image.width(), image.height(),
            material.width.unwrap_or(0), material.height.unwrap_or(0))
            .map_err(|error| format!("Installed {vmt}: {error}"))?;
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).map_err(|error| error.to_string())?;
        let texture = DraftTexture {key:vmt.clone(), part_path:model_path.into(), vmt_path:Some(vmt.clone()),
            vtf_path:Some(vtf.clone()), vmt_text:Some(text), png:format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner())),
            strip_alpha:false};
        if !editable_files_match(install, &expected_files(&texture)?)? {
            return Err(format!("Installed {vmt} uses a non-standard texture mapping. Export a backup instead; no files were changed."));
        }
        if !textures.iter().any(|other: &DraftTexture| other.key == texture.key) { textures.push(texture); }
    }
    if !textures.iter().any(|texture| texture.key == selected_material) { return Err("Selected installed material could not be imported".into()); }
    if root.is_dir() {
        for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().is_some_and(|extension| extension == "json") {
                let existing = read_at(root, path.file_stem().and_then(|value| value.to_str()).ok_or("Invalid draft filename")?)?;
                if existing.archived_files.is_empty() && existing.model_path == model_path && existing.textures.len() == textures.len()
                    && textures.iter().all(|texture| existing.textures.iter().any(|other|
                    other.key == texture.key && other.png == texture.png && other.vmt_text == texture.vmt_text)) {
                    return Ok(existing);
                }
            }
        }
    }
    save_at(root, Draft {version:1, id:None, name:format!("{} - installed", model_path.rsplit('/').next().unwrap_or(model_path).trim_end_matches(".mdl")),
        model_path:model_path.into(), slot_selections:BTreeMap::new(), active_part_path:model_path.into(),
        active_material_key:Some(selected_material), textures, component_textures:Vec::new(), component_error:None, ai_originals:Vec::new(), workshop_source:None, archived_files:Vec::new()})
}

#[tauri::command]
pub async fn edit_installed_skin(id: String, model_path: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Draft, String> {
    let index = state.0.lock().map_err(|_| "Index lock failed")?;
    editable_installed_at(&storage(&app)?, &index, &id, &model_path)
}

#[tauri::command]
pub fn export_custom_skin(id: String, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone()
        .ok_or("Choose your Garry's Mod folder first")?;
    export_package(installed_draft(&install, &id)?)
}

fn archive_at(root: &Path, install: &Path, id: &str) -> Result<Draft, String> {
    let saved = save_at(root, installed_draft(install, id)?)?;
    // The durable backup is written before any installed file is removed.
    delete_skin_at(install, id)?;
    Ok(saved)
}

#[tauri::command]
pub fn archive_custom_skin(id: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone()
        .ok_or("Choose your Garry's Mod folder first")?;
    archive_at(&storage(&app)?, &install, &id)?;
    Ok(())
}

fn install_files(draft: &Draft) -> Result<Vec<SavedFile>, String> {
    if draft.archived_files.is_empty() {
        draft.textures.iter().filter(|texture| supported(texture))
            .map(expected_files).collect::<Result<Vec<_>, _>>().map(|files| files.into_iter().flatten().collect())
    } else { Ok(draft.archived_files.clone()) }
}

fn export_files(skins: &[SkinExport]) -> Result<Vec<SavedFile>, String> {
    let mut files = Vec::new();
    let mut materials = std::collections::HashSet::new();
    for skin in skins {
        let vmt = key(&skin.vmt_path);
        if !materials.insert(vmt.clone()) { return Err("Duplicate material selected for installation".into()); }
        files.extend(expected_files(&DraftTexture {key:vmt.clone(), part_path:String::new(),
            vmt_path:Some(vmt), vtf_path:Some(key(&skin.material_path)), vmt_text:Some(skin.vmt_text.clone()),
            png:skin.png_base64.clone(), strip_alpha:skin.strip_alpha})?);
    }
    Ok(files)
}

fn installation_targets(files: &[SavedFile], install: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    if files.is_empty() { return Err("No supported textures to install in this draft".into()); }
    if !install.join("addons").is_dir() || !install.join("data").is_dir() {
        return Err("Choose your Garry's Mod folder first".into());
    }
    let mut targets = BTreeMap::new();
    for file in files {
        let target = file_target(install, file)?;
        let bytes = STANDARD.decode(&file.bytes).map_err(|error| error.to_string())?;
        if let Some(other) = targets.insert(target, bytes.clone()) {
            if other != bytes { return Err("Saved textures conflict at the same installation path".into()); }
        }
    }
    Ok(targets)
}

fn overwrite_fingerprint(path: &Path, installed: &[u8], replacement: &[u8]) -> String {
    let mut hash = Sha256::new();
    for bytes in [path.to_string_lossy().as_bytes(), installed, replacement] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallConflict {
    path: String,
    fingerprint: String,
    installed_png: Option<String>,
    replacement_png: Option<String>,
    installed_text: Option<String>,
    replacement_text: Option<String>,
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReview {
    conflicts: Vec<InstallConflict>,
    affected_models: Vec<String>,
    warnings: Vec<String>,
}

fn material_settings(text: &str) -> Vec<String> {
    let tokens = Regex::new(r#"//[^\r\n]*|/\*[\s\S]*?\*/|"([^"]*)"|([^\s"{}]+)|([{}])"#).unwrap();
    let mut result = Vec::new();
    let mut skip_reference = false;
    for token in tokens.captures_iter(text).filter_map(|capture|
        capture.get(1).or_else(|| capture.get(2)).or_else(|| capture.get(3)).map(|token| key(token.as_str()))) {
        if skip_reference { skip_reference = false; continue; }
        if token == "$basetexture" { skip_reference = true; continue; }
        result.push(token);
    }
    result
}

fn install_affected_models(files: &[SavedFile], index: &ContentIndex, install: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let targets = installation_targets(files, install)?;
    let mut users = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for (path, source) in index.files.iter().filter(|(path, _)| path.ends_with(".mdl") && is_skin_target_model(path)) {
        let mdl = model::mdl::read_mdl_bytes(&source.read()?)
            .map_err(|error| format!("Could not review {path}: {error}"))?;
        for texture in &mdl.textures {
            let material = material_candidates(&texture.name, &mdl.cd_texture_paths).into_iter()
                .find(|candidate| find(index, candidate).is_some());
            if let Some(material) = material.filter(|material| is_skin_material_path(material)) {
                users.entry(key(&material)).or_default().insert(path.clone());
            }
        }
    }
    let mut affected = std::collections::BTreeSet::new();
    let mut warnings = Vec::new();
    for (vmt, models) in users {
        let target = file_target(install, &SavedFile {root:"vmt".into(),
            relative:vmt.trim_start_matches("materials/").into(), bytes:String::new()})?;
        if !target.exists() && !targets.contains_key(&target) { continue; }
        let before = if target.exists() { fs::read(&target).map_err(|error| error.to_string())? }
            else { find(index, &vmt).ok_or("Original material is not indexed")?.read()? };
        let after = targets.get(&target).unwrap_or(&before);
        let before_text = std::str::from_utf8(&before).map_err(|error| format!("Cannot review {vmt}: {error}"))?;
        let after_text = std::str::from_utf8(after).map_err(|error| format!("Cannot review {vmt}: {error}"))?;
        let references = textures::texture_reference(before_text).and_then(|before|
            textures::texture_reference(after_text).map(|after| (before, after)));
        let (before_reference, after_reference) = match references {
            Ok(references) => references,
            Err(error) => {
                warnings.push(format!("Could not determine effects for {} ({vmt}): {error}", models.iter().cloned().collect::<Vec<_>>().join(", ")));
                continue;
            }
        };
        let texture_target = |reference: &str| file_target(install, &SavedFile {root:"vtf".into(),
            relative:reference.trim_start_matches("materials/").into(), bytes:String::new()});
        let before_target = texture_target(&before_reference)?;
        let after_target = texture_target(&after_reference)?;
        if before == *after && !targets.contains_key(&before_target) && !targets.contains_key(&after_target) { continue; }
        let settings_changed = material_settings(before_text) != material_settings(after_text);
        let read_texture = |reference: &str, path: &Path, pending: bool| -> Result<image::RgbaImage, String> {
            let bytes = if let Some(bytes) = targets.get(path).filter(|_| pending) { bytes.clone() }
                else if path.exists() { fs::read(path).map_err(|error| error.to_string())? }
                else { find(index, reference).ok_or_else(|| format!("Cannot review missing texture {reference}"))?.read()? };
            textures::decode_texture(&bytes).map(|image| image.to_rgba8())
                .map_err(|error| format!("Cannot review {reference}: {error}"))
        };
        let images = if settings_changed { None } else {
            match read_texture(&before_reference, &before_target, false).and_then(|before|
                read_texture(&after_reference, &after_target, true).map(|after| (before, after))) {
                Ok(images) => Some(images),
                Err(error) => {
                    warnings.push(format!("Could not determine effects for {} ({vmt}): {error}", models.iter().cloned().collect::<Vec<_>>().join(", ")));
                    continue;
                }
            }
        };
        for path in models {
            if affected.contains(&path) { continue; }
            let detail = build_model(index, &path).map_err(|error| format!("Could not review {path}: {error}"))?;
            for (material_index, material) in detail.materials.iter().enumerate() {
                if material.vmt_path.as_deref() != Some(&vmt) { continue; }
                let uvs = detail.meshes.iter().filter(|mesh| mesh.material_index == material_index)
                    .flat_map(|mesh| mesh.uvs.iter().copied()).collect::<Vec<_>>();
                let pixels_changed = if let Some((before, after)) = &images {
                    shared_texture::uv_pixels_differ(before, after, &uvs)?
                } else { false };
                if !uvs.is_empty() && (settings_changed || pixels_changed) {
                    affected.insert(path.clone());
                    break;
                }
            }
        }
    }
    Ok((affected.into_iter().collect(), warnings))
}

fn install_review(files: &[SavedFile], index: &ContentIndex, install: &Path) -> Result<InstallReview, String> {
    let (affected_models, warnings) = install_affected_models(files, index, install)?;
    Ok(InstallReview {conflicts:install_conflicts(files, install)?, affected_models, warnings})
}

fn install_preview(bytes: &[u8]) -> Result<String, String> {
    let mut pixels = textures::decode_texture(bytes)?.to_rgba8();
    for pixel in pixels.pixels_mut() { pixel[3] = 255; }
    let image = DynamicImage::ImageRgba8(pixels).thumbnail(320, 320);
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, ImageFormat::Png).map_err(|error| error.to_string())?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner())))
}

fn install_conflicts(files: &[SavedFile], install: &Path) -> Result<Vec<InstallConflict>, String> {
    let mut conflicts = Vec::new();
    for (path, replacement) in installation_targets(files, install)? {
        if !path.exists() { continue; }
        let installed = fs::read(&path).map_err(|error| error.to_string())?;
        if installed == replacement { continue; }
        let mut conflict = InstallConflict {path:path.display().to_string(),
            fingerprint:overwrite_fingerprint(&path, &installed, &replacement),
            installed_png:None, replacement_png:None, installed_text:None, replacement_text:None, warning:None};
        if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("vtf")) {
            let mut warnings = Vec::new();
            match install_preview(&installed) {
                Ok(png) => conflict.installed_png = Some(png),
                Err(error) => warnings.push(format!("Installed VTF preview unavailable: {error}")),
            }
            match install_preview(&replacement) {
                Ok(png) => conflict.replacement_png = Some(png),
                Err(error) => warnings.push(format!("Replacement VTF preview unavailable: {error}")),
            }
            if !warnings.is_empty() { conflict.warning = Some(warnings.join("\n")); }
        } else {
            conflict.installed_text = Some(String::from_utf8_lossy(&installed).into_owned());
            conflict.replacement_text = Some(String::from_utf8_lossy(&replacement).into_owned());
        }
        conflicts.push(conflict);
    }
    Ok(conflicts)
}

fn install_output(root: &Path, id: &str, choices: Option<&BTreeMap<String, bool>>, index: &ContentIndex) -> Result<(Draft, Draft), String> {
    let mut draft = read_at(root, id)?;
    if let Some(choices) = choices {
        for texture in &mut draft.textures {
            if let Some(choice) = texture.vtf_path.as_ref().and_then(|path| choices.get(path)) { texture.strip_alpha = *choice; }
        }
    }
    let output = shared_texture::flatten_draft(index, &draft)?;
    Ok((draft, output))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentMergeRequest {
    id: String,
    vtf_paths: Vec<String>,
    component_keys: Option<Vec<String>>,
    alpha_choices: BTreeMap<String, bool>,
}

fn component_merge_files(draft: &Draft, request: &ComponentMergeRequest, index: &ContentIndex, install: &Path) -> Result<Vec<SavedFile>, String> {
    if !draft.archived_files.is_empty() || draft.component_textures.is_empty() {
        return Err("Merge requires an editable draft with individual component assignments; flattened VTFs and backups can only replace the entire texture".into());
    }
    let mut groups = BTreeMap::<String, Vec<shared_texture::ComponentTexture>>::new();
    let mut found = std::collections::HashSet::new();
    for component in &draft.component_textures {
        let component_key = format!("{}:{}", component.part_path, component.material_index);
        if request.component_keys.as_ref().is_some_and(|keys| !keys.contains(&component_key)) { continue; }
        let detail = build_model(index, &component.part_path)?;
        let material = detail.materials.get(component.material_index).ok_or("Merge component material is missing")?;
        let vtf = material.vtf_path.as_ref().ok_or("Merge component has no original VTF")?;
        if !request.vtf_paths.contains(vtf) { continue; }
        if request.component_keys.is_none() {
            let original = material.image.as_ref().ok_or("Merge component has no original texture for comparison")?;
            if textures::decode_texture(&png_bytes(original)?)?.to_rgba8()
                == textures::decode_texture(&png_bytes(&component.png)?)?.to_rgba8() { continue; }
        }
        found.insert(component_key);
        let mut source = component.clone();
        if request.alpha_choices.get(vtf) == Some(&true) {
            let mut image = textures::decode_texture(&png_bytes(&source.png)?)?.to_rgba8();
            for pixel in image.pixels_mut() { pixel[3] = 255; }
            let mut png = Cursor::new(Vec::new());
            image.write_to(&mut png, ImageFormat::Png).map_err(|error| error.to_string())?;
            source.png = format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner()));
        }
        groups.entry(vtf.clone()).or_default().push(source);
    }
    if request.component_keys.as_ref().is_some_and(|keys| keys.is_empty() || keys.iter().any(|key| !found.contains(key))) {
        return Err("The selected merge component changed or is no longer in this draft".into());
    }
    if groups.is_empty() { return Err("No changed component assignments are available to merge for these textures".into()); }
    if request.vtf_paths.iter().any(|vtf| !groups.contains_key(vtf)) {
        return Err("Merge is only available for component-scoped textures. Install shared-only edits separately, or choose Replace entire texture.".into());
    }
    let mut files = Vec::new();
    for (vtf, components) in groups {
        let detail = build_model(index, &components[0].part_path)?;
        let material = &detail.materials[components[0].material_index];
        let template = draft.textures.iter().find(|texture| texture.vtf_path.as_ref() == Some(&vtf))
            .ok_or("Merge component has no shared draft texture")?;
        let relative = format!("{}_custom.vtf", vtf.trim_start_matches("materials/").trim_end_matches(".vtf"));
        let target = file_target(install, &SavedFile {root:"vtf".into(), relative:relative.clone(), bytes:String::new()})?;
        let base = if target.exists() { textures::decode_texture(&fs::read(&target).map_err(|error| error.to_string())?)?.to_rgba8() }
            else { textures::decode_texture(&png_bytes(material.image.as_ref().ok_or("Merge has no original base texture")?)?)?.to_rgba8() };
        let png = shared_texture::merge_components(index, &vtf, base, &components)?;
        let mut texture = template.clone();
        texture.png = png;
        // Strip alpha only inside imported sources, never across preserved installed regions.
        texture.strip_alpha = false;
        let image = textures::decode_texture(&png_bytes(&texture.png)?)?;
        files.push(SavedFile {root:"vtf".into(), relative:relative.clone(), bytes:STANDARD.encode(encode_skin_vtf(&image, false)?)});
        for component in &components {
            let detail = build_model(index, &component.part_path)?;
            let material = &detail.materials[component.material_index];
            texture.vmt_path = material.vmt_path.clone();
            let vmt = texture.vmt_path.as_ref().ok_or("Merge component has no VMT")?;
            texture.vmt_text = draft.textures.iter().find(|other| other.vmt_path.as_ref() == Some(vmt))
                .and_then(|other| other.vmt_text.clone()).or_else(|| material.vmt_text.clone());
            let expected = expected_files(&texture)?.into_iter().find(|file| file.root == "vmt").unwrap();
            let target = file_target(install, &expected)?;
            if target.exists() {
                let text = fs::read_to_string(&target).map_err(|error| error.to_string())?;
                if textures::texture_reference(&text)? != format!("materials/{relative}") {
                    return Err(format!("{} points to another VTF; merge cannot safely change this material mapping", target.display()));
                }
                files.push(SavedFile {bytes:STANDARD.encode(text.as_bytes()), ..expected});
            } else { files.push(expected); }
        }
    }
    let mut unique = BTreeMap::new();
    for file in files {
        let id = (file.root.clone(), file.relative.clone());
        if let Some(previous) = unique.insert(id, file.clone()) {
            if previous.bytes != file.bytes { return Err("Merged materials conflict at the same path".into()); }
        }
    }
    Ok(unique.into_values().collect())
}

fn prepared_component_merge(root: &Path, request: &ComponentMergeRequest, index: &ContentIndex, install: &Path) -> Result<Vec<SavedFile>, String> {
    component_merge_files(&read_at(root, &request.id)?, request, index, install)
}

#[tauri::command]
pub async fn preview_component_merge(request: ComponentMergeRequest, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<InstallReview, String> {
    let index = state.0.lock().map_err(|_| "Index lock failed")?.clone();
    let install = index.install_root.clone().ok_or("Choose your Garry's Mod folder first")?;
    let root = storage(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        install_review(&prepared_component_merge(&root, &request, &index, &install)?, &index, &install)
    }).await.map_err(|error| format!("Merge preview failed: {error}"))?
}

#[tauri::command]
pub async fn install_component_merge(request: ComponentMergeRequest, overwrite: BTreeMap<String, String>,
    app: tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let index = state.0.lock().map_err(|_| "Index lock failed")?.clone();
    let install = index.install_root.clone().ok_or("Choose your Garry's Mod folder first")?;
    let root = storage(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let files = prepared_component_merge(&root, &request, &index, &install)?;
        save_and_install_merge(&root, &request.id, files, &install, &overwrite)
    }).await.map_err(|error| format!("Component installation failed: {error}"))?
}

fn save_and_install_merge(root: &Path, source_id: &str, files: Vec<SavedFile>, install: &Path,
    overwrite: &BTreeMap<String, String>) -> Result<String, String> {
    for conflict in install_conflicts(&files, install)? {
        if overwrite.get(&conflict.path) != Some(&conflict.fingerprint) {
            return Err("Installed files or the merged result changed since review. Review the merge again.".into());
        }
    }
    let mut snapshot = read_at(root, source_id)?;
    snapshot.id = None;
    snapshot.name = format!("{} - merged installation", snapshot.name);
    snapshot.textures.clear();
    snapshot.component_textures.clear();
    snapshot.component_error = None;
    snapshot.ai_originals.clear();
    snapshot.archived_files = files;
    let saved = save_at(root, snapshot)?;
    if let Err(error) = restore_files_at(&saved.archived_files, install, overwrite) {
        return Err(format!("{error}\nThe merged copy was saved locally as \"{}\" but installation did not complete.", saved.name));
    }
    Ok(saved.name)
}

#[tauri::command]
pub async fn preview_skin_install(id: Option<String>, skins: Option<Vec<SkinExport>>, alpha_choices: Option<BTreeMap<String, bool>>,
    app: tauri::AppHandle, state: State<'_, AppState>) -> Result<InstallReview, String> {
    let index = state.0.lock().map_err(|_| "Index lock failed")?.clone();
    let install = index.install_root.clone()
        .or_else(|| rfd::FileDialog::new().set_title("Choose the garrysmod folder for installation").pick_folder())
        .ok_or("Export cancelled")?;
    let root = storage(&app)?;
    let destination = install.clone();
    let review = tauri::async_runtime::spawn_blocking(move || {
        let files = match (id, skins) {
            (Some(id), None) => {
                let (_, output) = install_output(&root, &id, alpha_choices.as_ref(), &index)?;
                install_files(&output)?
            }
            (None, Some(skins)) => export_files(&skins)?,
            _ => return Err("Choose either a saved skin or edited textures for installation".into()),
        };
        install_review(&files, &index, &destination)
    }).await.map_err(|error| format!("Install review failed: {error}"))??;
    state.0.lock().map_err(|_| "Index lock failed")?.install_root = Some(install);
    Ok(review)
}

fn restore_files_at(files: &[SavedFile], install: &Path, overwrite: &BTreeMap<String, String>) -> Result<(), String> {
    let mut prepared = Vec::new();
    for (target, bytes) in installation_targets(files, install)? {
        let previous = if target.exists() { Some(fs::read(&target).map_err(|error| error.to_string())?) } else { None };
        if previous.as_ref() == Some(&bytes) { continue; }
        if let Some(previous) = &previous {
            if overwrite.get(&target.display().to_string()) != Some(&overwrite_fingerprint(&target, previous, &bytes)) {
                return Err(format!("Another skin already exists at {}. Review and confirm the replacement; files may have changed since confirmation.", target.display()));
            }
        }
        fs::create_dir_all(target.parent().ok_or("Invalid saved skin path")?).map_err(|error| error.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap()).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.as_file().sync_all().map_err(|error| error.to_string())?;
        prepared.push((target, file, previous, bytes));
    }
    let mut written: Vec<(PathBuf, Option<Vec<u8>>, Vec<u8>)> = Vec::new();
    for (target, file, previous, bytes) in prepared {
        let result = (|| {
            let current = if target.exists() { Some(fs::read(&target).map_err(|error| error.to_string())?) } else { None };
            if current != previous { return Err(format!("{} changed during installation. Review the replacement again.", target.display())); }
            if previous.is_some() { file.persist(&target).map_err(|error| error.to_string())?; }
            else { file.persist_noclobber(&target).map_err(|error| error.to_string())?; }
            Ok(())
        })();
        if let Err(error) = result {
            let mut errors = vec![error];
            for (path, previous, installed) in written.into_iter().rev() {
                let rollback = (|| {
                    if fs::read(&path).map_err(|error| error.to_string())? != installed {
                        return Err(format!("{} changed after installation; preserved it", path.display()));
                    }
                    if let Some(previous) = previous {
                        let mut backup = tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|error| error.to_string())?;
                        backup.write_all(&previous).map_err(|error| error.to_string())?;
                        backup.as_file().sync_all().map_err(|error| error.to_string())?;
                        backup.persist(&path).map_err(|error| error.to_string())?;
                    } else { fs::remove_file(&path).map_err(|error| error.to_string())?; }
                    Ok(())
                })();
                if let Err(error) = rollback { errors.push(format!("Rollback failed: {error}")); }
            }
            return Err(errors.join("\n"));
        }
        written.push((target, previous, bytes));
    }
    Ok(())
}

#[cfg(test)]
fn restore_at(draft: &Draft, install: &Path) -> Result<(), String> {
    restore_files_at(&install_files(draft)?, install, &BTreeMap::new())
}

pub(super) fn install_texture_exports(skins: &[SkinExport], install: &Path, overwrite: Option<BTreeMap<String, String>>) -> Result<(), String> {
    restore_files_at(&export_files(skins)?, install, &overwrite.unwrap_or_default())
}

#[tauri::command]
pub fn install_draft(id: String, alpha_choices: Option<BTreeMap<String, bool>>, overwrite: Option<BTreeMap<String, String>>,
    app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone()
        .ok_or("Choose your Garry's Mod folder first")?;
    let root = storage(&app)?;
    let (draft, output) = install_output(&root, &id, alpha_choices.as_ref(),
        &*state.0.lock().map_err(|_| "Index lock failed")?)?;
    restore_files_at(&install_files(&output)?, &install, &overwrite.unwrap_or_default())?;
    save_at(&root, draft).map_err(|error| format!("Skin installed, but its alpha settings could not be saved: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_component_images_are_stored_once_and_restored_for_each_part() {
        let mut draft = sample();
        let png = draft.textures[0].png.clone();
        draft.component_textures = vec![
            shared_texture::ComponentTexture {part_path:"models/first.mdl".into(), material_index:0, png:png.clone(), source_id:None},
            shared_texture::ComponentTexture {part_path:"models/second.mdl".into(), material_index:0, png, source_id:None},
        ];
        let original = draft.component_textures.clone();
        compact_component_sources(&mut draft);
        assert!(!draft.component_textures[0].png.is_empty());
        assert!(draft.component_textures[1].png.is_empty());
        assert_eq!(draft.component_textures[0].source_id, draft.component_textures[1].source_id);
        expand_component_sources(&mut draft).unwrap();
        assert_eq!(draft.component_textures, original);
    }

    #[test]
    fn component_sources_persist_while_all_deliverables_use_one_flattened_atlas() {
        let Ok(content) = std::env::var("PERP_TEST_CONTENT") else { return; };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&content), &mut index.files);
        let gun_path = "models/perpheads/weapons/c_ak74u.mdl";
        let stock_path = "models/perpheads/weapons/attachments/stock_ak74.mdl";
        let gun = build_model(&index, gun_path).unwrap();
        let stock = build_model(&index, stock_path).unwrap();
        let (gun_index, material) = gun.materials.iter().enumerate()
            .find(|(_, material)| material.vtf_path.as_ref().is_some_and(|path| path.ends_with("/gripsmag.vtf"))).unwrap();
        let stock_index = stock.materials.iter().position(|other| other.vtf_path == material.vtf_path).unwrap();
        let width = material.width.unwrap();
        let height = material.height.unwrap();
        let largest = if width.max(height) <= 2048 { (width * 2, height * 2) } else { (width, height) };
        let colour = |rgba, (width, height)| {
            let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width, height, image::Rgba(rgba)));
            let mut bytes = Cursor::new(Vec::new());
            image.write_to(&mut bytes, ImageFormat::Png).unwrap();
            format!("data:image/png;base64,{}", STANDARD.encode(bytes.into_inner()))
        };
        let green = colour([8, 230, 80, 255], largest);
        let pink = colour([230, 20, 170, 255], (width / 2, height / 2));
        let mut draft = sample();
        draft.model_path = gun_path.into();
        draft.slot_selections = BTreeMap::from([("SLOT_STOCK".into(), Some("stock_ak74".into()))]);
        draft.textures = vec![DraftTexture {key:material.vmt_path.clone().unwrap(), part_path:gun_path.into(),
            vmt_path:material.vmt_path.clone(), vtf_path:material.vtf_path.clone(), vmt_text:material.vmt_text.clone(),
            png:material.image.clone().unwrap(), strip_alpha:false}];
        draft.component_textures = vec![
            shared_texture::ComponentTexture {part_path:gun_path.into(), material_index:gun_index, png:green.clone(), source_id:None},
            shared_texture::ComponentTexture {part_path:stock_path.into(), material_index:stock_index, png:pink.clone(), source_id:None},
        ];
        let root = tempfile::tempdir().unwrap();
        let saved = save_at(root.path(), draft).unwrap();
        let reloaded = read_at(root.path(), saved.id.as_ref().unwrap()).unwrap();
        assert_eq!(reloaded.component_textures[0].png, green);
        assert_eq!(reloaded.component_textures[1].png, pink);
        let output = shared_texture::flatten_draft(&index, &reloaded).unwrap();
        assert!(output.component_textures.is_empty());
        assert!(output.component_error.is_none());
        assert_eq!(output.textures.len(), 1);
        assert_eq!(output.textures[0].vmt_text, material.vmt_text);
        let pixels = image::load_from_memory(&png_bytes(&output.textures[0].png).unwrap()).unwrap().to_rgba8();
        assert_eq!(pixels.dimensions(), largest);
        assert!(pixels.pixels().any(|pixel| pixel.0 == [8, 230, 80, 255]));
        assert!(pixels.pixels().any(|pixel| pixel.0 == [230, 20, 170, 255]));
        let exported = package_bytes(output.clone()).unwrap();
        let package: serde_json::Value = serde_json::from_slice(&exported).unwrap();
        assert!(package["draft"].get("componentTextures").is_none());
        let workshop_shape = serde_json::to_value(&output).unwrap();
        assert!(workshop_shape.get("componentTextures").is_none());
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        restore_at(&output, install.path()).unwrap();
        let file = expected_files(&output.textures[0]).unwrap().into_iter().find(|file| file.root == "vtf").unwrap();
        let installed = vtf_decode::decode_highres(&fs::read(file_target(install.path(), &file).unwrap()).unwrap()).unwrap();
        assert_eq!(installed.to_rgba8(), pixels);
        assert_eq!(read_at(root.path(), saved.id.as_ref().unwrap()).unwrap().component_textures.len(), 2);

        let aliases: Vec<_> = index.files.iter().filter(|(path, _)| path.starts_with("models/perpheads/weapons/c_ak74u."))
            .map(|(path, source)| (path.replacen("c_ak74u.", "c_component_overlap.", 1), source.clone())).collect();
        index.files.extend(aliases);
        let mut conflicting = reloaded.clone();
        conflicting.component_textures[1].part_path = "models/perpheads/weapons/c_component_overlap.mdl".into();
        conflicting.component_textures[1].material_index = gun_index;
        let error = shared_texture::flatten_draft(&index, &conflicting).err().unwrap();
        assert!(error.contains("conflicting component colours or alpha"), "{error}");
        let mut duplicate = reloaded.clone();
        duplicate.component_textures.push(duplicate.component_textures[0].clone());
        assert!(validate(&duplicate).is_err());
        assert!(shared_texture::flatten_draft(&index, &duplicate).is_err());
    }

    #[test]
    fn installed_import_matches_original_models_without_changing_game_files() {
        let Ok(content) = std::env::var("PERP_TEST_CONTENT") else { return; };
        let game = tempfile::tempdir().unwrap();
        let saved = tempfile::tempdir().unwrap();
        fs::create_dir(game.path().join("addons")).unwrap();
        fs::create_dir(game.path().join("data")).unwrap();
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&content), &mut index.files);
        let path = "models/perpheads/weapons/c_katana.mdl";
        index.models = [path, "models/perpheads/weapons/c_ak101.mdl"].into_iter().map(|path| ModelSummary {
            path:path.into(), origin:"fixture".into(), compatible:true
        }).collect();
        index.install_root = Some(game.path().to_path_buf());
        let original = build_model(&index, path).unwrap();
        let material = original.materials.iter().find(|material| material.vmt_path.as_deref()
            == Some("materials/models/weapons/v_models/katana/metal.vmt")).unwrap();
        let vmt = material.vmt_path.as_ref().unwrap();
        let vtf = material.vtf_path.as_ref().unwrap();
        let edited = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            material.width.unwrap() / 2, material.height.unwrap() / 2, image::Rgba([210, 35, 40, 255])));
        let encoded = encode_vtf(&edited).unwrap();
        write_custom_skin(game.path(), vtf, vmt, material.vmt_text.as_ref().unwrap(), &encoded).unwrap();
        let inventory = custom_skin_inventory(game.path());
        let skin = &inventory[0];
        let original_vmt = fs::read(skin.vmt_path.as_ref().unwrap()).unwrap();
        let original_vtf = fs::read(skin.vtf_path.as_ref().unwrap()).unwrap();
        assert_eq!(matching_models(&index, vmt).unwrap(), [path]);
        assert!(editable_installed_at(saved.path(), &index, &skin.id, "models/perpheads/weapons/c_ak101.mdl").is_err());
        let draft = editable_installed_at(saved.path(), &index, &skin.id, path).unwrap();
        assert!(draft.archived_files.is_empty());
        assert!(draft.slot_selections.is_empty());
        assert_eq!(draft.textures.len(), 1);
        let decoded = image::load_from_memory(&png_bytes(&draft.textures[0].png).unwrap()).unwrap();
        assert_eq!(decoded.to_rgba8(), edited.to_rgba8());
        let status = summary(&draft, Some(game.path())).unwrap();
        assert_eq!(status.installed_count, 1);
        assert_eq!(status.installed_ids, [skin.id.clone()]);
        let again = editable_installed_at(saved.path(), &index, &skin.id, path).unwrap();
        assert_eq!(draft.id, again.id);
        assert_eq!(fs::read(skin.vmt_path.as_ref().unwrap()).unwrap(), original_vmt);
        assert_eq!(fs::read(skin.vtf_path.as_ref().unwrap()).unwrap(), original_vtf);
        assert_eq!(build_model(&index, path).unwrap().materials.iter().find(|entry| entry.vmt_path.as_ref() == Some(vmt)).unwrap().image,
            material.image, "Fresh model must still use the original texture");
        let package = package_bytes(draft.clone()).unwrap();
        let imported_root = tempfile::tempdir().unwrap();
        let imported = import_package_at(imported_root.path(), &package).unwrap();
        assert_eq!(imported.textures[0].png, draft.textures[0].png);
        fs::write(skin.vtf_path.as_ref().unwrap(), b"broken VTF").unwrap();
        assert!(editable_installed_at(saved.path(), &index, &skin.id, path).is_err());
        fs::write(skin.vtf_path.as_ref().unwrap(), original_vtf).unwrap();
        uninstall_at(&draft, game.path()).unwrap();
        assert_eq!(summary(&draft, Some(game.path())).unwrap().installed_count, 0);
    }

    #[test]
    fn installed_matching_preserves_shader_changes_and_compares_vtf_pixels() {
        let actual = b"VertexLitGeneric\n{\n$basetexture \"models\\weapons\\v_models\\test_custom\"\n}";
        let expected = b"VertexLitGeneric\n{\n$basetexture \"models/weapons/v_models/test_custom\"\n}";
        assert!(editable_file_matches(actual, expected, "vmt"));
        assert!(!editable_file_matches(actual, b"UnlitGeneric\n{\n$basetexture \"models/weapons/v_models/test_custom\"\n}", "vmt"));
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 4, image::Rgba([220, 30, 50, 255])));
        let bytes = encode_vtf(&image).unwrap();
        let mut changed_header = bytes.clone();
        changed_header[20] ^= 1;
        assert!(editable_file_matches(&changed_header, &bytes, "vtf"));
        let different = encode_vtf(&DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255])))).unwrap();
        assert!(!editable_file_matches(&bytes, &different, "vtf"));
    }

    fn sample() -> Draft {
        let mut image = image::RgbaImage::new(4, 4);
        for pixel in image.pixels_mut() { *pixel = image::Rgba([220, 30, 50, 255]); }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut png, ImageFormat::Png).unwrap();
        Draft { version: 1, id: None, name: "AK101 draft".into(), model_path: "models/perpheads/weapons/c_ak101.mdl".into(),
            slot_selections: BTreeMap::from([("SLOT_SIGHT".into(), Some("eotech".into())), ("SLOT_MAGAZINE".into(), None)]),
            active_part_path: "models/perpheads/weapons/c_ak101.mdl".into(), active_material_key: Some("material".into()),
            textures: vec![DraftTexture { key: "material".into(), part_path: "models/perpheads/weapons/c_ak101.mdl".into(),
                vmt_path: Some(format!("{SKIN_MATERIAL_PREFIX}ak101/frame.vmt")),
                vtf_path: Some(format!("{SKIN_MATERIAL_PREFIX}ak101/frame.vtf")),
                vmt_text: Some("VertexLitGeneric\n{\n$basetexture \"original\"\n}".into()),
                png: format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner())), strip_alpha: false }],
            component_textures:Vec::new(), component_error:None, ai_originals: Vec::new(), workshop_source: None, archived_files: Vec::new() }
    }

    #[test]
    fn ai_originals_remain_byte_exact_across_draft_save_and_reload() {
        let root = tempfile::tempdir().unwrap();
        let mut draft = sample();
        let png = draft.textures[0].png.clone();
        draft.ai_originals.push(AiOriginal {id:"texture-1".into(), label:"Gun".into(), png:png.clone()});
        let saved = save_at(root.path(), draft).unwrap();
        let restored = read_at(root.path(), saved.id.as_deref().unwrap()).unwrap();
        assert_eq!(restored.ai_originals[0].png, png);
        let bytes = serde_json::to_value(sample()).unwrap();
        assert!(bytes.get("aiOriginals").is_none());
        assert!(serde_json::from_value::<Draft>(bytes).unwrap().ai_originals.is_empty());
    }

    #[test]
    fn shared_skin_roundtrip_keeps_complete_draft_and_imports_as_new_copies() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut draft = sample();
        draft.textures[0].strip_alpha = true;
        draft.workshop_source = Some(WorkshopSource {id:"a".repeat(32), steam_id:"76561198000000000".into(), license:"CC-BY-4.0".into(),
            parent:Some("b".repeat(32)), parent_steam_id:Some("76561198000000001".into())});
        draft.ai_originals.push(AiOriginal {id:"original".into(), label:"Original".into(), png:draft.textures[0].png.clone()});
        let mut second = draft.textures[0].clone();
        second.key = "stock".into();
        second.part_path = "models/perpheads/weapons/stock_ak101.mdl".into();
        draft.textures.push(second);
        let saved = save_at(source.path(), draft).unwrap();
        let bytes = package_bytes(saved.clone()).unwrap();
        let package: SkinPackage = serde_json::from_slice(&bytes).unwrap();
        assert!(package.draft.id.is_none());
        let first = import_package_at(destination.path(), &bytes).unwrap();
        let second = import_package_at(destination.path(), &bytes).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.id, saved.id);
        let mut original = serde_json::to_value(&saved).unwrap();
        let mut imported = serde_json::to_value(read_at(destination.path(), first.id.as_deref().unwrap()).unwrap()).unwrap();
        original["id"] = serde_json::Value::Null;
        imported["id"] = serde_json::Value::Null;
        assert_eq!(original, imported);
        assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 2);
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 1);
    }

    #[test]
    fn workshop_attribution_rejects_invalid_identity_or_licence() {
        let valid = WorkshopSource {id:"a".repeat(32), steam_id:"76561198000000000".into(), license:"CC-BY-4.0".into(),
            parent:None, parent_steam_id:None};
        for invalid in [
            WorkshopSource {id:"../skin".into(), ..valid.clone()},
            WorkshopSource {steam_id:"not-an-id".into(), ..valid.clone()},
            WorkshopSource {license:"Unspecified".into(), ..valid},
        ] {
            let mut draft = sample();
            draft.workshop_source = Some(invalid);
            assert!(validate(&draft).is_err());
        }
    }

    #[test]
    fn deleting_whole_skin_removes_attachments_and_incomplete_leftovers() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let mut draft = sample();
        for (index, name) in ["stock", "suppressor", "magazine"].into_iter().enumerate() {
            let mut texture = draft.textures[0].clone();
            texture.key = name.into();
            texture.vmt_path = Some(format!("{SKIN_MATERIAL_PREFIX}ak101/{name}.vmt"));
            texture.vtf_path = Some(format!("{SKIN_MATERIAL_PREFIX}ak101/{name}.vtf"));
            draft.textures.push(texture);
            assert_eq!(draft.textures.len(), index + 2);
        }
        let saved = save_at(root.path(), draft).unwrap();
        restore_at(&saved, install.path()).unwrap();
        let stock_files = expected_files(&saved.textures[1]).unwrap();
        fs::remove_file(file_target(install.path(), &stock_files[0]).unwrap()).unwrap();
        let suppressor_files = expected_files(&saved.textures[2]).unwrap();
        fs::remove_file(file_target(install.path(), &suppressor_files[1]).unwrap()).unwrap();
        assert_eq!(custom_skin_inventory(install.path()).len(), 4);
        delete_at(root.path(), Some(install.path()), saved.id.as_deref().unwrap()).unwrap();
        assert!(custom_skin_inventory(install.path()).is_empty());
        assert!(!draft_path(root.path(), saved.id.as_deref().unwrap()).unwrap().exists());
    }

    #[test]
    fn deleting_skin_preserves_other_references_and_modified_files() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let saved = save_at(root.path(), sample()).unwrap();
        restore_at(&saved, install.path()).unwrap();
        let files = expected_files(&saved.textures[0]).unwrap();
        let other_vmt = install.path().join("data").join("phcustomtextures").join("other.vmt");
        fs::write(&other_vmt, STANDARD.decode(&files[0].bytes).unwrap()).unwrap();
        delete_at(root.path(), Some(install.path()), saved.id.as_deref().unwrap()).unwrap();
        assert!(other_vmt.exists());
        assert!(file_target(install.path(), &files[1]).unwrap().exists());
        delete_skin_at(install.path(), "vmt:other.vmt").unwrap();
        assert!(custom_skin_inventory(install.path()).is_empty());

        let saved = save_at(root.path(), sample()).unwrap();
        restore_at(&saved, install.path()).unwrap();
        let target = file_target(install.path(), &files[1]).unwrap();
        fs::write(&target, b"modified texture").unwrap();
        delete_at(root.path(), Some(install.path()), saved.id.as_deref().unwrap()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"modified texture");
        let leftovers = custom_skin_inventory(install.path());
        assert_eq!(leftovers.len(), 1);
        assert!(delete_skin_at(install.path(), &leftovers[0].id).unwrap().is_empty());
    }

    #[test]
    fn shared_backups_export_without_uninstall_and_import_without_installing() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let original = sample();
        restore_at(&original, install.path()).unwrap();
        let files = expected_files(&original.textures[0]).unwrap();
        let id = format!("vmt:{}", files[0].relative);
        let installed = installed_draft(install.path(), &id).unwrap();
        let bytes = package_bytes(installed).unwrap();
        assert!(files_match(install.path(), &files).unwrap());
        let archived = archive_at(root.path(), install.path(), &id).unwrap();
        let archived_bytes = package_bytes(archived).unwrap();
        assert_eq!(bytes, archived_bytes);
        let imported = import_package_at(root.path(), &bytes).unwrap();
        assert!(!files_match(install.path(), &files).unwrap());
        assert!(imported.textures.is_empty());
        assert_eq!(serde_json::to_value(&imported.archived_files).unwrap(), serde_json::to_value(&files).unwrap());
        restore_at(&imported, install.path()).unwrap();
        assert!(files_match(install.path(), &files).unwrap());
        fs::write(file_target(install.path(), &files[0]).unwrap(), b"someone else's skin").unwrap();
        assert!(restore_at(&imported, install.path()).is_err());
    }

    #[test]
    fn shared_skin_rejects_bad_formats_paths_images_and_backups_without_saving() {
        let root = tempfile::tempdir().unwrap();
        let bytes = package_bytes(sample()).unwrap();
        let baseline: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (field, value) in [("format", serde_json::json!("other")), ("version", serde_json::json!(2))] {
            let mut invalid = baseline.clone();
            invalid[field] = value;
            assert!(import_package_at(root.path(), &serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        for path in ["../escape.mdl", "C:/private.mdl", "models/../escape.mdl", "models\\escape.mdl",
            "models/CON.mdl", "models/trailing. /escape.mdl", "models/a:stream.mdl"] {
            let mut invalid = baseline.clone();
            invalid["draft"]["modelPath"] = serde_json::json!(path);
            assert!(import_package_at(root.path(), &serde_json::to_vec(&invalid).unwrap()).is_err(), "{path}");
        }
        let mut invalid = baseline.clone();
        invalid["draft"]["textures"][0]["png"] = serde_json::json!("not a PNG");
        assert!(import_package_at(root.path(), &serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid = baseline.clone();
        invalid["draft"]["textures"][0]["vmtText"] = serde_json::json!("missing base texture");
        assert!(import_package_at(root.path(), &serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut oversized = sample();
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image::RgbaImage::new(8193, 1)).write_to(&mut png, ImageFormat::Png).unwrap();
        oversized.textures[0].png = STANDARD.encode(png.into_inner());
        assert!(package_bytes(oversized).is_err());
        let mut oversized = sample();
        oversized.name = "x".repeat(1025);
        assert!(package_bytes(oversized).is_err());
        let mut oversized = sample();
        oversized.textures = vec![oversized.textures[0].clone(); 257];
        assert!(package_bytes(oversized).is_err());
        let mut backup = sample();
        backup.textures.clear();
        for (folder, path) in [("vmt", "../bad.vmt"), ("vmt", "config.cfg"), ("vtf", "nul.vtf"), ("lua", "bad.lua")] {
            backup.archived_files = vec![SavedFile {root:folder.into(), relative:path.into(), bytes:STANDARD.encode(b"test")}];
            assert!(package_bytes(backup.clone()).is_err());
        }
        backup.archived_files = vec![
            SavedFile {root:"vmt".into(), relative:"same.vmt".into(), bytes:STANDARD.encode(b"one")},
            SavedFile {root:"vmt".into(), relative:"SAME.vmt".into(), bytes:STANDARD.encode(b"two")},
        ];
        assert!(package_bytes(backup).is_err());
        assert!(import_package_at(root.path(), b"not JSON").is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn jarheads_install_alpha_choice_preserves_draft_and_uninstalls_matching_files() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let mut draft = sample();
        let texture = &mut draft.textures[0];
        texture.part_path = "models/perpheads/weapons/attachments/rifle_eotech.mdl".into();
        texture.vmt_path = Some("materials/models/jarheads/weapons/attachments/rifle_sights/eotech/eotech553.vmt".into());
        texture.vtf_path = Some("materials/models/jarheads/weapons/attachments/rifle_sights/eotech/eotech553.vtf".into());
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 4, image::Rgba([70, 90, 130, 0])))
            .write_to(&mut png, ImageFormat::Png).unwrap();
        texture.png = format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner()));
        let original_png = texture.png.clone();
        texture.strip_alpha = true;
        assert!(supported(texture));
        let saved = save_at(root.path(), draft).unwrap();
        restore_at(&saved, install.path()).unwrap();
        assert_eq!(summary(&saved, Some(install.path())).unwrap().installed_count, 1);
        let expected = expected_files(&saved.textures[0]).unwrap();
        let vtf_file = expected.iter().find(|file| file.root == "vtf").unwrap();
        let bytes = fs::read(file_target(install.path(), vtf_file).unwrap()).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()) & 0x2000, 0);
        assert!(vtf_decode::decode_highres(&bytes).unwrap().to_rgba8().pixels().all(|p| p.0 == [70, 90, 130, 255]));
        let loaded = read_at(root.path(), saved.id.as_deref().unwrap()).unwrap();
        assert_eq!(loaded.textures[0].png, original_png);
        assert!(loaded.textures[0].strip_alpha);
        uninstall_at(&saved, install.path()).unwrap();
        assert_eq!(summary(&saved, Some(install.path())).unwrap().installed_count, 0);
        assert_eq!(read_at(root.path(), saved.id.as_deref().unwrap()).unwrap().textures[0].png, original_png);
        let mut legacy = serde_json::to_value(&saved).unwrap();
        legacy["textures"][0].as_object_mut().unwrap().remove("stripAlpha");
        assert!(!serde_json::from_value::<Draft>(legacy).unwrap().textures[0].strip_alpha);
    }

    #[test]
    fn drafts_persist_loadout_png_and_updates_without_installing() {
        let root = tempfile::tempdir().unwrap();
        let mut saved = save_at(root.path(), sample()).unwrap();
        let id = saved.id.clone().unwrap();
        let loaded = read_at(root.path(), &id).unwrap();
        assert_eq!(loaded.slot_selections["SLOT_MAGAZINE"], None);
        assert_eq!(loaded.slot_selections["SLOT_SIGHT"].as_deref(), Some("eotech"));
        assert_eq!(loaded.textures[0].png, saved.textures[0].png);
        saved.name = "Updated".into();
        save_at(root.path(), saved).unwrap();
        assert_eq!(read_at(root.path(), &id).unwrap().name, "Updated");
        let mut invalid = read_at(root.path(), &id).unwrap();
        invalid.textures[0].png = "not a PNG".into();
        assert!(save_at(root.path(), invalid).is_err());
        assert_eq!(read_at(root.path(), &id).unwrap().name, "Updated");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        assert!(draft_path(root.path(), "../escape").is_err());
    }

    #[test]
    fn draft_install_status_and_uninstall_preserve_saved_copy_and_other_skins() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let draft = save_at(root.path(), sample()).unwrap();
        assert_eq!(summary(&draft, Some(install.path())).unwrap().installed_count, 0);
        let texture = &draft.textures[0];
        restore_at(&draft, install.path()).unwrap();
        restore_at(&draft, install.path()).unwrap();
        assert_eq!(summary(&draft, Some(install.path())).unwrap().installed_count, 1);
        let files = expected_files(texture).unwrap();
        let vmt = file_target(install.path(), &files[0]).unwrap();
        fs::write(&vmt, b"modified by someone else").unwrap();
        assert!(uninstall_at(&draft, install.path()).is_err());
        assert!(vmt.exists());
        fs::write(&vmt, STANDARD.decode(&files[0].bytes).unwrap()).unwrap();
        uninstall_at(&draft, install.path()).unwrap();
        assert_eq!(summary(&draft, Some(install.path())).unwrap().installed_count, 0);
        assert!(read_at(root.path(), draft.id.as_deref().unwrap()).is_ok());
    }

    #[test]
    fn install_impact_ignores_vmt_formatting_and_texture_reference_only_changes() {
        let original = "VertexLitGeneric\n{\n$basetexture \"models/original\"\n$phong 1\n}";
        let custom = "\"vertexlitgeneric\" { // comment\n \"$basetexture\" \"models/custom\" \n\"$phong\" \"1\" /* comment */ }";
        assert_eq!(material_settings(original), material_settings(custom));
        assert_ne!(material_settings(original), material_settings(&custom.replace("\"1\"", "\"0\"")));
    }

    #[test]
    fn actual_component_install_merge_preserves_installed_regions_material_and_draft_sources() {
        let Ok(content) = std::env::var("PERP_TEST_CONTENT") else { return; };
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&content), &mut index.files);
        let full_index = index.clone();
        let gun_path = "models/perpheads/weapons/c_ak74u.mdl";
        let stock_path = "models/perpheads/weapons/attachments/stock_ak74.mdl";
        index.files.retain(|path, _| !path.ends_with(".mdl") || path == gun_path || path == stock_path);
        let gun = build_model(&index, gun_path).unwrap();
        let stock = build_model(&index, stock_path).unwrap();
        let (gun_index, material) = gun.materials.iter().enumerate().find(|(_, material)|
            material.vtf_path.as_ref().is_some_and(|path| path.ends_with("/gripsmag.vtf"))).unwrap();
        let stock_index = stock.materials.iter().position(|other| other.vtf_path == material.vtf_path).unwrap();
        let (width, height) = (material.width.unwrap(), material.height.unwrap());
        let make_png = |colour, width, height| {
            let mut png = Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(width, height, image::Rgba(colour)).write_to(&mut png, ImageFormat::Png).unwrap();
            format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner()))
        };
        let mut old = sample();
        old.textures = vec![DraftTexture {key:material.vmt_path.clone().unwrap(), part_path:gun_path.into(),
            vmt_path:material.vmt_path.clone(), vtf_path:material.vtf_path.clone(), vmt_text:material.vmt_text.clone(),
            png:make_png([20, 90, 200, 40], width, height), strip_alpha:false}];
        let initial_files = install_files(&old).unwrap();
        let initial_review = install_review(&initial_files, &full_index, install.path()).unwrap();
        assert!(initial_review.conflicts.is_empty(), "A first install must still review affected models");
        assert!(initial_review.warnings.is_empty(), "{:?}", initial_review.warnings);
        assert!(initial_review.affected_models.contains(&gun_path.to_owned()));
        restore_at(&old, install.path()).unwrap();
        assert!(install_affected_models(&initial_files, &index, install.path()).unwrap().0.is_empty(),
            "Reinstalling exactly the installed skin must not flag any model");
        let initial_vtf = initial_files.iter().find(|file| file.root == "vtf").unwrap();
        let initial_target = file_target(install.path(), initial_vtf).unwrap();
        fs::write(&initial_target, b"broken installed VTF").unwrap();
        let unreadable = install_review(&initial_files, &index, install.path()).unwrap();
        assert!(!unreadable.warnings.is_empty(), "Unknown effects must be visible, not reported as unchanged");
        assert!(unreadable.affected_models.is_empty());
        assert_eq!(unreadable.conflicts.len(), 1, "A corrupt installed VTF must still offer reviewed replacement");
        fs::write(initial_target, STANDARD.decode(&initial_vtf.bytes).unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut draft = old.clone();
        draft.component_textures = vec![
            shared_texture::ComponentTexture {part_path:gun_path.into(), material_index:gun_index,
                png:make_png([210, 30, 50, 80], width * 2, height * 2), source_id:None},
            shared_texture::ComponentTexture {part_path:stock_path.into(), material_index:stock_index,
                png:stock.materials[stock_index].image.clone().unwrap(), source_id:None},
        ];
        let draft = save_at(root.path(), draft).unwrap();
        let request = ComponentMergeRequest {id:draft.id.clone().unwrap(), vtf_paths:vec![material.vtf_path.clone().unwrap()],
            component_keys:Some(vec![format!("{gun_path}:{gun_index}")]), alpha_choices:BTreeMap::new()};
        let snapshot = fs::read(draft_path(root.path(), &request.id).unwrap()).unwrap();
        let before = installation_targets(&install_files(&old).unwrap(), install.path()).unwrap();
        let merged = prepared_component_merge(root.path(), &request, &index, install.path()).unwrap();
        let (affected, warnings) = install_affected_models(&merged, &index, install.path()).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(affected, vec![gun_path.to_owned()],
            "A stock sharing the VTF must not be flagged when its effective UV pixels are unchanged");
        let stock_material = &stock.materials[stock_index];
        if stock_material.vmt_path != material.vmt_path {
            let mut stock_texture = old.textures[0].clone();
            stock_texture.vmt_path = stock_material.vmt_path.clone();
            stock_texture.vmt_text = stock_material.vmt_text.clone();
            let stock_vmt = expected_files(&stock_texture).unwrap().into_iter().find(|file| file.root == "vmt").unwrap();
            let stock_target = file_target(install.path(), &stock_vmt).unwrap();
            fs::create_dir_all(stock_target.parent().unwrap()).unwrap();
            fs::write(&stock_target, STANDARD.decode(&stock_vmt.bytes).unwrap()).unwrap();
            assert_eq!(install_affected_models(&merged, &index, install.path()).unwrap().0, vec![gun_path.to_owned()],
                "An already-installed stock material referencing the changed atlas still needs changed UV pixels to count");
            let mut replacement = old.clone();
            replacement.textures[0].png = make_png([90, 200, 20, 255], width, height);
            assert!(install_affected_models(&install_files(&replacement).unwrap(), &index, install.path()).unwrap().0
                .contains(&stock_path.to_owned()), "Changing the entire installed atlas also affects the stock's existing material");
            fs::remove_file(stock_target).unwrap();
        }
        assert_eq!(merged.len(), 2, "Snapshot must track the existing VMT without rewriting its settings");
        let merged_vtf = merged.iter().find(|file| file.root == "vtf").unwrap();
        let image = textures::decode_texture(&STANDARD.decode(&merged_vtf.bytes).unwrap()).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (width * 2, height * 2));
        assert!(image.pixels().any(|pixel| pixel.0 == [210, 30, 50, 80]));
        assert!(image.pixels().any(|pixel| pixel.0 == [20, 90, 200, 40]));
        let conflicts = install_conflicts(&merged, install.path()).unwrap();
        let approval = conflicts.into_iter().map(|conflict| (conflict.path, conflict.fingerprint)).collect();
        let mut alpha = request.clone();
        alpha.alpha_choices.insert(material.vtf_path.clone().unwrap(), true);
        let opaque = prepared_component_merge(root.path(), &alpha, &index, install.path()).unwrap();
        let opaque_vtf = opaque.iter().find(|file| file.root == "vtf").unwrap();
        let image = textures::decode_texture(&STANDARD.decode(&opaque_vtf.bytes).unwrap()).unwrap().to_rgba8();
        assert!(image.pixels().any(|pixel| pixel.0 == [210, 30, 50, 255]));
        assert!(image.pixels().any(|pixel| pixel.0 == [20, 90, 200, 40]), "Alpha removal must not touch preserved regions");
        assert!(restore_files_at(&opaque, install.path(), &approval).is_err(), "Changed alpha choice requires new approval");
        for (path, bytes) in &before { assert_eq!(fs::read(path).unwrap(), *bytes); }
        restore_files_at(&merged, install.path(), &approval).unwrap();
        assert!(install_affected_models(&merged, &index, install.path()).unwrap().0.is_empty());
        assert_eq!(fs::read(draft_path(root.path(), &request.id).unwrap()).unwrap(), snapshot);
        let vmt = before.keys().find(|path| path.extension().unwrap() == "vmt").unwrap();
        assert_eq!(fs::read(vmt).unwrap(), before[vmt]);
        let alias = "models/perpheads/weapons/c_merge_overlap.mdl";
        index.files.insert(alias.into(), index.files[gun_path].clone());
        for extension in ["vvd", "dx90.vtx"] {
            let source = gun_path.trim_end_matches("mdl").to_owned() + extension;
            if let Some(value) = index.files.get(&source).cloned() {
                index.files.insert(alias.trim_end_matches("mdl").to_owned() + extension, value);
            }
        }
        assert!(component_merge_files(&draft, &request, &index, install.path()).is_ok(), "Already identical overlapping pixels are safe");
        let mut changed = draft.clone();
        changed.component_textures[0].png = make_png([90, 200, 20, 255], width, height);
        assert!(component_merge_files(&changed, &request, &index, install.path()).err().unwrap().contains("Merge blocked"));
        let flattened = shared_texture::flatten_draft(&index, &draft).unwrap();
        assert!(component_merge_files(&flattened, &request, &index, install.path()).is_err());
        let name = save_and_install_merge(root.path(), &request.id, merged.clone(), install.path(), &BTreeMap::new()).unwrap();
        assert!(name.ends_with(" - merged installation"));
        let saved = fs::read_dir(root.path()).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
            .find_map(|path| {
                let saved = read_at(root.path(), path.file_stem().unwrap().to_str().unwrap()).unwrap();
                if saved.id != draft.id { Some(saved) } else { None }
            }).unwrap();
        assert!(saved.textures.is_empty() && saved.component_textures.is_empty());
        assert_eq!(summary(&saved, Some(install.path())).unwrap().installed_count, 1);
        assert_eq!(fs::read(draft_path(root.path(), &request.id).unwrap()).unwrap(), snapshot);
        assert!(package_bytes(saved.clone()).is_ok(), "Merged installation must export as a complete backup");
        uninstall_at(&saved, install.path()).unwrap();
        assert_eq!(summary(&saved, Some(install.path())).unwrap().installed_count, 0);
        restore_at(&saved, install.path()).unwrap();
        assert_eq!(summary(&saved, Some(install.path())).unwrap().installed_count, 1);
    }

    #[test]
    fn overwrite_requires_review_of_exact_installed_and_replacement_files() {
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let mut original = sample();
        original.id = Some("original".into());
        restore_at(&original, install.path()).unwrap();
        let before = installation_targets(&install_files(&original).unwrap(), install.path()).unwrap();
        let mut replacement = original.clone();
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([20, 90, 200, 80])))
            .write_to(&mut png, ImageFormat::Png).unwrap();
        replacement.textures[0].png = format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner()));
        replacement.textures[0].vmt_text = Some("VertexLitGeneric\n{\n$basetexture \"original\"\n$phong 1\n}".into());
        let files = install_files(&replacement).unwrap();
        let conflicts = install_conflicts(&files, install.path()).unwrap();
        assert_eq!(conflicts.len(), 2);
        let preview = conflicts.iter().find(|conflict| conflict.installed_png.is_some()).unwrap();
        let left = image::load_from_memory(&png_bytes(preview.installed_png.as_ref().unwrap()).unwrap()).unwrap().to_rgba8();
        let right = image::load_from_memory(&png_bytes(preview.replacement_png.as_ref().unwrap()).unwrap()).unwrap().to_rgba8();
        assert_eq!(left.get_pixel(0, 0).0, [220, 30, 50, 255]);
        assert_eq!(right.get_pixel(0, 0).0, [20, 90, 200, 255]);
        let approval: BTreeMap<_, _> = conflicts.iter().map(|conflict| (conflict.path.clone(), conflict.fingerprint.clone())).collect();
        assert!(restore_files_at(&files, install.path(), &BTreeMap::new()).is_err());
        for (path, bytes) in &before { assert_eq!(fs::read(path).unwrap(), *bytes); }
        let vtf = before.keys().find(|path| path.extension().unwrap() == "vtf").unwrap();
        fs::write(vtf, b"externally changed").unwrap();
        assert!(restore_files_at(&files, install.path(), &approval).is_err());
        assert_eq!(fs::read(vtf).unwrap(), b"externally changed");
        fs::write(vtf, &before[vtf]).unwrap();
        let mut changed_files = files.clone();
        changed_files[0].bytes = STANDARD.encode(b"changed replacement");
        assert!(restore_files_at(&changed_files, install.path(), &approval).is_err());
        for (path, bytes) in &before { assert_eq!(fs::read(path).unwrap(), *bytes); }
        let unrelated = install.path().join("addons").join("unrelated.txt");
        fs::write(&unrelated, b"untouched").unwrap();
        restore_files_at(&files, install.path(), &approval).unwrap();
        assert_eq!(summary(&original, Some(install.path())).unwrap().installed_count, 0);
        assert_eq!(summary(&replacement, Some(install.path())).unwrap().installed_count, 1);
        assert!(install_conflicts(&files, install.path()).unwrap().is_empty());
        restore_files_at(&files, install.path(), &BTreeMap::new()).unwrap();
        assert_eq!(fs::read(unrelated).unwrap(), b"untouched");
        let source = &replacement.textures[0];
        install_texture_exports(&[SkinExport {material_path:source.vtf_path.clone().unwrap(),
            vmt_path:source.vmt_path.clone().unwrap(), vmt_text:source.vmt_text.clone().unwrap(),
            png_base64:source.png.clone(), strip_alpha:false}], install.path(), None).unwrap();
    }

    #[test]
    fn undecodable_installed_vtf_reports_preview_warning_without_hiding_conflict() {
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let files = install_files(&sample()).unwrap();
        let vtf = files.iter().find(|file| file.root == "vtf").unwrap();
        let target = file_target(install.path(), vtf).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"damaged VTF").unwrap();
        let conflicts = install_conflicts(&files, install.path()).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].installed_png.is_none());
        assert!(conflicts[0].replacement_png.is_some());
        assert!(conflicts[0].warning.as_deref().unwrap().contains("Installed VTF preview unavailable"));
        assert_eq!(fs::read(&target).unwrap(), b"damaged VTF");
    }

    #[cfg(windows)]
    #[test]
    fn failed_overwrite_rolls_back_already_replaced_files() {
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let files = install_files(&sample()).unwrap();
        restore_files_at(&files, install.path(), &BTreeMap::new()).unwrap();
        let before = installation_targets(&files, install.path()).unwrap();
        let mut changed = files.clone();
        for file in &mut changed { file.bytes = STANDARD.encode(b"new bytes"); }
        let conflicts = install_conflicts(&changed, install.path()).unwrap();
        let approval = conflicts.into_iter().map(|conflict| (conflict.path, conflict.fingerprint)).collect();
        let vmt = before.keys().find(|path| path.extension().unwrap() == "vmt").unwrap();
        let mut permissions = fs::metadata(vmt).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(vmt, permissions.clone()).unwrap();
        let result = restore_files_at(&changed, install.path(), &approval);
        permissions.set_readonly(false);
        fs::set_permissions(vmt, permissions).unwrap();
        assert!(result.is_err());
        for (path, bytes) in before { assert_eq!(fs::read(path).unwrap(), bytes); }
    }

    #[test]
    fn uninstall_backup_retains_shared_vtf_and_restore_refuses_conflicts() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        let folder = install.path().join("data").join("phcustomtextures");
        let vtf = install.path().join("addons").join("skins").join("materials").join("shared_custom.vtf");
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir_all(vtf.parent().unwrap()).unwrap();
        fs::write(folder.join("one.vmt"), b"$basetexture \"shared_custom\"").unwrap();
        fs::write(folder.join("two.vmt"), b"$basetexture \"shared_custom\"").unwrap();
        fs::write(&vtf, b"shared bytes").unwrap();
        let draft = archive_at(root.path(), install.path(), "vmt:one.vmt").unwrap();
        assert!(!folder.join("one.vmt").exists());
        assert!(folder.join("two.vmt").exists() && vtf.exists());
        restore_at(&draft, install.path()).unwrap();
        uninstall_at(&draft, install.path()).unwrap();
        assert!(vtf.exists());
        fs::write(folder.join("one.vmt"), b"another skin").unwrap();
        assert!(restore_at(&draft, install.path()).is_err());
        assert_eq!(fs::read(folder.join("one.vmt")).unwrap(), b"another skin");
    }

    #[test]
    fn existing_installed_skin_can_be_uninstalled_and_restored_without_loss() {
        let root = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        let vmt = install.path().join("data").join("phcustomtextures").join("test.vmt");
        let vtf = install.path().join("addons").join("skins").join("materials").join("test_custom.vtf");
        fs::create_dir_all(vmt.parent().unwrap()).unwrap();
        fs::create_dir_all(vtf.parent().unwrap()).unwrap();
        fs::write(&vmt, b"$basetexture \"test_custom\"").unwrap();
        fs::write(&vtf, b"original vtf bytes").unwrap();
        let draft = archive_at(root.path(), install.path(), "vmt:test.vmt").unwrap();
        assert!(!vmt.exists() && !vtf.exists());
        restore_at(&draft, install.path()).unwrap();
        assert_eq!(fs::read(&vtf).unwrap(), b"original vtf bytes");
        assert_eq!(summary(&draft, Some(install.path())).unwrap().installed_count, 1);
        fs::write(&vtf, b"another skin").unwrap();
        assert!(restore_at(&draft, install.path()).is_err());
        assert_eq!(fs::read(&vtf).unwrap(), b"another skin");
    }
}

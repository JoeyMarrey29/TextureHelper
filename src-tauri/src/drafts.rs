use super::*;
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Serialize, Deserialize)]
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
    root: String,
    relative: String,
    bytes: String,
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
    pub ai_originals: Vec<AiOriginal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_source: Option<WorkshopSource>,
    #[serde(default)]
    archived_files: Vec<SavedFile>,
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

pub(super) fn read_at(root: &Path, id: &str) -> Result<Draft, String> {
    let bytes = fs::read(draft_path(root, id)?).map_err(|error| error.to_string())?;
    let draft: Draft = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if draft.id.as_deref() != Some(id) {
        return Err("Draft ID does not match its filename".into());
    }
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
    let bytes = serde_json::to_vec(&draft).map_err(|error| error.to_string())?;
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
            if let Some(root) = install {
                if files_match(root, &expected_files(texture)?)? {
                    installed_count += 1;
                    installed_ids.push(format!("vmt:{}", texture.vmt_path.as_deref().unwrap().trim_start_matches("materials/")));
                }
            }
        }
    }
    Ok(DraftSummary { id: draft.id.clone().ok_or("Draft has no ID")?, name: draft.name.clone(),
        model_path: draft.model_path.clone(), texture_count: draft.textures.len(),
        installed_count, installable_count, archived, installed_ids })
}

#[tauri::command]
pub fn save_draft(draft: Draft, app: tauri::AppHandle) -> Result<Draft, String> {
    if !draft.archived_files.is_empty() {
        return Err("Installed-skin backups cannot be edited as assembly drafts".into());
    }
    save_at(&storage(&app)?, draft)
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
pub fn load_draft(id: String, app: tauri::AppHandle) -> Result<Draft, String> {
    read_at(&storage(&app)?, &id)
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
            if target.is_file() && fs::read(&target).map_err(|error| error.to_string())? == bytes {
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
pub fn export_draft(id: String, app: tauri::AppHandle) -> Result<Option<String>, String> {
    export_package(read_at(&storage(&app)?, &id)?)
}

#[tauri::command]
pub fn import_skin(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("Texture Helper skin", &["phskin"]).pick_file()
        else { return Ok(None); };
    let mut bytes = Vec::new();
    fs::File::open(path).map_err(|error| error.to_string())?.take(MAX_SHARE_BYTES + 1)
        .read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    let draft = import_package_at(&storage(&app)?, &bytes)?;
    Ok(Some(draft.name))
}

fn uninstall_at(draft: &Draft, install: &Path) -> Result<usize, String> {
    let groups = if draft.archived_files.is_empty() {
        draft.textures.iter().filter(|texture| supported(texture))
            .map(expected_files).collect::<Result<Vec<_>, _>>()?
    } else { vec![draft.archived_files.clone()] };
    let mut ids = Vec::new();
    for files in groups {
        if files_match(install, &files)? {
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
        textures: Vec::new(), ai_originals: Vec::new(), workshop_source: None, archived_files: files })
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

fn restore_at(draft: &Draft, install: &Path) -> Result<(), String> {
    let files = if draft.archived_files.is_empty() {
        draft.textures.iter().filter(|texture| supported(texture))
            .map(expected_files).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect()
    } else { draft.archived_files.clone() };
    if files.is_empty() { return Err("No supported textures to install in this draft".into()); }
    if !install.join("addons").is_dir() || !install.join("data").is_dir() {
        return Err("Choose your Garry's Mod folder first".into());
    }
    let mut pending = BTreeMap::new();
    for file in &files {
        let target = file_target(install, file)?;
        let bytes = STANDARD.decode(&file.bytes).map_err(|error| error.to_string())?;
        if target.exists() {
            if fs::read(&target).map_err(|error| error.to_string())? != bytes {
                return Err(format!("Another skin already exists at {}", target.display()));
            }
        } else if let Some(other) = pending.insert(target, bytes.clone()) {
            if other != bytes { return Err("Saved textures conflict at the same installation path".into()); }
        }
    }
    let mut prepared = Vec::new();
    for (target, bytes) in pending {
        fs::create_dir_all(target.parent().ok_or("Invalid saved skin path")?).map_err(|error| error.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap()).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.as_file().sync_all().map_err(|error| error.to_string())?;
        prepared.push((target, file));
    }
    let mut written = Vec::new();
    for (target, file) in prepared {
        if let Err(error) = file.persist_noclobber(&target) {
            let mut errors = vec![error.to_string()];
            for path in written {
                if let Err(error) = fs::remove_file(path) { errors.push(format!("Rollback failed: {error}")); }
            }
            return Err(errors.join("\n"));
        }
        written.push(target);
    }
    Ok(())
}

#[tauri::command]
pub fn install_draft(id: String, alpha_choices: Option<BTreeMap<String, bool>>, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let install = state.0.lock().map_err(|_| "Index lock failed")?.install_root.clone()
        .ok_or("Choose your Garry's Mod folder first")?;
    let root = storage(&app)?;
    let mut draft = read_at(&root, &id)?;
    if let Some(choices) = alpha_choices {
        for texture in &mut draft.textures {
            if let Some(choice) = texture.vtf_path.as_ref().and_then(|path| choices.get(path)) {
                texture.strip_alpha = *choice;
            }
        }
    }
    restore_at(&draft, &install)?;
    save_at(&root, draft).map_err(|error| format!("Skin installed, but its alpha settings could not be saved: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ai_originals: Vec::new(), workshop_source: None, archived_files: Vec::new() }
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

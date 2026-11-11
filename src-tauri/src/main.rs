#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gma;
mod model;
mod vtf_decode;
mod updates;
mod drafts;
mod textures;
mod shared_texture;
mod workshop;

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImageView, ImageFormat};
use regex::Regex;
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{Manager, State};
use valve_pak::VPK;
use walkdir::WalkDir;

use gma::GmaArchive;
use model::mdl::BonePose;
use model::vvd::VvdVertex;
use model::{MdlFile, read_mdl, read_vtx, read_vvd};

#[derive(Clone)]
enum Source {
    Loose(PathBuf),
    Archive {
        gma: PathBuf,
        entry: String,
        package_name: String,
    },
    Vpk {
        archive: PathBuf,
        entry: String,
    },
}

impl Source {
    fn origin(&self) -> String {
        match self {
            Self::Loose(path) => path.display().to_string(),
            Self::Archive { gma, .. } => gma.display().to_string(),
            Self::Vpk { archive, .. } => archive.display().to_string(),
        }
    }

    fn read(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Loose(path) => fs::read(path).map_err(|error| error.to_string()),
            Self::Archive { gma, entry, .. } => GmaArchive::open(gma)
                .and_then(|archive| archive.read(entry))
                .map_err(|error| error.to_string()),
            Self::Vpk { archive, entry } => {
                let package = VPK::open(archive).map_err(|error| error.to_string())?;
                let mut reader = package.get_file(entry).map_err(|error| error.to_string())?;
                let mut bytes = Vec::new();
                reader
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                Ok(bytes)
            }
        }
    }
}

#[derive(Clone, Default)]
struct ContentIndex {
    files: HashMap<String, Source>,
    install_root: Option<PathBuf>,
    models: Vec<ModelSummary>,
}

#[derive(Default)]
struct AppState(Mutex<ContentIndex>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelSummary {
    path: String,
    origin: String,
    compatible: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanResult {
    root: String,
    archive_count: usize,
    model_count: usize,
    models: Vec<ModelSummary>,
    has_install_root: bool,
    custom_skins: Vec<CustomSkin>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomSkin {
    id: String,
    name: String,
    vmt_path: Option<String>,
    vtf_path: Option<String>,
    has_vtf: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterialInfo {
    name: String,
    vmt_path: Option<String>,
    vmt_origin: Option<String>,
    vmt_text: Option<String>,
    vtf_path: Option<String>,
    vtf_origin: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    image: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeshInfo {
    material_index: usize,
    positions: Vec<f32>,
    normals: Vec<f32>,
    uvs: Vec<f32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelDetail {
    path: String,
    origin: String,
    triangle_count: usize,
    materials: Vec<MaterialInfo>,
    meshes: Vec<MeshInfo>,
    related_models: Vec<ModelSummary>,
    attachments: Vec<AttachmentInfo>,
    root_bone_transform: Option<[f32; 16]>,
    bones: Vec<BoneInfo>,
    /// Sequence whose first frame poses the mesh and bones; None means the reference pose.
    sequence: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentInfo {
    name: String,
    transform: [f32; 16],
}

/// Model-space bone matrix in the displayed pose, used to bonemerge parts by bone name.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BoneInfo {
    name: String,
    transform: [f32; 16],
}

fn source_matrix(values: &[f32; 12]) -> [f32; 16] {
    [
        values[0], values[4], values[8], 0.0, values[1], values[5], values[9], 0.0, values[2],
        values[6], values[10], 0.0, values[3], values[7], values[11], 1.0,
    ]
}

fn multiply_matrix(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            result[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
    }
    result
}

fn bone_local_matrix((position, [x, y, z, w]): BonePose) -> [f32; 16] {
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        position[0],
        position[1],
        position[2],
        1.0,
    ]
}

fn world_pose(mdl: &MdlFile, local: &[BonePose]) -> Result<Vec<[f32; 16]>, String> {
    let mut world: Vec<[f32; 16]> = Vec::with_capacity(local.len());
    for (index, bone) in mdl.bones.iter().enumerate() {
        let matrix = bone_local_matrix(local[index]);
        if bone.parent < -1 || bone.parent >= index as i32 {
            return Err(format!("Invalid or unordered parent for bone {}", bone.name));
        }
        if matrix.iter().any(|value| !value.is_finite()) {
            return Err(format!("Non-finite pose for bone {}", bone.name));
        }
        world.push(if bone.parent >= 0 {
            multiply_matrix(&world[bone.parent as usize], &matrix)
        } else { matrix });
    }
    Ok(world)
}

fn invert_rigid(m: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for column in 0..3 {
        for row in 0..3 {
            out[column * 4 + row] = m[row * 4 + column];
        }
    }
    for row in 0..3 {
        out[12 + row] = -(0..3).map(|k| out[k * 4 + row] * m[12 + k]).sum::<f32>();
    }
    out[15] = 1.0;
    out
}

fn transform_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    let v = transform_direction(m, p);
    [v[0] + m[12], v[1] + m[13], v[2] + m[14]]
}

fn transform_direction(m: &[f32; 16], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|row| m[row] * v[0] + m[4 + row] * v[1] + m[8 + row] * v[2])
}

/// Viewmodels place some bones only through animation (the G36 magazine bone sits far below the
/// gun in the reference pose), so guns are shown in their idle sequence. PERPHEADS Lua can pick
/// another sequence for a part, e.g. folded flip-up iron sights.
fn display_sequence(mdl: &MdlFile, requested: Option<usize>) -> Result<Option<usize>, String> {
    if let Some(index) = requested {
        if index >= mdl.sequences.len() {
            return Err(format!("Sequence {index} is not available on this model"));
        }
        return Ok(Some(index));
    }
    Ok(mdl.sequences
        .iter()
        .position(|sequence| sequence.activity.eq_ignore_ascii_case("ACT_VM_IDLE"))
        .or_else(|| {
            mdl.sequences.iter().position(|sequence| sequence.label.eq_ignore_ascii_case("idle"))
        }))
}

/// Bone matrices for the sequence's first frame. Viewmodel animations also move the whole gun in
/// view space, so the pose is re-anchored on the bone that skins the most vertices.
fn sequence_pose(mdl: &MdlFile, frame: &[BonePose], rest: &[[f32; 16]], vertices: &[VvdVertex]) -> Result<Vec<[f32; 16]>, String> {
    let animated = world_pose(mdl, frame)?;
    if animated.is_empty() {
        return Ok(animated);
    }
    let mut weights = vec![0.0f32; mdl.bones.len()];
    for vertex in vertices {
        for slot in 0..(vertex.bone_count as usize).min(3) {
            if let Some(weight) = weights.get_mut(vertex.bone_indices[slot] as usize) {
                *weight += vertex.bone_weights[slot];
            }
        }
    }
    let anchor = (0..weights.len())
        .max_by(|a, b| weights[*a].total_cmp(&weights[*b]))
        .unwrap_or(0);
    let realign = multiply_matrix(&rest[anchor], &invert_rigid(&animated[anchor]));
    Ok(animated.iter().map(|matrix| multiply_matrix(&realign, matrix)).collect())
}

fn skin_vertex(vertex: &VvdVertex, skin: &[[f32; 16]]) -> ([f32; 3], [f32; 3]) {
    let (mut position, mut normal, mut total) = ([0.0f32; 3], [0.0f32; 3], 0.0f32);
    for slot in 0..(vertex.bone_count as usize).min(3) {
        let weight = vertex.bone_weights[slot];
        let Some(matrix) = skin.get(vertex.bone_indices[slot] as usize) else {
            continue;
        };
        if weight <= 0.0 {
            continue;
        }
        let (p, n) = (transform_point(matrix, vertex.position), transform_direction(matrix, vertex.normal));
        for axis in 0..3 {
            position[axis] += p[axis] * weight;
            normal[axis] += n[axis] * weight;
        }
        total += weight;
    }
    if total <= 0.0 {
        return (vertex.position, vertex.normal);
    }
    let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt().max(1e-6);
    (position.map(|value| value / total), normal.map(|value| value / length))
}

fn key(path: &str) -> String {
    let mut normalized = path
        .replace('\\', "/")
        .trim_matches('/')
        .to_ascii_lowercase();
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized
}

fn is_perp_model(path: &str, source: &Source) -> bool {
    let lower = path.to_ascii_lowercase();
    let from_perp_archive = matches!(source, Source::Archive { package_name, .. } if package_name.to_ascii_lowercase().contains("perpheads"));
    lower.ends_with(".mdl")
        && (lower.contains("perpheads")
            || from_perp_archive
            || source.origin().to_ascii_lowercase().contains("perpheads"))
}

fn is_skin_target_model(path: &str) -> bool {
    let path = key(path);
    if path.contains("/weapons/components/") || path.contains("/weapons/casing/") {
        return false;
    }
    if path.starts_with("models/perpheads/weapons/attachments/")
        || path.starts_with("models/perpheads/weapons/magazines/")
    {
        return path.ends_with(".mdl");
    }
    path.starts_with("models/perpheads/weapons/c_") && path.ends_with(".mdl")
}

fn collect_loose(base: &Path, files: &mut HashMap<String, Source>) {
    collect_loose_under(base, base, files);
}

fn collect_loose_under(base: &Path, relative_to: &Path, files: &mut HashMap<String, Source>) {
    for item in WalkDir::new(base).into_iter().filter_map(Result::ok) {
        if !item.file_type().is_file()
            || !item.path().extension().is_some_and(|ext| {
                ["mdl", "vvd", "vtx", "vmt", "vtf"]
                    .iter()
                    .any(|want| ext.eq_ignore_ascii_case(want))
            })
        {
            continue;
        }
        if let Ok(relative) = item.path().strip_prefix(relative_to) {
            let path = relative.to_string_lossy();
            files.insert(key(&path), Source::Loose(item.path().to_path_buf()));
        }
    }
}

#[tauri::command]
fn choose_content_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn scan_content(root: String, state: State<'_, AppState>) -> Result<ScanResult, String> {
    let (index, result) = build_index(&root)?;
    *state.0.lock().map_err(|_| "Index lock failed")? = index;
    Ok(result)
}

fn build_index(root: &str) -> Result<(ContentIndex, ScanResult), String> {
    let root_path = PathBuf::from(root);
    let content = if root_path.join("garrysmod").is_dir() {
        root_path.join("garrysmod")
    } else {
        root_path
    };
    if !content.is_dir() {
        return Err("Choose Garry's Mod or a folder of extracted PERPHEADS content.".into());
    }
    let install_root = content.join("addons").is_dir().then(|| content.clone());
    let addons = install_root
        .as_ref()
        .map(|_| content.join("addons"))
        .unwrap_or_else(|| content.clone());
    let mut search_roots = vec![addons.clone()];
    if let Some(install) = &install_root {
        if let Some(steamapps) = install.ancestors().nth(3) {
            let workshop = steamapps.join("workshop/content/4000");
            if workshop.is_dir() {
                search_roots.push(workshop);
            }
        }
    }
    let mut files = HashMap::new();
    let mut archive_count = 0;
    for search_root in &search_roots {
        for item in WalkDir::new(search_root)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
        {
            if item.file_type().is_file()
                && item
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gma"))
            {
                let gma_path = item.path().to_path_buf();
                if let Ok(archive) = ::gma::open(&gma_path) {
                    let package_name = archive.name().to_owned();
                    let relevant = package_name.to_ascii_lowercase().contains("perpheads")
                        || archive
                            .entries()
                            .any(|entry| key(entry.filename()).starts_with("models/perpheads/"));
                    let mut contributed = false;
                    for entry in archive.entries() {
                        let path = key(entry.filename());
                        let is_material = path.starts_with("materials/")
                            && (path.ends_with(".vmt") || path.ends_with(".vtf"));
                        if !relevant && !is_material {
                            continue;
                        }
                        let source = Source::Archive {
                            gma: gma_path.clone(),
                            entry: entry.filename().to_owned(),
                            package_name: package_name.clone(),
                        };
                        if relevant {
                            files.insert(path, source);
                        } else {
                            files.entry(path).or_insert(source);
                        }
                        contributed = true;
                    }
                    if contributed {
                        archive_count += 1;
                    }
                }
            }
        }
    }
    if install_root.is_none() {
        collect_loose(&content, &mut files);
    } else {
        for search_root in &search_roots {
            for entry in fs::read_dir(search_root)
                .map_err(|error| error.to_string())?
                .filter_map(Result::ok)
            {
                if !entry.path().is_dir() {
                    continue;
                }
                let loose_root = if search_root == &addons {
                    if entry.file_name().to_string_lossy().eq_ignore_ascii_case("skins") { continue; }
                    entry.path()
                } else {
                    entry.path().join("content")
                };
                if loose_root.is_dir() {
                    collect_original_loose(&loose_root, &loose_root, &mut files);
                }
            }
        }
    }
    if install_root.is_some() {
        for subdir in ["materials", "models"] {
            let loose_root = content.join(subdir);
            if loose_root.is_dir() {
                collect_original_loose(&loose_root, &content, &mut files);
            }
        }
        if let Some(game_root) = content.parent() {
            for relative in [
                "garrysmod/garrysmod_dir.vpk",
                "garrysmod/fallbacks_dir.vpk",
                "sourceengine/content_cstrike_dir.vpk",
                "sourceengine/content_hl2_dir.vpk",
                "sourceengine/hl2_misc_dir.vpk",
                "sourceengine/hl2_textures_dir.vpk",
            ] {
                let path = game_root.join(relative);
                let Ok(archive) = VPK::open(&path) else {
                    continue;
                };
                for entry in archive.file_paths() {
                    let name = key(entry);
                    if name.starts_with("materials/")
                        && (name.ends_with(".vmt") || name.ends_with(".vtf"))
                    {
                        files.entry(name).or_insert_with(|| Source::Vpk {
                            archive: path.clone(),
                            entry: entry.clone(),
                        });
                    }
                }
            }
        }
    }
    let compatibility = compatible_models(&files);
    let mut models: Vec<ModelSummary> = files
        .iter()
        .filter(|(path, source)| is_perp_model(path, source))
        .map(|(path, source)| ModelSummary {
            path: path.clone(),
            origin: source.origin(),
            compatible: compatibility.get(path).copied().unwrap_or(false),
        })
        .collect();
    models.sort_by(|a, b| a.path.cmp(&b.path));
    let result = ScanResult {
        root: content.display().to_string(),
        archive_count,
        model_count: models.len(),
        models: models.clone(),
        has_install_root: install_root.is_some(),
        custom_skins: install_root
            .as_ref()
            .map(|root| custom_skin_inventory(root))
            .unwrap_or_default(),
    };
    Ok((
        ContentIndex {
            files,
            install_root,
            models,
        },
        result,
    ))
}

const SKIN_MATERIAL_PREFIX: &str = "materials/models/weapons/v_models/";
fn collect_original_loose(base: &Path, relative_to: &Path, files: &mut HashMap<String, Source>) {
    let mut loose = HashMap::new();
    collect_loose_under(base, relative_to, &mut loose);
    for (path, source) in loose {
        // Installed loose skins must not replace the original first-person material source.
        if is_skin_material_path(&path) && files.contains_key(&path) { continue; }
        files.insert(path, source);
    }
}
fn is_skin_material_path(path: &str) -> bool {
    (path.starts_with(SKIN_MATERIAL_PREFIX)
        || path.starts_with("materials/models/jarheads/weapons/"))
        && !path.contains("reticles")
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && !path.contains('\\') && !path.contains(':')
}

fn base_texture_path(reference: &str) -> String {
    let reference = key(reference);
    let reference = reference.strip_prefix("materials/").unwrap_or(&reference);
    let stem = reference.strip_suffix(".vtf")
        .or_else(|| reference.strip_suffix(".vmt")).unwrap_or(reference);
    format!("materials/{stem}.vtf")
}

fn material_candidates(name: &str, dirs: &[String]) -> Vec<String> {
    let mut candidates = Vec::new();
    if name.contains('/') || name.contains('\\') {
        candidates.push(format!("materials/{}.vmt", name.replace('\\', "/")));
    }
    for dir in dirs {
        candidates.push(format!(
            "materials/{}/{}.vmt",
            dir.replace('\\', "/").trim_matches('/'),
            name
        ));
    }
    candidates.push(format!("materials/{name}.vmt"));
    candidates
}

fn mdl_compatible(
    mdl: &MdlFile,
    files: &HashMap<String, Source>,
    checked: &mut HashMap<String, bool>,
    texture_re: &Regex,
) -> bool {
    mdl.body_parts.iter().any(|body| {
        body.models.first().is_some_and(|model| {
            model.meshes.iter().any(|mesh| {
                let Some(texture) = mdl.texture_for_material(0, mesh.material) else {
                    return false;
                };
                let path = material_candidates(&texture.name, &mdl.cd_texture_paths)
                    .into_iter()
                    .map(|path| key(&path))
                    .find(|path| files.contains_key(path));
                let Some(path) = path else { return false };
                if !is_skin_material_path(&path) {
                    return false;
                }
                *checked.entry(path.clone()).or_insert_with(|| {
                    let Some(source) = files.get(&path) else {
                        return false;
                    };
                    let Ok(text) = source
                        .read()
                        .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
                    else {
                        return false;
                    };
                    let Some(reference) = texture_re
                        .captures(&text)
                        .and_then(|capture| capture.get(1).or_else(|| capture.get(2)))
                    else {
                        return false;
                    };
                    let vtf = base_texture_path(reference.as_str());
                    is_skin_material_path(&vtf) && files.contains_key(&vtf)
                })
            })
        })
    })
}

fn compatible_models(files: &HashMap<String, Source>) -> HashMap<String, bool> {
    let mut result = HashMap::new();
    let mut archives: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut checked = HashMap::new();
    let texture_re = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+(?:"([^"\r\n]+)"|([^"\s]+))"#).unwrap();
    for (path, source) in files {
        if !is_perp_model(path, source) || !is_skin_target_model(path) {
            continue;
        }
        match source {
            Source::Archive { gma, .. } => {
                archives.entry(gma.clone()).or_default().push(path.clone())
            }
            _ => {
                let compatible = source
                    .read()
                    .ok()
                    .and_then(|bytes| model::mdl::read_mdl_bytes(&bytes).ok())
                    .is_some_and(|mdl| mdl_compatible(&mdl, files, &mut checked, &texture_re));
                result.insert(path.clone(), compatible);
            }
        }
    }
    for (gma, paths) in archives {
        let Ok(archive) = ::gma::open(&gma) else {
            continue;
        };
        let wanted: std::collections::HashSet<_> = paths.into_iter().collect();
        for entry in archive.entries() {
            let path = key(entry.filename());
            if !wanted.contains(&path) {
                continue;
            }
            let compatible = archive
                .read_entry(entry, |_, reader| {
                    let mut bytes = Vec::new();
                    reader.read_to_end(&mut bytes)?;
                    Ok::<_, std::io::Error>(bytes)
                })
                .ok()
                .and_then(Result::ok)
                .and_then(|bytes| model::mdl::read_mdl_bytes(&bytes).ok())
                .is_some_and(|mdl| mdl_compatible(&mdl, files, &mut checked, &texture_re));
            result.insert(path, compatible);
        }
    }
    result
}

fn custom_skin_inventory(install: &Path) -> Vec<CustomSkin> {
    let vmt_root = install.join("data/phcustomtextures");
    let vtf_root = install.join("addons/skins/materials");
    let texture_re = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+(?:"([^"\r\n]+)"|([^"\s]+))"#).unwrap();
    let mut skins = Vec::new();
    let mut referenced = HashMap::<String, usize>::new();
    for entry in WalkDir::new(&vmt_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file()
            || !entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vmt"))
        {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&vmt_root) else {
            continue;
        };
        let relative = key(&relative.to_string_lossy());
        let vtf_path = fs::read_to_string(entry.path())
            .ok()
            .and_then(|text| {
                texture_re
                    .captures(&text)
                    .and_then(|capture| capture.get(1).or_else(|| capture.get(2)).map(|part| part.as_str().to_owned()))
            })
            .map(|reference| base_texture_path(&reference).trim_start_matches("materials/").trim_end_matches(".vtf").to_owned())
            .filter(|reference| {
                reference.ends_with("_custom")
                    && !reference.contains('\\') && !reference.contains(':')
                    && reference
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..")
            })
            .map(|reference| {
                vtf_root.join(format!(
                    "{}.vtf",
                    reference.trim_start_matches("materials/")
                ))
            });
        if let Some(path) = &vtf_path {
            *referenced.entry(key(&path.to_string_lossy())).or_default() += 1;
        }
        let name = relative.trim_end_matches(".vmt").to_owned();
        skins.push(CustomSkin {
            id: format!("vmt:{relative}"),
            name,
            vmt_path: Some(entry.path().display().to_string()),
            has_vtf: vtf_path.as_ref().is_some_and(|path| path.is_file()),
            vtf_path: vtf_path.map(|path| path.display().to_string()),
        });
    }
    for entry in WalkDir::new(&vtf_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file()
            || !entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vtf"))
        {
            continue;
        }
        let full = key(&entry.path().to_string_lossy());
        if referenced.contains_key(&full) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&vtf_root) else {
            continue;
        };
        let relative = key(&relative.to_string_lossy());
        skins.push(CustomSkin {
            id: format!("vtf:{relative}"),
            name: relative.trim_end_matches(".vtf").to_owned(),
            vmt_path: None,
            vtf_path: Some(entry.path().display().to_string()),
            has_vtf: true,
        });
    }
    skins.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    skins
}

#[tauri::command]
fn list_custom_skins(state: State<'_, AppState>) -> Result<Vec<CustomSkin>, String> {
    let guard = state.0.lock().map_err(|_| "Index lock failed")?;
    Ok(guard
        .install_root
        .as_ref()
        .map(|root| custom_skin_inventory(root))
        .unwrap_or_default())
}

fn remove_in_root(path: &Path, root: &Path) -> Result<(), String> {
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let canonical_path = fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("Skin file is outside the Garry's Mod custom-skin folders.".into());
    }
    fs::remove_file(canonical_path).map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_custom_skin(id: String, state: State<'_, AppState>) -> Result<Vec<CustomSkin>, String> {
    let guard = state.0.lock().map_err(|_| "Index lock failed")?;
    let install = guard
        .install_root
        .as_ref()
        .ok_or("Choose your Garry's Mod folder first")?;
    delete_skin_at(install, &id)
}

fn delete_skin_at(install: &Path, id: &str) -> Result<Vec<CustomSkin>, String> {
    let skins = custom_skin_inventory(install);
    let skin = skins
        .iter()
        .find(|skin| skin.id == id)
        .ok_or("Custom skin no longer exists")?;
    let vtf_references = skin
        .vtf_path
        .as_ref()
        .map(|path| {
            skins
                .iter()
                .filter(|other| other.vmt_path.is_some() && other.vtf_path.as_ref() == Some(path))
                .count()
        })
        .unwrap_or(0);
    if let Some(path) = &skin.vmt_path {
        remove_in_root(Path::new(path), &install.join("data/phcustomtextures"))?;
    }
    if skin.has_vtf && vtf_references <= 1 {
        if let Some(path) = &skin.vtf_path {
            remove_in_root(Path::new(path), &install.join("addons/skins/materials"))?;
        }
    }
    Ok(custom_skin_inventory(install))
}

fn find<'a>(index: &'a ContentIndex, path: &str) -> Option<&'a Source> {
    index.files.get(&key(path))
}

fn material_info(index: &ContentIndex, name: &str, dirs: &[String]) -> MaterialInfo {
    let found = material_candidates(name, dirs)
        .into_iter()
        .find_map(|path| find(index, &path).map(|source| (key(&path), source)));
    let vmt_origin = found.as_ref().map(|(_, source)| source.origin());
    let vmt_text = found
        .as_ref()
        .and_then(|(_, source)| source.read().ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let texture_re = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+(?:"([^"\r\n]+)"|([^"\s]+))"#).unwrap();
    let vtf_path = vmt_text
        .as_ref()
        .and_then(|text| texture_re.captures(text))
        .and_then(|capture| capture.get(1).or_else(|| capture.get(2)))
        .map(|part| base_texture_path(part.as_str()));
    let vtf_source = vtf_path.as_ref().and_then(|path| find(index, path));
    let vtf_origin = vtf_source.map(Source::origin);
    let decoded = vtf_source
        .and_then(|source| source.read().ok())
        .and_then(|bytes| vtf_decode::decode_highres(&bytes).ok());
    let (width, height, image) = match decoded {
        Some(decoded) => {
            let (width, height) = decoded.dimensions();
            let mut png = Cursor::new(Vec::new());
            let image = decoded.write_to(&mut png, ImageFormat::Png).ok().map(|_| {
                format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(png.into_inner())
                )
            });
            (Some(width), Some(height), image)
        }
        None => (None, None, None),
    };
    MaterialInfo {
        name: name.into(),
        vmt_path: found.map(|(path, _)| path),
        vmt_origin,
        vmt_text,
        vtf_path: vtf_path.map(|path| key(&path)),
        vtf_origin,
        width,
        height,
        image,
    }
}

#[tauri::command]
fn open_model(
    path: String,
    sequence: Option<usize>,
    state: State<'_, AppState>,
) -> Result<ModelDetail, String> {
    let guard = state.0.lock().map_err(|_| "Index lock failed")?;
    build_posed_model(&guard, &path, sequence)
}

fn build_model(index: &ContentIndex, path: &str) -> Result<ModelDetail, String> {
    build_posed_model(index, path, None)
}

fn build_posed_model(
    index: &ContentIndex,
    path: &str,
    requested_sequence: Option<usize>,
) -> Result<ModelDetail, String> {
    let base = key(&path)
        .strip_suffix(".mdl")
        .ok_or("Select an MDL file")?
        .to_owned();
    let mdl_source = find(index, &format!("{base}.mdl")).ok_or("Model not indexed")?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let component = |suffix: &str| -> Result<PathBuf, String> {
        let source =
            find(index, &format!("{base}.{suffix}")).ok_or(format!("Missing {suffix} file"))?;
        let target = temp.path().join(format!("model.{suffix}"));
        fs::write(&target, source.read()?).map_err(|error| error.to_string())?;
        Ok(target)
    };
    let mdl = read_mdl(&component("mdl")?).map_err(|error| error.to_string())?;
    let vvd = read_vvd(&component("vvd")?).map_err(|error| error.to_string())?;
    let vtx = read_vtx(&component("dx90.vtx").or_else(|_| component("vtx"))?)
        .map_err(|error| error.to_string())?;
    if mdl.header.checksum != vvd.header.checksum || mdl.header.checksum != vtx.header.checksum {
        return Err("MDL/VVD/VTX checksum mismatch".into());
    }
    let vertices = vvd.mesh_vertices(0).map_err(|error| error.to_string())?;
    let rest_local: Vec<BonePose> = mdl
        .bones
        .iter()
        .map(|bone| (bone.default_position, bone.default_rotation))
        .collect();
    let rest = world_pose(&mdl, &rest_local)?;
    let sequence = if let Some(index) = display_sequence(&mdl, requested_sequence)? {
        let sequence = &mdl.sequences[index];
        let frame = sequence.first_frame.as_ref()
            .map_err(|error| format!("Cannot pose {} sequence '{}': {error}", path, sequence.label))?;
        for vertex in &vertices {
            if vertex.bone_count > 3 || (0..vertex.bone_count as usize).any(|slot| {
                !vertex.bone_weights[slot].is_finite() || vertex.bone_weights[slot] < 0.0
                    || vertex.bone_indices[slot] as usize >= mdl.bones.len()
            }) {
                return Err("Invalid VVD bone influence".into());
            }
        }
        Some((sequence.label.clone(), sequence_pose(&mdl, frame, &rest, &vertices)?))
    } else { None };
    let posed_vertices: Option<Vec<([f32; 3], [f32; 3])>> = sequence.as_ref().map(|(_, pose)| {
        let skin: Vec<[f32; 16]> = pose
            .iter()
            .zip(&rest)
            .map(|(posed, reference)| multiply_matrix(posed, &invert_rigid(reference)))
            .collect();
        vertices.iter().map(|vertex| skin_vertex(vertex, &skin)).collect()
    });
    let bone_matrices = sequence.as_ref().map_or(&rest, |(_, pose)| pose);
    let mut meshes = Vec::new();
    let mut materials = Vec::new();
    let mut material_ids = HashMap::<String, usize>::new();
    let mut triangle_count = 0;
    for (body_index, body) in mdl.body_parts.iter().enumerate() {
        let Some(vtx_body) = vtx.body_parts.get(body_index) else {
            continue;
        };
        let Some(mdl_model) = body.models.first() else {
            continue;
        };
        let Some(vtx_model) = vtx_body.models.first() else {
            continue;
        };
        let Some(lod) = vtx_model.lods.first() else {
            continue;
        };
        let model_base = mdl_model.vertex_index_bytes / 48;
        if model_base < 0 {
            return Err("Negative model vertex base".into());
        }
        for (mesh_index, mesh) in mdl_model.meshes.iter().enumerate() {
            let Some(vtx_mesh) = lod.meshes.get(mesh_index) else {
                continue;
            };
            let name = mdl
                .texture_for_material(0, mesh.material)
                .map(|texture| texture.name.clone())
                .unwrap_or_else(|| format!("material_{}", mesh.material));
            let material_index = *material_ids.entry(name.clone()).or_insert_with(|| {
                let id = materials.len();
                materials.push(material_info(index, &name, &mdl.cd_texture_paths));
                id
            });
            let mut positions = Vec::new();
            let mut normals = Vec::new();
            let mut uvs = Vec::new();
            for group in &vtx_mesh.strip_groups {
                for triangle in &group.triangles {
                    for vertex_id in triangle {
                        let item = group
                            .vertices
                            .get(*vertex_id as usize)
                            .ok_or("Invalid VTX vertex")?;
                        let global = model_base as usize
                            + mesh.vertex_offset as usize
                            + item.original_mesh_vertex_id as usize;
                        let vertex = vertices
                            .get(global)
                            .ok_or("VVD vertex index out of range")?;
                        let (position, normal) = posed_vertices
                            .as_ref()
                            .map_or((vertex.position, vertex.normal), |posed| posed[global]);
                        positions.extend_from_slice(&position);
                        normals.extend_from_slice(&normal);
                        uvs.extend_from_slice(&vertex.uv);
                    }
                    triangle_count += 1;
                }
            }
            if !positions.is_empty() {
                meshes.push(MeshInfo {
                    material_index,
                    positions,
                    normals,
                    uvs,
                });
            }
        }
    }
    if meshes.is_empty() {
        return Err("No renderable LOD0 mesh found".into());
    }
    // MDL files provide mount transforms, but not the PERPHEADS gameplay
    // compatibility rules. Avoid presenting filename matches as valid parts.
    let related_models = Vec::new();
    Ok(ModelDetail {
        path: key(&path),
        origin: mdl_source.origin(),
        triangle_count,
        materials,
        meshes,
        related_models,
        attachments: mdl
            .attachments
            .iter()
            .filter_map(|attachment| {
                let bone = bone_matrices.get(attachment.bone)?;
                Some(AttachmentInfo {
                    name: attachment.name.clone(),
                    transform: multiply_matrix(bone, &source_matrix(&attachment.local)),
                })
            })
            .collect(),
        root_bone_transform: bone_matrices.first().copied(),
        bones: mdl
            .bones
            .iter()
            .zip(bone_matrices)
            .map(|(bone, transform)| BoneInfo {
                name: bone.name.clone(),
                transform: *transform,
            })
            .collect(),
        sequence: sequence.map(|(label, _)| label),
    })
}

fn encode_vtf(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let (width, height) = image.dimensions();
    if width == 0
        || height == 0
        || width > u16::MAX as u32
        || height > u16::MAX as u32
        || !width.is_power_of_two()
        || !height.is_power_of_two()
    {
        return Err("VTF textures need power-of-two dimensions up to 65535 pixels.".into());
    }
    let mut mips = vec![image.to_rgba8()];
    while mips.last().unwrap().width() > 1 || mips.last().unwrap().height() > 1 {
        let previous = mips.last().unwrap();
        mips.push(image::imageops::resize(
            previous,
            (previous.width() / 2).max(1),
            (previous.height() / 2).max(1),
            image::imageops::FilterType::Triangle,
        ));
    }
    let has_alpha = mips[0].pixels().any(|pixel| pixel.0[3] < 255);
    let mut bytes = vec![0_u8; 80];
    bytes[0..4].copy_from_slice(b"VTF\0");
    bytes[4..8].copy_from_slice(&7_u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&2_u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&80_u32.to_le_bytes());
    bytes[16..18].copy_from_slice(&(width as u16).to_le_bytes());
    bytes[18..20].copy_from_slice(&(height as u16).to_le_bytes());
    bytes[20..24].copy_from_slice(&(if has_alpha { 0x2000_u32 } else { 0 }).to_le_bytes());
    bytes[24..26].copy_from_slice(&1_u16.to_le_bytes());
    bytes[48..52].copy_from_slice(&1_f32.to_le_bytes());
    bytes[52..56].copy_from_slice(&0_u32.to_le_bytes()); // RGBA8888
    bytes[56] = mips.len() as u8;
    bytes[57..61].copy_from_slice(&u32::MAX.to_le_bytes()); // No low-resolution image
    bytes[63..65].copy_from_slice(&1_u16.to_le_bytes());
    for mip in mips.into_iter().rev() {
        bytes.extend_from_slice(mip.as_raw());
    }
    Ok(bytes)
}

fn encode_skin_vtf(image: &DynamicImage, strip_alpha: bool) -> Result<Vec<u8>, String> {
    if !strip_alpha { return encode_vtf(image); }
    let mut opaque = image.to_rgba8();
    for pixel in opaque.pixels_mut() { pixel[3] = 255; }
    encode_vtf(&DynamicImage::ImageRgba8(opaque))
}

#[tauri::command]
fn texture_has_alpha(png_base64: String) -> Result<bool, String> {
    let bytes = STANDARD.decode(png_base64.strip_prefix("data:image/png;base64,").unwrap_or(&png_base64))
        .map_err(|error| error.to_string())?;
    let image = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(image.to_rgba8().pixels().any(|pixel| pixel[3] < 255))
}

fn decode_dds(bytes: &[u8]) -> Result<DynamicImage, String> {
    if bytes.len() < 128 || &bytes[..4] != b"DDS " {
        return Err("Invalid DDS header".into());
    }
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let (width, height) = (u32_at(16), u32_at(12));
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err("Unsupported DDS dimensions".into());
    }
    // The image crate handles BC1/BC2/BC3 (DXT1/3/5). Our exported DDS is
    // uncompressed RGBA, so decode that layout here as well.
    if u32_at(84) != 0 {
        return image::load_from_memory_with_format(bytes, ImageFormat::Dds)
            .map_err(|error| format!("Unsupported DDS compression: {error}"));
    }
    if u32_at(88) != 32 {
        return Err("Only 32-bit uncompressed DDS and DXT1/3/5 DDS are supported".into());
    }
    let masks = [u32_at(92), u32_at(96), u32_at(100), u32_at(104)];
    let pitch = if u32_at(20) >= width * 4 {
        u32_at(20) as usize
    } else {
        width as usize * 4
    };
    let data_len = pitch.checked_mul(height as usize).ok_or("DDS too large")?;
    if bytes.len() < 128 + data_len {
        return Err("DDS pixel data is incomplete".into());
    }
    let channel = |pixel: u32, mask: u32, default: u8| -> u8 {
        if mask == 0 {
            return default;
        }
        let bits = mask.count_ones();
        let value = (pixel & mask) >> mask.trailing_zeros();
        ((value as u64 * 255) / ((1_u64 << bits) - 1)) as u8
    };
    let mut image = image::RgbaImage::new(width, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = 128 + y * pitch + x * 4;
            let pixel = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            image.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    channel(pixel, masks[0], 0),
                    channel(pixel, masks[1], 0),
                    channel(pixel, masks[2], 0),
                    channel(pixel, masks[3], 255),
                ]),
            );
        }
    }
    Ok(DynamicImage::ImageRgba8(image))
}

fn encode_dds(image: &DynamicImage) -> Vec<u8> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut bytes = vec![0_u8; 128];
    bytes[..4].copy_from_slice(b"DDS ");
    let mut put =
        |offset: usize, value: u32| bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    put(4, 124);
    put(8, 0x100f);
    put(12, height);
    put(16, width);
    put(20, width * 4);
    put(76, 32);
    put(80, 0x41);
    put(88, 32);
    put(92, 0x000000ff);
    put(96, 0x0000ff00);
    put(100, 0x00ff0000);
    put(104, 0xff000000);
    put(108, 0x1000);
    bytes.extend_from_slice(rgba.as_raw());
    bytes
}

#[tauri::command]
fn copy_png(png_base64: String, strip_alpha: bool) -> Result<(), String> {
    let image = clipboard_copy_pixels(&png_base64, strip_alpha)?;
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("Could not open the clipboard: {error}"))?;
    clipboard.set_image(arboard::ImageData {
        width: image.width() as usize,
        height: image.height() as usize,
        bytes: std::borrow::Cow::Borrowed(image.as_raw()),
    }).map_err(|error| format!("Could not copy the image: {error}"))
}

fn clipboard_copy_pixels(png_base64: &str, strip_alpha: bool) -> Result<image::RgbaImage, String> {
    let bytes = STANDARD.decode(
        png_base64.strip_prefix("data:image/png;base64,").unwrap_or(&png_base64)
    ).map_err(|error| format!("Clipboard PNG base64 failed: {error}"))?;
    let mut image = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| format!("Clipboard PNG decode failed: {error}"))?.to_rgba8();
    if strip_alpha {
        for pixel in image.pixels_mut() {
            pixel[3] = 255;
        }
    }
    Ok(image)
}

#[tauri::command]
fn import_clipboard_image() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("Could not open the clipboard: {error}"))?;
    let image = clipboard.get_image()
        .map_err(|error| format!("Could not read a clipboard image. Copy image pixels in your editor first: {error}"))?;
    clipboard_image_png(image)
}

fn clipboard_image_png(image: arboard::ImageData<'_>) -> Result<String, String> {
    let width = u32::try_from(image.width).map_err(|_| "Clipboard image width is too large.")?;
    let height = u32::try_from(image.height).map_err(|_| "Clipboard image height is too large.")?;
    if width == 0 || height == 0 {
        return Err("Clipboard image dimensions must be nonzero.".into());
    }
    let rgba = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .ok_or("Clipboard image pixel data does not match its dimensions.")?;
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(rgba).write_to(&mut png, ImageFormat::Png)
        .map_err(|error| format!("Clipboard image PNG encoding failed: {error}"))?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner())))
}

#[tauri::command]
fn texture_rgb_preview(png_base64: String) -> Result<String, String> {
    let bytes = STANDARD.decode(
        png_base64.strip_prefix("data:image/png;base64,").unwrap_or(&png_base64)
    ).map_err(|error| format!("Texture preview base64 failed: {error}"))?;
    let mut rgb = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| format!("Texture preview PNG failed: {error}"))?.to_rgba8();
    // Source often uses base-texture alpha for shader masks, not transparency.
    // Strip it before browser compositing, which otherwise loses hidden RGB.
    for pixel in rgb.pixels_mut() {
        pixel[3] = 255;
    }
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(rgb).write_to(&mut png, ImageFormat::Png)
        .map_err(|error| format!("Texture preview encoding failed: {error}"))?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner())))
}

#[tauri::command]
fn import_image(data_base64: String) -> Result<String, String> {
    let encoded = data_base64
        .split_once(',')
        .map(|(_, data)| data)
        .unwrap_or(&data_base64);
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|error| error.to_string())?;
    let image = if bytes.starts_with(b"VTF\0") {
        textures::decode_texture(&bytes)?
    } else if bytes.starts_with(b"DDS ") {
        decode_dds(&bytes)?
    } else {
        image::load_from_memory_with_format(&bytes, ImageFormat::Png)
            .map_err(|error| error.to_string())?
    };
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(png.into_inner())
    ))
}

#[tauri::command]
fn save_dds(name: String, png_base64: String) -> Result<Option<String>, String> {
    let bytes = STANDARD
        .decode(
            png_base64
                .strip_prefix("data:image/png;base64,")
                .unwrap_or(&png_base64),
        )
        .map_err(|error| error.to_string())?;
    let image = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    let filename = format!(
        "{}.dds",
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            })
            .collect::<String>()
    );
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(&filename)
        .add_filter("DDS image", &["dds"])
        .save_file()
    else {
        return Ok(None);
    };
    if path.exists() {
        return Err(
            "A file already exists at that path. Choose a new name to keep the existing file."
                .into(),
        );
    }
    fs::write(&path, encode_dds(&image)).map_err(|error| error.to_string())?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
fn export_texture(
    material_path: String,
    vmt_path: String,
    vmt_text: String,
    png_base64: String,
    strip_alpha: Option<bool>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let material_path = key(&material_path);
    let vmt_path = key(&vmt_path);
    if !is_skin_material_path(&material_path)
        || !is_skin_material_path(&vmt_path)
    {
        return Err("Unsupported weapon material path; reticles are excluded by PERPHEADS.".into());
    }
    if !material_path.ends_with(".vtf") || !vmt_path.ends_with(".vmt") {
        return Err("Invalid material path".into());
    }
    let bytes = STANDARD
        .decode(
            png_base64
                .strip_prefix("data:image/png;base64,")
                .unwrap_or(&png_base64),
        )
        .map_err(|error| error.to_string())?;
    let png = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    let vtf = encode_skin_vtf(&png, strip_alpha.unwrap_or(false))?;
    let install = state
        .0
        .lock()
        .map_err(|_| "Index lock failed")?
        .install_root
        .clone()
        .or_else(|| {
            rfd::FileDialog::new()
                .set_title("Choose the garrysmod folder for export")
                .pick_folder()
        })
        .ok_or("Export cancelled")?;
    let result = write_custom_skin(&install, &material_path, &vmt_path, &vmt_text, &vtf)?;
    state
        .0
        .lock()
        .map_err(|_| "Index lock failed")?
        .install_root = Some(install);
    Ok(result)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkinExport {
    material_path: String,
    vmt_path: String,
    vmt_text: String,
    png_base64: String,
    #[serde(default)]
    strip_alpha: bool,
}

#[tauri::command]
fn export_textures(skins: Vec<SkinExport>, state: State<'_, AppState>) -> Result<usize, String> {
    if skins.is_empty() {
        return Err("No edited textures to install".into());
    }
    let install = state
        .0
        .lock()
        .map_err(|_| "Index lock failed")?
        .install_root
        .clone()
        .or_else(|| {
            rfd::FileDialog::new()
                .set_title("Choose the garrysmod folder for export")
                .pick_folder()
        })
        .ok_or("Export cancelled")?;
    if !install.join("addons").is_dir() || !install.join("data").is_dir() {
        return Err("Choose a Garry's Mod garrysmod folder containing addons and data.".into());
    }
    let mut prepared = Vec::new();
    let mut destinations = std::collections::HashSet::new();
    let mut shared_textures = HashMap::new();
    for skin in skins {
        let material_path = key(&skin.material_path);
        let vmt_path = key(&skin.vmt_path);
        if !is_skin_material_path(&material_path)
            || !is_skin_material_path(&vmt_path)
            || !material_path.ends_with(".vtf")
            || !vmt_path.ends_with(".vmt")
        {
            return Err(format!("Guide does not support this material: {vmt_path}"));
        }
        let vtf_target = install.join("addons/skins").join(format!(
            "{}_custom.vtf",
            material_path.trim_end_matches(".vtf")
        ));
        let vmt_target = install
            .join("data/phcustomtextures")
            .join(vmt_path.trim_start_matches("materials/"));
        if vtf_target.exists()
            || vmt_target.exists()
            || !destinations.insert(vmt_target.clone())
        {
            return Err(format!(
                "A custom skin already exists or is duplicated: {vmt_path}"
            ));
        }
        let texture_re = Regex::new(r#"(?im)^\s*"?\$basetexture"?\s+(?:"[^"\r\n]+"|[^"\s]+)"#).unwrap();
        if !texture_re.is_match(&skin.vmt_text) {
            return Err(format!("VMT has no $basetexture reference: {vmt_path}"));
        }
        let bytes = STANDARD
            .decode(
                skin.png_base64
                    .strip_prefix("data:image/png;base64,")
                    .unwrap_or(&skin.png_base64),
            )
            .map_err(|error| error.to_string())?;
        let png = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        let vtf = encode_skin_vtf(&png, skin.strip_alpha)?;
        if let Some(previous) = shared_textures.insert(vtf_target, vtf.clone()) {
            if previous != vtf {
                return Err("Edited materials sharing a VTF have conflicting texture pixels".into());
            }
        }
        prepared.push((material_path, vmt_path, skin.vmt_text, vtf));
    }
    let mut written = Vec::new();
    for (material_path, vmt_path, vmt_text, vtf) in &prepared {
        let vtf_target = install.join("addons/skins").join(format!(
            "{}_custom.vtf", material_path.trim_end_matches(".vtf")));
        let texture_already_written = written.contains(&vtf_target);
        match write_custom_skin_shared(&install, material_path, vmt_path, vmt_text, vtf, texture_already_written) {
            Ok(_) => {
                if !texture_already_written { written.push(vtf_target); }
                written.push(
                    install
                        .join("data/phcustomtextures")
                        .join(vmt_path.trim_start_matches("materials/")),
                );
            }
            Err(error) => {
                for path in written {
                    let _ = fs::remove_file(path);
                }
                return Err(error);
            }
        }
    }
    state
        .0
        .lock()
        .map_err(|_| "Index lock failed")?
        .install_root = Some(install);
    Ok(prepared.len())
}

fn write_custom_skin(
    install: &Path,
    material_path: &str,
    vmt_path: &str,
    vmt_text: &str,
    vtf: &[u8],
) -> Result<String, String> {
    write_custom_skin_shared(install, material_path, vmt_path, vmt_text, vtf, false)
}

fn write_custom_skin_shared(
    install: &Path,
    material_path: &str,
    vmt_path: &str,
    vmt_text: &str,
    vtf: &[u8],
    shared_texture_written: bool,
) -> Result<String, String> {
    if !install.join("addons").is_dir() || !install.join("data").is_dir() {
        return Err("Choose a Garry's Mod garrysmod folder containing addons and data.".into());
    }
    let stem = material_path.trim_end_matches(".vtf");
    let custom_path = format!("{stem}_custom.vtf");
    let texture_reference = custom_path
        .trim_start_matches("materials/")
        .trim_end_matches(".vtf");
    let texture_re = Regex::new(r#"(?im)^(\s*"?\$basetexture"?\s+)(?:"[^"\r\n]+"|[^"\s]+)"#).unwrap();
    if !texture_re.is_match(&vmt_text) {
        return Err("VMT has no $basetexture reference to update.".into());
    }
    let custom_vmt = texture_re.replace(&vmt_text, |captures: &regex::Captures| {
        format!("{}\"{}\"", &captures[1], texture_reference)
    });
    let vtf_target = install.join("addons/skins").join(&custom_path);
    let vmt_target = install
        .join("data/phcustomtextures")
        .join(vmt_path.trim_start_matches("materials/"));
    if (vtf_target.exists() && !shared_texture_written) || vmt_target.exists() {
        return Err(
            "A custom skin already exists at this path. Move or rename it before exporting again."
                .into(),
        );
    }
    fs::create_dir_all(vtf_target.parent().ok_or("Invalid VTF path")?)
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(vmt_target.parent().ok_or("Invalid VMT path")?)
        .map_err(|error| error.to_string())?;
    if shared_texture_written {
        if fs::read(&vtf_target).map_err(|error| error.to_string())? != vtf {
            return Err("Shared installed texture changed during export".into());
        }
    } else {
        fs::write(&vtf_target, vtf).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::write(&vmt_target, custom_vmt.as_bytes()) {
        if !shared_texture_written { let _ = fs::remove_file(&vtf_target); }
        return Err(error.to_string());
    }
    Ok(format!(
        "VTF: {}\nVMT: {}",
        vtf_target.display(),
        vmt_target.display()
    ))
}

#[tauri::command]
fn save_png(name: String, png_base64: String) -> Result<Option<String>, String> {
    let filename = format!(
        "{}.png",
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            })
            .collect::<String>()
    );
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(&filename)
        .add_filter("PNG image", &["png"])
        .save_file()
    else {
        return Ok(None);
    };
    if path.exists() {
        return Err(
            "A file already exists at that path. Choose a new name to keep the existing file."
                .into(),
        );
    }
    let bytes = STANDARD
        .decode(
            png_base64
                .strip_prefix("data:image/png;base64,")
                .unwrap_or(&png_base64),
        )
        .map_err(|error| error.to_string())?;
    image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(Some(path.display().to_string()))
}

fn main() {
    let minimized = std::env::args_os().any(|arg| arg == "--minimized");
    let mut context = tauri::generate_context!();
    if minimized {
        for window in &mut context.config_mut().app.windows {
            window.visible = false;
            window.focus = false;
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::UpdateState::default())
        .manage(workshop::WorkshopState::default())
        .manage(AppState::default())
        .setup(move |app| {
            if minimized {
                let window = app.get_webview_window("main").ok_or("Main window not available")?;
                window.show()?;
                window.minimize()?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            workshop::workshop_list, workshop::workshop_signin,
            workshop::workshop_account, workshop::workshop_signout, workshop::workshop_stage,
            workshop::workshop_pending, workshop::workshop_discard, workshop::workshop_publish, workshop::workshop_preview,
            workshop::workshop_download, workshop::workshop_moderate, workshop::workshop_profile,
            workshop::workshop_submissions,
            workshop::workshop_bans,
            updates::check_update,
            updates::update_log,
            updates::install_update,
            choose_content_folder,
            scan_content,
            open_model,
            export_texture,
            list_custom_skins,
            delete_custom_skin,
            save_png,
            copy_png,
            import_clipboard_image,
            import_image,
            texture_rgb_preview,
            texture_has_alpha,
            save_dds,
            export_textures,
            drafts::save_draft,
            drafts::prepare_draft_output,
            drafts::list_drafts,
            drafts::load_draft,
            drafts::delete_draft,
            drafts::uninstall_draft,
            drafts::archive_custom_skin,
            drafts::installed_skin_models,
            drafts::edit_installed_skin,
            drafts::export_draft,
            drafts::export_custom_skin,
            drafts::import_skin,
            drafts::install_draft,
            textures::save_texture_variant,
            textures::rename_texture_variant,
            textures::list_texture_variants,
            textures::load_texture_variant,
            textures::delete_texture_variant,
            shared_texture::combine_shared_texture
        ])
        .run(context)
        .expect("failed to run texture helper");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment_data() -> serde_json::Value {
        let text = include_str!("..\\..\\ui\\attachment-data.js");
        serde_json::from_str(text.split_once(" = ").unwrap().1.trim().trim_end_matches(';')).unwrap()
    }

    #[test]
    fn all_lua_loadouts_have_idle_poses_and_matching_mount_bones() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else { return };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        let data = attachment_data();
        let mut parts = HashMap::new();
        for (path, weapon) in data["weapons"].as_object().unwrap() {
            let gun = build_model(&index, path).unwrap_or_else(|error| panic!("{path}: {error}"));
            assert_eq!(gun.sequence.as_deref(), Some("idle"), "{path}");
            let names = weapon["compatible"].as_array().unwrap().iter()
                .chain(weapon["defaults"].as_array().unwrap())
                .chain(weapon["requirements"].as_object().unwrap().values()
                    .flat_map(|names| names.as_array().unwrap()));
            for name in names {
                let path = data["attachments"][name.as_str().unwrap()]["model"].as_str().unwrap();
                let part = parts.entry(path).or_insert_with(|| build_model(&index, path).unwrap());
                assert!(gun.bones.iter().any(|bone| bone.name.eq_ignore_ascii_case(&part.bones[0].name)),
                    "{} has no mount bone for {path}", gun.path);
            }
        }
    }

    #[test]
    #[ignore = "Writes real model JSON fixtures for headless UI tests"]
    fn dump_preview_fixtures() {
        let root = std::env::var("PERP_TEST_CONTENT").expect("Set PERP_TEST_CONTENT");
        let output = PathBuf::from(std::env::var("PERP_PREVIEW_FIXTURES").expect("Set PERP_PREVIEW_FIXTURES"));
        fs::create_dir_all(&output).unwrap();
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        let data = attachment_data();
        let mut paths: Vec<_> = data["weapons"].as_object().unwrap().keys().map(String::as_str).collect();
        paths.extend(data["attachments"].as_object().unwrap().values()
            .map(|part| part["model"].as_str().unwrap()));
        paths.sort_unstable();
        paths.dedup();
        for path in paths {
            let detail = build_model(&index, path).unwrap();
            let file = output.join(format!("{}.json", path.replace('/', "__")));
            fs::write(file, serde_json::to_vec(&detail).unwrap()).unwrap();
        }
        for part in data["attachments"].as_object().unwrap().values().filter(|part| part["sequences"].is_object()) {
            let path = part["model"].as_str().unwrap();
            for sequence in [0, 1] {
                let detail = build_posed_model(&index, path, Some(sequence)).unwrap();
                let file = output.join(format!("{}__seq_{sequence}.json", path.replace('/', "__")));
                fs::write(file, serde_json::to_vec(&detail).unwrap()).unwrap();
            }
        }
    }

        #[test]
        #[ignore = "Regenerates public workshop path/dimension allowlist from installed content"]
        fn generate_workshop_catalog() {
            let root = std::env::var("PERP_TEST_CONTENT").expect("Set PERP_TEST_CONTENT");
            let mut index = ContentIndex::default();
            collect_loose(Path::new(&root), &mut index.files);
            let data = attachment_data();
            let mut models = serde_json::Map::new();
            let mut materials = serde_json::Map::new();
            let mut paths: Vec<String> = data["weapons"].as_object().unwrap().keys().cloned().collect();
            paths.extend(data["attachments"].as_object().unwrap().values()
                .map(|part| part["model"].as_str().unwrap().to_owned()));
            paths.extend(compatible_models(&index.files).into_iter()
                .filter_map(|(path, compatible)| compatible.then_some(path)));
            paths.sort(); paths.dedup();
            for path in paths {
                let detail = build_model(&index, &path).unwrap();
                let mut slots: std::collections::BTreeMap<String, Vec<serde_json::Value>> = std::collections::BTreeMap::new();
                if let Some(weapon) = data["weapons"].get(&path) {
                    for name in weapon["compatible"].as_array().unwrap().iter().chain(weapon["defaults"].as_array().unwrap()) {
                        let slot = data["attachments"][name.as_str().unwrap()]["slot"].as_str().unwrap().to_owned();
                        slots.entry(slot).or_default().push(name.clone());
                    }
                }
                let mut parts = std::collections::BTreeSet::from([path.clone()]);
                if let Some(weapon) = data["weapons"].get(&path) {
                    let mut names: Vec<_> = weapon["compatible"].as_array().unwrap().iter()
                        .chain(weapon["defaults"].as_array().unwrap()).collect();
                    if let Some(requirements) = weapon["requirements"].as_object() {
                        names.extend(requirements.values().flat_map(|parts| parts.as_array().unwrap()));
                    }
                    for name in names {
                        parts.insert(data["attachments"][name.as_str().unwrap()]["model"].as_str().unwrap().to_owned());
                    }
                }
                models.insert(path.clone(), serde_json::json!({"slots":slots,"parts":parts}));
                for material in &detail.materials {
                    if let (Some(vmt), Some(vtf)) = (&material.vmt_path, &material.vtf_path) {
                        if is_skin_material_path(vmt) && is_skin_material_path(vtf)
                            && material.width.is_some_and(|width| width > 0) && material.height.is_some_and(|height| height > 0) {
                            materials.insert(format!("{path}|{vmt}"), serde_json::json!({
                                "vtf":vtf, "width":material.width, "height":material.height
                            }));
                        }
                    }
                }
            }
            fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("workshop").join("src").join("catalog.json"),
                serde_json::to_vec_pretty(&serde_json::json!({"models":models,"materials":materials})).unwrap()).unwrap();
        }
    #[test]
    fn clipboard_image_conversion_preserves_rgba_and_dimensions() {
        let pixels = vec![210, 100, 30, 0, 40, 50, 60, 32, 90, 80, 70, 255, 1, 2, 3, 128];
        let png = clipboard_image_png(arboard::ImageData {
            width: 2, height: 2, bytes: std::borrow::Cow::Borrowed(&pixels),
        }).unwrap();
        let bytes = STANDARD.decode(png.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
        let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::Png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(decoded.into_raw(), pixels);
        let opaque = clipboard_copy_pixels(&png, true).unwrap();
        assert_eq!(opaque.dimensions(), (2, 2));
        for (original, copied) in pixels.chunks_exact(4).zip(opaque.pixels()) {
            assert_eq!(&copied.0[..3], &original[..3]);
            assert_eq!(copied[3], 255);
        }
        assert_eq!(clipboard_copy_pixels(&png, false).unwrap().into_raw(), pixels);
        for (width, height, data) in [(0, 1, vec![]), (2, 2, vec![0; 3]), (usize::MAX, 1, vec![])] {
            assert!(clipboard_image_png(arboard::ImageData {
                width, height, bytes: std::borrow::Cow::Owned(data),
            }).is_err());
        }
    }

    #[test]
    fn clipboard_rejects_invalid_images_before_opening_clipboard() {
        assert!(copy_png("not base64!".into(), true).unwrap_err().contains("base64 failed"));
        assert!(copy_png(STANDARD.encode(b"not a PNG"), true).unwrap_err().contains("PNG decode failed"));
        assert!(copy_png(format!("data:image/png;base64,{}", STANDARD.encode(b"not a PNG")), false)
            .unwrap_err().contains("PNG decode failed"));
    }

    #[test]
    fn ak74u_receiver_rgb_preview_reveals_shader_masked_texture() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else { return };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        let gun = build_model(&index, "models/perpheads/weapons/c_ak74u.mdl").unwrap();
        let rec = gun.materials.iter().find(|material|
            material.vmt_path.as_deref() == Some("materials/models/weapons/v_models/ak74/rec.vmt")).unwrap();
        let source = rec.image.as_ref().unwrap();
        let decode = |url: &str| image::load_from_memory(
            &STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap()
        ).unwrap().to_rgba8();
        let original = decode(source);
        let masked = original.pixels().filter(|pixel| pixel[3] < 128).count();
        println!("AK74U Rec: {masked}/{} pixels have alpha below 128; VMT: {}",
            original.width() * original.height(), rec.vmt_text.as_deref().unwrap());
        assert!(masked > 0);
        let preview = decode(&texture_rgb_preview(source.clone()).unwrap());
        assert!(preview.pixels().all(|pixel| pixel[3] == 255));
        for (before, after) in original.pixels().zip(preview.pixels()) {
            assert_eq!(&before.0[..3], &after.0[..3]);
        }
    }

    #[test]
    fn base_texture_paths_normalize_extensions_and_source_slashes() {
        for reference in [
            r"models\\weapons\v_models\m3\map2.vmt",
            "MODELS/WEAPONS/V_MODELS/M3/MAP2.VTF",
            "materials/models/weapons/v_models/m3/map2",
        ] {
            assert_eq!(base_texture_path(reference), "materials/models/weapons/v_models/m3/map2.vtf");
        }
        assert_eq!(base_texture_path(r"models\Jarheads\Weapons\microdot\LaRue ECOS"),
            "materials/models/jarheads/weapons/microdot/larue ecos.vtf");
    }

    #[test]
    fn actual_weapon_textures_with_extensions_and_spaces_resolve_and_install() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else { return };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        for stem in [
            "models/weapons/v_models/m3/map2",
            "models/weapons/v_models/m3/map1",
            "models/weapons/v_models/m3/12gauge",
            "models/weapons/v_models/sw500/saw500_diffuse",
            "models/weapons/v_models/m14/m14scoperail",
            "models/jarheads/weapons/attachments/rifle_sights/microdot/larue ecos",
        ] {
            let material = material_info(&index, stem, &[]);
            assert_eq!(material.vtf_path.as_deref(), Some(format!("materials/{stem}.vtf").as_str()), "{stem}");
            assert!(material.image.is_some(), "No decoded texture for {stem}");
            let install = tempfile::tempdir().unwrap();
            fs::create_dir(install.path().join("addons")).unwrap();
            fs::create_dir(install.path().join("data")).unwrap();
            write_custom_skin(install.path(), material.vtf_path.as_deref().unwrap(),
                material.vmt_path.as_deref().unwrap(), material.vmt_text.as_deref().unwrap(), b"test texture").unwrap();
            let text = fs::read_to_string(install.path().join("data").join("phcustomtextures").join(format!("{stem}.vmt"))).unwrap();
            assert!(text.contains(&format!("\"{stem}_custom\"")), "{text}");
            assert!(!text.contains(" ECOS\""), "Old quoted path was only partly replaced");
            let inventory = custom_skin_inventory(install.path());
            assert!(inventory.iter().any(|skin| skin.vmt_path.is_some() && skin.has_vtf), "{stem}");
        }
        let gun = build_model(&index, "models/perpheads/weapons/c_870.mdl").unwrap();
        assert!(gun.materials.iter().any(|material| material.name.eq_ignore_ascii_case("map2") && material.image.is_some()));
    }

    #[test]
    fn skin_material_paths_include_jarheads_but_not_reticles_or_unsafe_paths() {
        assert!(is_skin_material_path("materials/models/jarheads/weapons/attachments/rifle_sights/flip/sights.vmt"));
        assert!(is_skin_material_path("materials/models/weapons/v_models/ak74/rec.vtf"));
        for path in [
            "materials/models/jarheads/props/body.vmt",
            "materials/models/jarheads/weapons/attachments/rifle_sights/reticles/eotech.vmt",
            "materials/models/jarheads/weapons/../props/body.vmt",
            "materials/models/jarheads/weapons//body.vmt",
            "materials/models/jarheads/weapons/C:/body.vmt",
        ] { assert!(!is_skin_material_path(path), "{path}"); }
    }

    #[test]
    fn actual_jarheads_sight_materials_install_to_exact_game_paths() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else { return };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        let sight = build_model(&index, "models/perpheads/weapons/attachments/rifle_eotech.mdl").unwrap();
        let material = sight.materials.iter().find(|material|
            material.vmt_path.as_deref().is_some_and(|p| p.starts_with("materials/models/jarheads/") && is_skin_material_path(p))).unwrap();
        let vmt = material.vmt_path.as_deref().unwrap();
        let vtf = material.vtf_path.as_deref().unwrap();
        assert!(is_skin_material_path(vtf));
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        write_custom_skin(install.path(), vtf, vmt, material.vmt_text.as_deref().unwrap(), b"test texture").unwrap();
        let target = install.path().join("data").join("phcustomtextures").join(vmt.trim_start_matches("materials/"));
        assert!(target.is_file());
        assert!(fs::read_to_string(target).unwrap().contains(
            &format!("{}_custom", vtf.trim_start_matches("materials/").trim_end_matches(".vtf"))));
    }

    #[test]
    fn scar_flip_sights_keep_materials_when_folded_and_install_every_material() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else { return };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        let install = tempfile::tempdir().unwrap();
        fs::create_dir(install.path().join("addons")).unwrap();
        fs::create_dir(install.path().join("data")).unwrap();
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 4, image::Rgba([190, 40, 60, 255])));
        let bytes = encode_skin_vtf(&image, false).unwrap();
        let mut installed = std::collections::HashSet::new();
        let mut textures = std::collections::HashSet::new();
        for path in ["models/perpheads/weapons/attachments/rifle_flip_front.mdl",
            "models/perpheads/weapons/attachments/rifle_flip_rear.mdl"] {
            let up = build_posed_model(&index, path, Some(0)).unwrap();
            let down = build_posed_model(&index, path, Some(1)).unwrap();
            assert!(is_skin_target_model(path));
            assert!(!down.materials.is_empty());
            assert_eq!(up.materials.len(), down.materials.len());
            for (up, down) in up.materials.iter().zip(&down.materials) {
                assert_eq!(up.vmt_path, down.vmt_path);
                assert_eq!(up.vtf_path, down.vtf_path);
                let vmt = down.vmt_path.as_deref().unwrap();
                let vtf = down.vtf_path.as_deref().unwrap();
                assert!(is_skin_material_path(vmt), "{vmt}");
                assert!(is_skin_material_path(vtf), "{vtf}");
                assert!(down.image.is_some());
                if installed.insert(vmt.to_owned()) {
                    let shared = !textures.insert(vtf.to_owned());
                    write_custom_skin_shared(install.path(), vtf, vmt, down.vmt_text.as_deref().unwrap(), &bytes, shared).unwrap();
                }
                let target = install.path().join("data").join("phcustomtextures").join(vmt.trim_start_matches("materials/"));
                assert!(fs::read_to_string(target).unwrap().contains(&format!("{}_custom",
                    vtf.trim_start_matches("materials/").trim_end_matches(".vtf"))));
            }
        }
        assert!(installed.len() >= 2, "Both sight body and sight details must be installed");
        assert!(custom_skin_inventory(install.path()).iter().all(|skin| skin.has_vtf));
    }

    #[test]
    fn alpha_install_encoding_preserves_rgb_and_keeps_alpha_by_default() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 4, image::Rgba([70, 90, 130, 0])));
        let keep = vtf_decode::decode_highres(&encode_skin_vtf(&image, false).unwrap()).unwrap().to_rgba8();
        let remove = vtf_decode::decode_highres(&encode_skin_vtf(&image, true).unwrap()).unwrap().to_rgba8();
        assert!(keep.pixels().all(|p| p.0 == [70, 90, 130, 0]));
        assert!(remove.pixels().all(|p| p.0 == [70, 90, 130, 255]));
        assert!(image.to_rgba8().pixels().all(|p| p[3] == 0));
    }

    #[test]
    fn texture_rgb_preview_preserves_hidden_color_and_original_alpha() {
        let original = image::RgbaImage::from_fn(4, 1, |x, _|
            image::Rgba([73, 149, 211, [0, 1, 128, 255][x as usize]]));
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(original.clone()).write_to(&mut png, ImageFormat::Png).unwrap();
        let source = format!("data:image/png;base64,{}", STANDARD.encode(png.into_inner()));
        let preview = texture_rgb_preview(source.clone()).unwrap();
        let read = |url: &str| image::load_from_memory_with_format(
            &STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap(),
            ImageFormat::Png).unwrap().to_rgba8();
        assert!(read(&preview).pixels().all(|pixel| pixel.0 == [73, 149, 211, 255]));
        assert_eq!(read(&source), original);
        assert!(texture_rgb_preview("invalid".into()).is_err());
    }

    #[test]
    fn vtf_export_decodes_with_original_pixels() {
        let mut image = image::RgbaImage::new(8, 8);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8 * 20, y as u8 * 20, 90, 128]);
        }
        let encoded = encode_vtf(&DynamicImage::ImageRgba8(image.clone())).unwrap();
        let decoded = vtf_decode::decode_highres(&encoded).unwrap().to_rgba8();
        assert_eq!(decoded, image);
    }

    #[test]
    fn dds_export_roundtrips_rgba_pixels() {
        let mut image = image::RgbaImage::new(8, 4);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8 * 27, y as u8 * 71, 123, (x * 31) as u8]);
        }
        let dds = encode_dds(&DynamicImage::ImageRgba8(image.clone()));
        assert_eq!(decode_dds(&dds).unwrap().to_rgba8(), image);
    }

    #[test]
    fn compensator_has_mesh_and_material() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else {
            return;
        };
        let mut index = ContentIndex::default();
        collect_loose(Path::new(&root), &mut index.files);
        let detail = build_model(
            &index,
            "models/perpheads/weapons/attachments/rifle_compensator.mdl",
        )
        .unwrap();
        assert_eq!(detail.triangle_count, 170);
        assert!(
            detail
                .materials
                .iter()
                .any(|material| material.vtf_path.as_deref()
                    == Some("materials/models/weapons/v_models/g36c/c_grip.vtf"))
        );
        assert_eq!(
            detail
                .meshes
                .iter()
                .map(|mesh| mesh.uvs.len() / 6)
                .sum::<usize>(),
            170
        );
    }

    #[test]
    fn extracted_scarl_exposes_mounts_without_guessing_compatible_parts() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else {
            return;
        };
        let (index, result) = build_index(&root).unwrap();
        assert!(
            result
                .models
                .iter()
                .any(|item| item.path == "models/perpheads/weapons/c_scarl.mdl")
        );
        let detail = build_model(&index, "models/perpheads/weapons/c_scarl.mdl").unwrap();
        assert!(
            detail
                .attachments
                .iter()
                .any(|attachment| attachment.name == "stock"
                    && attachment.transform.iter().all(|value| value.is_finite()))
        );
        assert!(detail.related_models.is_empty());
        assert!(
            result
                .models
                .iter()
                .find(|item| item.path == "models/perpheads/weapons/c_scarl.mdl")
                .unwrap()
                .compatible
        );
        assert!(
            result
                .models
                .iter()
                .find(|item| item.path == "models/perpheads/weapons/attachments/stock_scarl.mdl")
                .unwrap()
                .compatible
        );
    }

    #[test]
    fn ak101_stock_has_distinct_local_bone_basis() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else {
            return;
        };
        let (index, _) = build_index(&root).unwrap();
        let gun = build_model(&index, "models/perpheads/weapons/c_ak101.mdl").unwrap();
        let stock = build_model(
            &index,
            "models/perpheads/weapons/attachments/stock_ak101.mdl",
        )
        .unwrap();
        assert!(gun.attachments.iter().any(|item| item.name == "stock"));
        let basis = stock.root_bone_transform.unwrap();
        assert!(basis.iter().all(|value| value.is_finite()));
        assert!((basis[12] - 0.166317).abs() < 0.01);
    }

    fn mounted_y_range(gun: &ModelDetail, part: &ModelDetail) -> (f32, f32) {
        let root = &part.bones[0];
        let target = gun.bones.iter().find(|bone| bone.name == root.name).unwrap();
        let mount = multiply_matrix(&target.transform, &invert_rigid(&root.transform));
        part.meshes
            .iter()
            .flat_map(|mesh| mesh.positions.chunks(3))
            .map(|p| mount[1] * p[0] + mount[5] * p[1] + mount[9] * p[2] + mount[13])
            .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)))
    }

    #[test]
    fn ak101_parts_bonemerge_flush_with_receiver() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else {
            return;
        };
        let (index, _) = build_index(&root).unwrap();
        let gun = build_model(&index, "models/perpheads/weapons/c_ak101.mdl").unwrap();
        let gun_rear = gun
            .meshes
            .iter()
            .flat_map(|mesh| mesh.positions.chunks(3).map(|p| p[1]))
            .fold(f32::MIN, f32::max);
        let stock = build_model(
            &index,
            "models/perpheads/weapons/attachments/stock_ak101.mdl",
        )
        .unwrap();
        assert_eq!(stock.bones[0].name, "stock");
        let (stock_front, stock_back) = mounted_y_range(&gun, &stock);
        assert!((stock_front - gun_rear).abs() < 0.5, "stock starts at {stock_front}, receiver ends at {gun_rear}");
        assert!(stock_back > gun_rear + 5.0);
        let reddot = build_model(
            &index,
            "models/perpheads/weapons/attachments/rifle_reddot.mdl",
        )
        .unwrap();
        let (sight_front, sight_back) = mounted_y_range(&gun, &reddot);
        assert!(sight_front > -25.0 && sight_back < gun_rear);
    }

    fn mounted_bounds(gun: &ModelDetail, part: &ModelDetail) -> ([f32; 3], [f32; 3]) {
        let root = &part.bones[0];
        let target = gun.bones.iter().find(|bone| bone.name == root.name).unwrap();
        let mount = multiply_matrix(&target.transform, &invert_rigid(&root.transform));
        mesh_bounds(part, &mount)
    }

    fn mesh_bounds(model: &ModelDetail, matrix: &[f32; 16]) -> ([f32; 3], [f32; 3]) {
        model
            .meshes
            .iter()
            .flat_map(|mesh| mesh.positions.chunks(3))
            .map(|p| transform_point(matrix, [p[0], p[1], p[2]]))
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
                ([0, 1, 2].map(|i| lo[i].min(p[i])), [0, 1, 2].map(|i| hi[i].max(p[i])))
            })
    }

    const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

    #[test]
    fn g36_magazine_mounts_in_magwell_from_idle_pose() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else {
            return;
        };
        let (index, _) = build_index(&root).unwrap();
        let gun = build_model(&index, "models/perpheads/weapons/c_g36.mdl").unwrap();
        assert_eq!(gun.sequence.as_deref(), Some("idle"));
        let magazine = build_model(&index, "models/perpheads/weapons/magazines/55645_30_2.mdl").unwrap();
        assert_eq!(magazine.bones[0].name, "mag");
        let (gun_min, gun_max) = mesh_bounds(&gun, &IDENTITY);
        let (mag_min, mag_max) = mounted_bounds(&gun, &magazine);
        // The reference pose leaves the "mag" bone ~75 units below the gun; the idle frame seats it.
        assert!(mag_max[2] > gun_min[2] && mag_max[2] < gun_max[2], "magazine top {mag_max:?}, gun {gun_min:?}..{gun_max:?}");
        assert!(mag_min[1] > gun_min[1] && mag_max[1] < gun_max[1]);
        let frame = gun.bones.iter().find(|bone| bone.name == "frame").unwrap();
        assert!(((mag_min[0] + mag_max[0]) / 2.0 - frame.transform[12]).abs() < 1.0);
    }

    #[test]
    fn flip_sights_fold_in_their_down_sequence() {
        let Ok(root) = std::env::var("PERP_TEST_CONTENT") else {
            return;
        };
        let (index, _) = build_index(&root).unwrap();
        let gun = build_model(&index, "models/perpheads/weapons/c_m4a1.mdl").unwrap();
        for path in [
            "models/perpheads/weapons/attachments/rifle_flip_front.mdl",
            "models/perpheads/weapons/attachments/rifle_flip_rear.mdl",
        ] {
            let up = build_model(&index, path).unwrap();
            let down = build_posed_model(&index, path, Some(1)).unwrap();
            assert_eq!(up.sequence.as_deref(), Some("idle"));
            assert_eq!(down.sequence.as_deref(), Some("idle_down"));
            let (up_min, up_max) = mounted_bounds(&gun, &up);
            let (down_min, down_max) = mounted_bounds(&gun, &down);
            assert!(up_max[2] > down_max[2] + 0.8, "{path}: up top {}, down top {}", up_max[2], down_max[2]);
            assert!((up_min[2] - down_min[2]).abs() < 0.1, "{path}: base moved");
        }
    }

    #[test]
    fn multiple_materials_can_install_one_shared_texture_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("addons")).unwrap();
        fs::create_dir(root.path().join("data")).unwrap();
        let vtf = format!("{SKIN_MATERIAL_PREFIX}shared/wood.vtf");
        let body = format!("{SKIN_MATERIAL_PREFIX}shared/body.vmt");
        let stock = format!("{SKIN_MATERIAL_PREFIX}shared/stock.vmt");
        let text = "$basetexture \"shared/wood\"";
        write_custom_skin(root.path(), &vtf, &body, text, b"same pixels").unwrap();
        write_custom_skin_shared(root.path(), &vtf, &stock, text, b"same pixels", true).unwrap();
        assert_eq!(custom_skin_inventory(root.path()).len(), 2);
        assert!(write_custom_skin_shared(root.path(), &vtf,
            &format!("{SKIN_MATERIAL_PREFIX}shared/other.vmt"), text, b"different pixels", true).is_err());
        assert!(write_custom_skin(root.path(), &vtf,
            &format!("{SKIN_MATERIAL_PREFIX}shared/other.vmt"), text, b"same pixels").is_err());
    }

    #[test]
    fn guide_layout_renames_texture_and_rewrites_vmt() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("addons")).unwrap();
        fs::create_dir(root.path().join("data")).unwrap();
        let result = write_custom_skin(
            root.path(),
            "materials/models/weapons/v_models/g36c/c_grip.vtf",
            "materials/models/weapons/v_models/g36c/c_grip.vmt",
            "VertexLitGeneric\n{\n  \"$basetexture\" \"models/weapons/v_models/g36c/c_grip\"\n}",
            b"vtf data",
        )
        .unwrap();
        assert!(result.contains("c_grip_custom.vtf"));
        assert_eq!(
            fs::read(
                root.path()
                    .join("addons/skins/materials/models/weapons/v_models/g36c/c_grip_custom.vtf")
            )
            .unwrap(),
            b"vtf data"
        );
        let vmt = fs::read_to_string(
            root.path()
                .join("data/phcustomtextures/models/weapons/v_models/g36c/c_grip.vmt"),
        )
        .unwrap();
        assert!(vmt.contains("\"$basetexture\" \"models/weapons/v_models/g36c/c_grip_custom\""));
    }

    #[test]
    fn custom_skin_inventory_and_delete_preserve_shared_texture() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("addons")).unwrap();
        fs::create_dir(root.path().join("data")).unwrap();
        let base = "models/weapons/v_models/g36c/c_grip";
        let vmt_text = format!("VertexLitGeneric {{ \n\"$basetexture\" \"{base}\"\n }}");
        write_custom_skin(
            root.path(),
            &format!("materials/{base}.vtf"),
            &format!("materials/{base}.vmt"),
            &vmt_text,
            b"vtf",
        )
        .unwrap();
        let vtf_path = root
            .path()
            .join(format!("addons/skins/materials/{base}_custom.vtf"));
        let second_vmt = root
            .path()
            .join("data/phcustomtextures/models/weapons/v_models/g36c/second.vmt");
        fs::write(&second_vmt, format!("\"$basetexture\" \"materials/{base}_custom.vtf\"")).unwrap();
        let skins = custom_skin_inventory(root.path());
        assert_eq!(skins.len(), 2);
        assert!(skins.iter().all(|skin| skin.has_vtf));
        delete_skin_at(root.path(), &format!("vmt:{base}.vmt")).unwrap();
        assert!(vtf_path.exists());
        assert!(second_vmt.exists());
        assert!(
            delete_skin_at(root.path(), "vmt:models/weapons/v_models/g36c/second.vmt")
                .unwrap()
                .is_empty()
        );
        assert!(!vtf_path.exists());
    }

    #[test]
    fn loose_installed_skins_do_not_override_original_material_sources() {
        let original = tempfile::tempdir().unwrap();
        let loose = tempfile::tempdir().unwrap();
        let path = "materials/models/weapons/v_models/test/receiver.vtf";
        let source = original.path().join(path);
        let override_path = loose.path().join(path);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        fs::write(&source, b"original").unwrap();
        fs::write(&override_path, b"installed gold").unwrap();
        let mut files = HashMap::from([(path.into(), Source::Loose(source))]);
        collect_original_loose(loose.path(), loose.path(), &mut files);
        assert_eq!(files[path].read().unwrap(), b"original");
        let game = tempfile::tempdir().unwrap();
        let installed = game.path().join("addons").join("skins").join(path);
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(installed, b"installed gold").unwrap();
        let (index, _) = build_index(game.path().to_str().unwrap()).unwrap();
        assert!(!index.files.contains_key(path), "Installed skins must not enter the original-content index");
    }

    #[test]
    fn workshop_gma_reads_compensator() {
        let Ok(path) = std::env::var("PERP_TEST_GMA") else {
            return;
        };
        let archive = GmaArchive::open(&path).unwrap();
        assert!(
            ::gma::open(&path)
                .unwrap()
                .name()
                .to_ascii_lowercase()
                .contains("perpheads")
        );
        let model_path = "models/perpheads/weapons/attachments/rifle_compensator.mdl";
        assert!(
            archive
                .paths()
                .unwrap()
                .iter()
                .any(|path| key(path) == model_path)
        );
        assert!(archive.read(model_path).unwrap().starts_with(b"IDST"));
    }

    #[test]
    fn installed_content_index_finds_workshop_model() {
        let Ok(root) = std::env::var("PERP_TEST_GMOD") else {
            return;
        };
        let (index, result) = build_index(&root).unwrap();
        assert!(result.has_install_root);
        assert!(result.archive_count > 0);
        let item = result
            .models
            .iter()
            .find(|item| item.path == "models/perpheads/weapons/attachments/rifle_compensator.mdl")
            .unwrap();
        assert!(item.origin.ends_with("content.gma"));
        let detail = build_model(&index, &item.path).unwrap();
        assert_eq!(detail.triangle_count, 170);
        assert_eq!(detail.materials[0].width, Some(512));
        let police =
            build_model(&index, "models/perpheads/government/police/male_01a.mdl").unwrap();
        assert!(
            police
                .materials
                .iter()
                .any(|material| material.name == "mouth" && material.image.is_some())
        );
    }

    #[test]
    fn normalizes_repeated_material_separators() {
        assert_eq!(
            key("materials/models//weapons/v_models/scarl/body.vtf"),
            "materials/models/weapons/v_models/scarl/body.vtf"
        );
    }

    #[test]
    fn finished_weapon_and_attachment_targets_exclude_crafting_parts() {
        assert!(is_skin_target_model("models/perpheads/weapons/c_scarl.mdl"));
        assert!(is_skin_target_model(
            "models/perpheads/weapons/attachments/stock_scarl.mdl"
        ));
        assert!(is_skin_target_model(
            "models/perpheads/weapons/magazines/55645_30.mdl"
        ));
        assert!(!is_skin_target_model(
            "models/perpheads/weapons/components/scarl_barrel.mdl"
        ));
        assert!(!is_skin_target_model(
            "models/perpheads/weapons/w_scarl.mdl"
        ));
    }
}

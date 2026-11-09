use std::{fs, io, path::Path};

const STUDIO_VERTEX_SIZE: i32 = 48;
const BODY_PART_SIZE: usize = 16;
const MODEL_SIZE: usize = 148;
const MESH_SIZE: usize = 116;

#[derive(Debug, Clone)]
pub struct MdlHeader {
    pub id: i32,
    pub version: i32,
    pub checksum: i32,

    pub name: String,
    pub length: i32,

    pub texture_count: i32,
    pub texture_offset: i32,

    pub cd_texture_count: i32,
    pub cd_texture_offset: i32,

    pub skin_reference_count: i32,
    pub skin_family_count: i32,
    pub skin_offset: i32,

    pub body_part_count: i32,
    pub body_part_offset: i32,

    pub bone_count: i32,
    pub bone_offset: i32,
}

#[derive(Debug, Clone)]
pub struct MdlBone {
    pub index: usize,
    pub name: String,
    pub parent: i32,
    pub pose_to_bone: [f32; 12],
    pub default_position: [f32; 3],
    pub default_rotation: [f32; 4],
    pub default_euler: [f32; 3],
    pub position_scale: [f32; 3],
    pub rotation_scale: [f32; 3],
}

/// Parent-relative bone position and quaternion.
pub type BonePose = ([f32; 3], [f32; 4]);

#[derive(Debug, Clone)]
pub struct MdlSequence {
    pub label: String,
    pub activity: String,
    pub first_frame: Result<Vec<BonePose>, String>,
}

#[derive(Debug, Clone)]
pub struct MdlAttachment {
    pub name: String,
    pub bone: usize,
    pub local: [f32; 12],
}

#[derive(Debug, Clone)]
pub struct MdlTexture {
    pub index: usize,
    pub name: String,
    pub flags: i32,
}

#[derive(Debug, Clone)]
pub struct MdlMesh {
    pub index: usize,

    /*
        This is a SKIN TABLE COLUMN,
        not directly a texture index.
    */
    pub material: i32,

    pub vertex_count: i32,

    /*
        Vertex offset relative to the owning model,
        measured in vertices.
    */
    pub vertex_offset: i32,
}

#[derive(Debug, Clone)]
pub struct MdlModel {
    pub index: usize,
    pub name: String,

    pub mesh_count: i32,
    pub mesh_offset: i32,

    pub vertex_count: i32,

    /*
        Byte offset into the reconstructed VVD vertex pool.
    */
    pub vertex_index_bytes: i32,

    pub meshes: Vec<MdlMesh>,
}

#[derive(Debug, Clone)]
pub struct MdlBodyPart {
    pub index: usize,
    pub name: String,

    pub model_count: i32,
    pub base: i32,

    pub models: Vec<MdlModel>,
}

#[derive(Debug)]
pub struct MdlFile {
    pub header: MdlHeader,

    pub textures: Vec<MdlTexture>,

    pub cd_texture_paths: Vec<String>,

    /*
        [skin family][skin reference column]
        => texture index
    */
    pub skins: Vec<Vec<i16>>,

    pub body_parts: Vec<MdlBodyPart>,

    pub bones: Vec<MdlBone>,
    pub attachments: Vec<MdlAttachment>,
    pub sequences: Vec<MdlSequence>,
}

impl MdlFile {
    pub fn texture_for_material(&self, skin: usize, material_column: i32) -> Option<&MdlTexture> {
        if material_column < 0 {
            return None;
        }

        let row = self.skins.get(skin)?;

        let texture_index = *row.get(material_column as usize)?;

        if texture_index < 0 {
            return None;
        }

        self.textures.get(texture_index as usize)
    }
}

pub fn read_mdl<P>(path: P) -> io::Result<MdlFile>
where
    P: AsRef<Path>,
{
    let data = fs::read(path)?;
    read_mdl_bytes(&data)
}

pub fn read_mdl_bytes(data: &[u8]) -> io::Result<MdlFile> {
    if data.len() < 240 {
        return Err(invalid_data("MDL file is too small"));
    }

    let header = MdlHeader {
        id: read_i32(&data, 0)?,

        version: read_i32(&data, 4)?,

        checksum: read_i32(&data, 8)?,

        name: read_fixed_string(&data, 12, 64)?,

        length: read_i32(&data, 76)?,

        texture_count: read_i32(&data, 204)?,

        texture_offset: read_i32(&data, 208)?,

        cd_texture_count: read_i32(&data, 212)?,

        cd_texture_offset: read_i32(&data, 216)?,

        skin_reference_count: read_i32(&data, 220)?,

        skin_family_count: read_i32(&data, 224)?,

        skin_offset: read_i32(&data, 228)?,

        body_part_count: read_i32(&data, 232)?,

        body_part_offset: read_i32(&data, 236)?,

        bone_count: read_i32(&data, 156)?,

        bone_offset: read_i32(&data, 160)?,
    };

    let mut bones = Vec::new();
    for index in 0..header.bone_count.max(0) as usize {
        let offset = header.bone_offset as usize + index * 216;
        ensure_range(&data, offset, 156, "MDL bone")?;
        let name_offset = read_i32(&data, offset)?;
        let name = if name_offset > 0 {
            let absolute = (offset as i64 + name_offset as i64) as usize;
            read_c_string(&data, absolute)?
        } else {
            format!("bone_{index}")
        };
        let mut pose_to_bone = [0.0; 12];
        for (component, value) in pose_to_bone.iter_mut().enumerate() {
            *value = read_f32(&data, offset + 96 + component * 4)?;
        }
        bones.push(MdlBone {
            index,
            name,
            parent: read_i32(&data, offset + 4)?,
            pose_to_bone,
            default_position: [
                read_f32(data, offset + 32)?,
                read_f32(data, offset + 36)?,
                read_f32(data, offset + 40)?,
            ],
            default_rotation: [
                read_f32(data, offset + 44)?,
                read_f32(data, offset + 48)?,
                read_f32(data, offset + 52)?,
                read_f32(data, offset + 56)?,
            ],
            default_euler: read_vec3(data, offset + 60)?,
            position_scale: read_vec3(data, offset + 72)?,
            rotation_scale: read_vec3(data, offset + 84)?,
        });
    }

    let sequences = read_sequences(data, &bones)?;

    let mut attachments = Vec::new();
    let attachment_count = read_i32(data, 240)?.clamp(0, 256) as usize;
    let attachment_offset = read_i32(data, 244)?;
    if attachment_count > 0 && attachment_offset > 0 {
        for index in 0..attachment_count {
            let offset = attachment_offset as usize + index * 92;
            ensure_range(data, offset, 60, "MDL attachment")?;
            let name_relative = read_i32(data, offset)?;
            let name_offset =
                checked_relative_offset(offset, name_relative, data.len(), "MDL attachment name")?;
            let bone = read_i32(data, offset + 8)?;
            if bone < 0 || bone as usize >= bones.len() {
                continue;
            }
            let mut local = [0.0; 12];
            for (component, value) in local.iter_mut().enumerate() {
                *value = read_f32(data, offset + 12 + component * 4)?;
            }
            attachments.push(MdlAttachment {
                name: read_c_string(data, name_offset)?,
                bone: bone as usize,
                local,
            });
        }
    }

    /*
        ========================================================
        TEXTURES
        ========================================================
    */

    let mut textures = Vec::new();

    for index in 0..header.texture_count.max(0) as usize {
        let entry = header.texture_offset as usize + index * 64;

        ensure_range(&data, entry, 64, "MDL texture")?;

        let name_relative = read_i32(&data, entry)?;

        let name_offset =
            checked_relative_offset(entry, name_relative, data.len(), "MDL texture name")?;

        textures.push(MdlTexture {
            index,

            name: read_c_string(&data, name_offset)?,

            flags: read_i32(&data, entry + 4)?,
        });
    }

    /*
        ========================================================
        CD TEXTURE SEARCH PATHS
        ========================================================
    */

    let mut cd_texture_paths = Vec::new();

    for index in 0..header.cd_texture_count.max(0) as usize {
        let pointer = header.cd_texture_offset as usize + index * 4;

        let string_offset = read_i32(&data, pointer)?;

        if string_offset > 0 {
            cd_texture_paths.push(read_c_string(&data, string_offset as usize)?);
        }
    }

    /*
        ========================================================
        SKIN TABLE
        ========================================================
    */

    let mut skins = Vec::new();

    if header.skin_reference_count > 0 && header.skin_family_count > 0 && header.skin_offset > 0 {
        let columns = header.skin_reference_count as usize;

        let families = header.skin_family_count as usize;

        for family in 0..families {
            let mut row = Vec::with_capacity(columns);

            for column in 0..columns {
                let offset = header.skin_offset as usize + (family * columns + column) * 2;

                row.push(read_i16(&data, offset)?);
            }

            skins.push(row);
        }
    }

    /*
        ========================================================
        BODY PARTS -> MODELS -> MESHES
        ========================================================
    */

    let mut body_parts = Vec::new();

    for body_index in 0..header.body_part_count.max(0) as usize {
        let body_offset = header.body_part_offset as usize + body_index * BODY_PART_SIZE;

        ensure_range(&data, body_offset, BODY_PART_SIZE, "MDL bodypart")?;

        let body_name_relative = read_i32(&data, body_offset)?;

        let body_name_offset = checked_relative_offset(
            body_offset,
            body_name_relative,
            data.len(),
            "MDL bodypart name",
        )?;

        let model_count = read_i32(&data, body_offset + 4)?;

        let base = read_i32(&data, body_offset + 8)?;

        let model_array_relative = read_i32(&data, body_offset + 12)?;

        let model_array = checked_relative_offset(
            body_offset,
            model_array_relative,
            data.len(),
            "MDL model array",
        )?;

        let mut models = Vec::new();

        for model_index in 0..model_count.max(0) as usize {
            let model_offset = model_array + model_index * MODEL_SIZE;

            ensure_range(&data, model_offset, MODEL_SIZE, "MDL model")?;

            let name = read_fixed_string(&data, model_offset, 64)?;

            let mesh_count = read_i32(&data, model_offset + 72)?;

            let mesh_relative = read_i32(&data, model_offset + 76)?;

            let vertex_count = read_i32(&data, model_offset + 80)?;

            let vertex_index_bytes = read_i32(&data, model_offset + 84)?;

            let mesh_array =
                checked_relative_offset(model_offset, mesh_relative, data.len(), "MDL mesh array")?;

            let mut meshes = Vec::new();

            for mesh_index in 0..mesh_count.max(0) as usize {
                let mesh_offset = mesh_array + mesh_index * MESH_SIZE;

                ensure_range(&data, mesh_offset, MESH_SIZE, "MDL mesh")?;

                meshes.push(MdlMesh {
                    index: mesh_index,

                    material: read_i32(&data, mesh_offset)?,

                    vertex_count: read_i32(&data, mesh_offset + 8)?,

                    vertex_offset: read_i32(&data, mesh_offset + 12)?,
                });
            }

            /*
                Sanity check: model vertex index is bytes.
            */

            if vertex_index_bytes % STUDIO_VERTEX_SIZE != 0 {
                return Err(invalid_data(&format!(
                    "Model '{}' VVD vertex byte offset {} is not divisible by 48",
                    name, vertex_index_bytes
                )));
            }

            models.push(MdlModel {
                index: model_index,

                name,

                mesh_count,

                mesh_offset: mesh_relative,

                vertex_count,

                vertex_index_bytes,

                meshes,
            });
        }

        body_parts.push(MdlBodyPart {
            index: body_index,

            name: read_c_string(&data, body_name_offset)?,

            model_count,

            base,

            models,
        });
    }

    Ok(MdlFile {
        header,

        textures,

        cd_texture_paths,

        skins,

        body_parts,

        bones,
        attachments,
        sequences,
    })
}

/*
    ============================================================
    SEQUENCES / ANIMATION (first frame only)
    ============================================================
*/

const ANIM_DESC_SIZE: usize = 100;
const SEQ_DESC_SIZE: usize = 212;
const ANIM_RAWPOS: u8 = 0x01;
const ANIM_RAWROT: u8 = 0x02;
const ANIM_ANIMPOS: u8 = 0x04;
const ANIM_ANIMROT: u8 = 0x08;
const ANIM_DELTA: u8 = 0x10;
const ANIM_RAWROT2: u8 = 0x20;

fn read_sequences(data: &[u8], bones: &[MdlBone]) -> io::Result<Vec<MdlSequence>> {
    let (anim_count, anim_offset) = (read_i32(data, 180)?, read_i32(data, 184)?);
    let (sequence_count, sequence_offset) = (read_i32(data, 188)?, read_i32(data, 192)?);
    if anim_count < 0 || sequence_count < 0 {
        return Err(invalid_data("Negative MDL animation or sequence count"));
    }
    if sequence_count == 0 {
        return Ok(Vec::new());
    }
    if sequence_offset <= 0 {
        return Err(invalid_data("Invalid MDL sequence offset"));
    }
    ensure_range(data, sequence_offset as usize, sequence_count as usize * SEQ_DESC_SIZE, "MDL sequences")?;
    let relative_string = |base: usize, field: usize| -> io::Result<String> {
        let relative = read_i32(data, base + field)?;
        if relative == 0 {
            return Ok(String::new());
        }
        let offset = checked_relative_offset(base, relative, data.len(), "MDL sequence name")?;
        read_c_string(data, offset)
    };
    let mut sequences = Vec::new();
    for index in 0..sequence_count as usize {
        let offset = sequence_offset as usize + index * SEQ_DESC_SIZE;
        // Retain errors on individual sequences so an unused animation cannot block inspection.
        let first_frame = (|| -> io::Result<Vec<BonePose>> {
            let position = checked_relative_offset(offset, read_i32(data, offset + 60)?, data.len(), "MDL sequence animation")?;
            let anim = read_i16(data, position)?;
            if anim < 0 || anim as i32 >= anim_count || anim_offset <= 0 {
                return Err(invalid_data("Invalid MDL sequence animation index"));
            }
            decode_first_frame(data, anim_offset as usize + anim as usize * ANIM_DESC_SIZE, bones)
        })().map_err(|error| error.to_string());
        sequences.push(MdlSequence {
            label: relative_string(offset, 4)?,
            activity: relative_string(offset, 8)?,
            first_frame,
        });
    }
    Ok(sequences)
}

fn decode_first_frame(data: &[u8], desc: usize, bones: &[MdlBone]) -> io::Result<Vec<BonePose>> {
    ensure_range(data, desc, ANIM_DESC_SIZE, "MDL animation")?;
    if read_i32(data, desc + 52)? != 0 {
        return Err(invalid_data("Animation is stored in an unsupported external .ani block"));
    }
    let anim_index = if read_i32(data, desc + 84)? != 0 {
        let section = checked_relative_offset(desc, read_i32(data, desc + 80)?, data.len(), "MDL animation section")?;
        if read_i32(data, section)? != 0 {
            return Err(invalid_data("Animation section is stored in an unsupported external .ani block"));
        }
        read_i32(data, section + 4)?
    } else {
        read_i32(data, desc + 56)?
    };
    let mut pose: Vec<BonePose> = bones
        .iter()
        .map(|bone| (bone.default_position, bone.default_rotation))
        .collect();
    if anim_index == 0 {
        return Err(invalid_data("MDL animation has no local bone data"));
    }
    let mut offset = checked_relative_offset(desc, anim_index, data.len(), "MDL animation data")?;
    for _ in 0..=bones.len() {
        ensure_range(data, offset, 4, "MDL animation bone")?;
        let (bone_index, flags, next) = (data[offset] as usize, data[offset + 1], read_i16(data, offset + 2)?);
        if bone_index == 255 {
            return Ok(pose);
        }
        let bone = bones.get(bone_index).ok_or_else(|| invalid_data("Animation bone index out of range"))?;
        if flags & ANIM_DELTA != 0 {
            return Err(invalid_data("Standalone delta animation poses are unsupported"));
        }
        let delta = flags & ANIM_DELTA != 0;
        let values = offset + 4;
        let rotation = if flags & ANIM_RAWROT != 0 {
            quaternion48(read_bytes(data, values, 6)?)
        } else if flags & ANIM_RAWROT2 != 0 {
            quaternion64(read_bytes(data, values, 8)?)
        } else if flags & ANIM_ANIMROT != 0 {
            let mut angles = [0.0; 3];
            for (axis, angle) in angles.iter_mut().enumerate() {
                let relative = read_i16(data, values + axis * 2)?;
                let value = if relative > 0 { anim_value(data, values + relative as usize)? } else { 0 };
                *angle = value as f32 * bone.rotation_scale[axis] + if delta { 0.0 } else { bone.default_euler[axis] };
            }
            euler_quaternion(angles)
        } else if delta {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            bone.default_rotation
        };
        let position = if flags & ANIM_RAWPOS != 0 {
            let raw = values + if flags & ANIM_RAWROT != 0 { 6 } else { 0 } + if flags & ANIM_RAWROT2 != 0 { 8 } else { 0 };
            let mut position = [0.0; 3];
            for (axis, value) in position.iter_mut().enumerate() {
                let bytes = read_bytes(data, raw + axis * 2, 2)?;
                *value = half_to_f32(u16::from_le_bytes([bytes[0], bytes[1]]));
            }
            position
        } else if flags & ANIM_ANIMPOS != 0 {
            let animated = values + if flags & ANIM_ANIMROT != 0 { 6 } else { 0 };
            let mut position = [0.0; 3];
            for (axis, coordinate) in position.iter_mut().enumerate() {
                let relative = read_i16(data, animated + axis * 2)?;
                let value = if relative > 0 { anim_value(data, animated + relative as usize)? } else { 0 };
                *coordinate = value as f32 * bone.position_scale[axis] + if delta { 0.0 } else { bone.default_position[axis] };
            }
            position
        } else if delta {
            [0.0; 3]
        } else {
            bone.default_position
        };
        pose[bone_index] = (position, rotation);
        if next == 0 {
            return Ok(pose);
        }
        if next < 4 {
            return Err(invalid_data("Invalid animation next-bone offset"));
        }
        offset += next as usize;
    }
    Err(invalid_data("Animation bone chain exceeds bone count"))
}

/// Run-length encoded mstudioanimvalue_t stream, sampled at frame 0.
fn anim_value(data: &[u8], offset: usize) -> io::Result<i16> {
    let header = read_bytes(data, offset, 2)?;
    if header[0] == 0 || header[0] > header[1] {
        return Err(invalid_data("Invalid animation value run"));
    }
    read_i16(data, offset + 2)
}

fn quaternion48(bytes: &[u8]) -> [f32; 4] {
    let word = |index: usize| u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]) as i32;
    let x = (word(0) - 32768) as f32 / 32768.0;
    let y = (word(1) - 32768) as f32 / 32768.0;
    let z = ((word(2) & 0x7fff) - 16384) as f32 / 16384.0;
    let w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    [x, y, z, if word(2) & 0x8000 != 0 { -w } else { w }]
}

fn quaternion64(bytes: &[u8]) -> [f32; 4] {
    let bits = u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]);
    let component = |shift: u32| (((bits >> shift) & 0x1f_ffff) as f64 - 1048576.0) / 1048576.5;
    let (x, y, z) = (component(0), component(21), component(42));
    let w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    [x as f32, y as f32, z as f32, if bits >> 63 != 0 { -w as f32 } else { w as f32 }]
}

/// Source's AngleQuaternion(RadianEuler): x is roll, y is pitch, z is yaw.
fn euler_quaternion(angles: [f32; 3]) -> [f32; 4] {
    let (sr, cr) = (angles[0] * 0.5).sin_cos();
    let (sp, cp) = (angles[1] * 0.5).sin_cos();
    let (sy, cy) = (angles[2] * 0.5).sin_cos();
    [
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let mantissa = (bits & 0x3ff) as f32;
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => if mantissa == 0.0 { f32::INFINITY } else { f32::NAN },
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/*
    ============================================================
    HELPERS
    ============================================================
*/

fn read_vec3(data: &[u8], offset: usize) -> io::Result<[f32; 3]> {
    Ok([read_f32(data, offset)?, read_f32(data, offset + 4)?, read_f32(data, offset + 8)?])
}

fn read_i16(data: &[u8], offset: usize) -> io::Result<i16> {
    let bytes = read_bytes(data, offset, 2)?;

    Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_i32(data: &[u8], offset: usize) -> io::Result<i32> {
    let bytes = read_bytes(data, offset, 4)?;

    Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_f32(data: &[u8], offset: usize) -> io::Result<f32> {
    let bytes = read_bytes(data, offset, 4)?;
    Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_fixed_string(data: &[u8], offset: usize, length: usize) -> io::Result<String> {
    let bytes = read_bytes(data, offset, length)?;

    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());

    Ok(String::from_utf8_lossy(&bytes[..end]).to_string())
}

fn read_c_string(data: &[u8], offset: usize) -> io::Result<String> {
    if offset >= data.len() {
        return Err(invalid_data("String offset exceeds MDL"));
    }

    let remaining = &data[offset..];

    let end = remaining
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(remaining.len());

    Ok(String::from_utf8_lossy(&remaining[..end]).to_string())
}

fn checked_relative_offset(
    base: usize,
    relative: i32,
    file_size: usize,
    label: &str,
) -> io::Result<usize> {
    let absolute = base as i64 + relative as i64;

    if absolute < 0 || absolute as usize >= file_size {
        return Err(invalid_data(&format!(
            "{} points outside file: base={} relative={}",
            label, base, relative
        )));
    }

    Ok(absolute as usize)
}

fn ensure_range(data: &[u8], offset: usize, count: usize, label: &str) -> io::Result<()> {
    if offset.checked_add(count).map(|end| end <= data.len()) != Some(true) {
        return Err(invalid_data(&format!("{} exceeds file", label)));
    }

    Ok(())
}

fn read_bytes(data: &[u8], offset: usize, count: usize) -> io::Result<&[u8]> {
    ensure_range(data, offset, count, "MDL read")?;

    Ok(&data[offset..offset + count])
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

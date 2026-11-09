use std::{fs, io, path::Path};

const BODY_PART_SIZE: usize = 8;
const MODEL_SIZE: usize = 8;
const LOD_SIZE: usize = 12;
const MESH_SIZE: usize = 9;
const STRIP_GROUP_SIZE: usize = 25;
const VERTEX_SIZE: usize = 9;
const STRIP_SIZE: usize = 27;

const STRIP_IS_TRILIST: u8 = 0x01;
const STRIP_IS_TRISTRIP: u8 = 0x02;

#[derive(Debug, Clone)]
pub struct VtxHeader {
    pub version: i32,
    pub checksum: i32,

    pub lod_count: i32,

    pub body_part_count: i32,

    pub body_part_offset: i32,

    pub file_size: usize,
}

#[derive(Debug, Clone)]
pub struct VtxVertex {
    /*
        Index relative to the owning MDL mesh's
        vertex range.
    */
    pub original_mesh_vertex_id: u16,
}

#[derive(Debug, Clone)]
pub struct VtxStripGroup {
    pub vertices: Vec<VtxVertex>,

    pub indices: Vec<u16>,

    pub triangles: Vec<[u16; 3]>,
}

#[derive(Debug, Clone)]
pub struct VtxMesh {
    pub strip_groups: Vec<VtxStripGroup>,
}

#[derive(Debug, Clone)]
pub struct VtxLod {
    pub index: usize,

    pub switch_point: f32,

    pub meshes: Vec<VtxMesh>,
}

#[derive(Debug, Clone)]
pub struct VtxModel {
    pub lods: Vec<VtxLod>,
}

#[derive(Debug, Clone)]
pub struct VtxBodyPart {
    pub models: Vec<VtxModel>,
}

#[derive(Debug)]
pub struct VtxFile {
    pub header: VtxHeader,

    pub body_parts: Vec<VtxBodyPart>,
}

pub fn read_vtx<P>(path: P) -> io::Result<VtxFile>
where
    P: AsRef<Path>,
{
    let data = fs::read(path)?;

    if data.len() < 36 {
        return Err(invalid_data("VTX is smaller than header"));
    }

    let header = VtxHeader {
        version: read_i32(&data, 0)?,

        checksum: read_i32(&data, 16)?,

        lod_count: read_i32(&data, 20)?,

        body_part_count: read_i32(&data, 28)?,

        body_part_offset: read_i32(&data, 32)?,

        file_size: data.len(),
    };

    let mut body_parts = Vec::new();

    /*
        ========================================================
        BODY PART
        ========================================================
    */

    for body_index in 0..header.body_part_count.max(0) as usize {
        let body_offset = header.body_part_offset as usize + body_index * BODY_PART_SIZE;

        let model_count = read_i32(&data, body_offset)?;

        let model_relative = read_i32(&data, body_offset + 4)?;

        let model_array =
            relative_offset(body_offset, model_relative, data.len(), "VTX model array")?;

        let mut models = Vec::new();

        /*
            ====================================================
            MODEL
            ====================================================
        */

        for model_index in 0..model_count.max(0) as usize {
            let model_offset = model_array + model_index * MODEL_SIZE;

            let lod_count = read_i32(&data, model_offset)?;

            let lod_relative = read_i32(&data, model_offset + 4)?;

            let lod_array =
                relative_offset(model_offset, lod_relative, data.len(), "VTX LOD array")?;

            let mut lods = Vec::new();

            /*
                ================================================
                LOD
                ================================================
            */

            for lod_index in 0..lod_count.max(0) as usize {
                let lod_offset = lod_array + lod_index * LOD_SIZE;

                let mesh_count = read_i32(&data, lod_offset)?;

                let mesh_relative = read_i32(&data, lod_offset + 4)?;

                let switch_point = read_f32(&data, lod_offset + 8)?;

                let mesh_array =
                    relative_offset(lod_offset, mesh_relative, data.len(), "VTX mesh array")?;

                let mut meshes = Vec::new();

                /*
                    ============================================
                    MESH
                    ============================================
                */

                for mesh_index in 0..mesh_count.max(0) as usize {
                    let mesh_offset = mesh_array + mesh_index * MESH_SIZE;

                    let group_count = read_i32(&data, mesh_offset)?;

                    let group_relative = read_i32(&data, mesh_offset + 4)?;

                    let group_array = relative_offset(
                        mesh_offset,
                        group_relative,
                        data.len(),
                        "VTX stripgroup array",
                    )?;

                    let mut strip_groups = Vec::new();

                    /*
                        ========================================
                        STRIP GROUP
                        ========================================
                    */

                    for group_index in 0..group_count.max(0) as usize {
                        let group_offset = group_array + group_index * STRIP_GROUP_SIZE;

                        let vertex_count = read_i32(&data, group_offset)?;

                        let vertex_relative = read_i32(&data, group_offset + 4)?;

                        let index_count = read_i32(&data, group_offset + 8)?;

                        let index_relative = read_i32(&data, group_offset + 12)?;

                        let strip_count = read_i32(&data, group_offset + 16)?;

                        let strip_relative = read_i32(&data, group_offset + 20)?;

                        let vertex_array = relative_offset(
                            group_offset,
                            vertex_relative,
                            data.len(),
                            "VTX stripgroup vertices",
                        )?;

                        let index_array = relative_offset(
                            group_offset,
                            index_relative,
                            data.len(),
                            "VTX stripgroup indices",
                        )?;

                        let strip_array = relative_offset(
                            group_offset,
                            strip_relative,
                            data.len(),
                            "VTX strips",
                        )?;

                        /*
                            Vertex_t is 9 bytes because VTX
                            structures are packed(1).
                        */

                        let mut vertices = Vec::new();

                        for vertex_index in 0..vertex_count.max(0) as usize {
                            let offset = vertex_array + vertex_index * VERTEX_SIZE;

                            /*
                                byte[3] bone weight IDs
                                byte numBones
                                ushort origMeshVertID
                                char[3] bone IDs
                            */

                            vertices.push(VtxVertex {
                                original_mesh_vertex_id: read_u16(&data, offset + 4)?,
                            });
                        }

                        let mut indices = Vec::new();

                        for index in 0..index_count.max(0) as usize {
                            indices.push(read_u16(&data, index_array + index * 2)?);
                        }

                        /*
                            ====================================
                            STRIPS -> TRIANGLES
                            ====================================
                        */

                        let mut triangles = Vec::<[u16; 3]>::new();

                        for strip_index in 0..strip_count.max(0) as usize {
                            let offset = strip_array + strip_index * STRIP_SIZE;

                            let strip_index_count = read_i32(&data, offset)?;

                            let strip_index_offset = read_i32(&data, offset + 4)?;

                            let flags = read_u8(&data, offset + 18)?;

                            if strip_index_count < 3 || strip_index_offset < 0 {
                                continue;
                            }

                            let start = strip_index_offset as usize;

                            let count = strip_index_count as usize;

                            if start + count > indices.len() {
                                return Err(invalid_data(
                                    "VTX strip index range exceeds stripgroup",
                                ));
                            }

                            let strip_indices = &indices[start..start + count];

                            if flags & STRIP_IS_TRILIST != 0 {
                                for tri in strip_indices.chunks_exact(3) {
                                    push_triangle(&mut triangles, tri[0], tri[1], tri[2]);
                                }
                            } else if flags & STRIP_IS_TRISTRIP != 0 {
                                for i in 0..strip_indices.len() - 2 {
                                    let a = strip_indices[i];

                                    let b = strip_indices[i + 1];

                                    let c = strip_indices[i + 2];

                                    /*
                                        Triangle strips alternate winding.
                                    */

                                    if i % 2 == 0 {
                                        push_triangle(&mut triangles, a, b, c);
                                    } else {
                                        push_triangle(&mut triangles, b, a, c);
                                    }
                                }
                            }
                        }

                        strip_groups.push(VtxStripGroup {
                            vertices,

                            indices,

                            triangles,
                        });
                    }

                    meshes.push(VtxMesh { strip_groups });
                }

                lods.push(VtxLod {
                    index: lod_index,

                    switch_point,

                    meshes,
                });
            }

            models.push(VtxModel { lods });
        }

        body_parts.push(VtxBodyPart { models });
    }

    Ok(VtxFile { header, body_parts })
}

fn push_triangle(output: &mut Vec<[u16; 3]>, a: u16, b: u16, c: u16) {
    /*
        Degenerate strip triangles are normal.
    */

    if a == b || b == c || a == c {
        return;
    }

    output.push([a, b, c]);
}

fn relative_offset(base: usize, relative: i32, file_size: usize, label: &str) -> io::Result<usize> {
    let value = base as i64 + relative as i64;

    if value < 0 || value as usize >= file_size {
        return Err(invalid_data(&format!(
            "{} outside file: base={} relative={}",
            label, base, relative
        )));
    }

    Ok(value as usize)
}

fn read_u8(data: &[u8], offset: usize) -> io::Result<u8> {
    if offset >= data.len() {
        return Err(invalid_data("Unexpected end of VTX"));
    }

    Ok(data[offset])
}

fn read_u16(data: &[u8], offset: usize) -> io::Result<u16> {
    if offset + 2 > data.len() {
        return Err(invalid_data("Unexpected end of VTX"));
    }

    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_i32(data: &[u8], offset: usize) -> io::Result<i32> {
    if offset + 4 > data.len() {
        return Err(invalid_data("Unexpected end of VTX"));
    }

    Ok(i32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn read_f32(data: &[u8], offset: usize) -> io::Result<f32> {
    if offset + 4 > data.len() {
        return Err(invalid_data("Unexpected end of VTX"));
    }

    Ok(f32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

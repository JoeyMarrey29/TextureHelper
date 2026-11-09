use std::{fs, io, path::Path};

const MAX_LODS: usize = 8;
const VERTEX_SIZE: usize = 48;
const FIXUP_SIZE: usize = 12;

#[derive(Debug, Clone)]
pub struct VvdHeader {
    pub id: i32,
    pub version: i32,
    pub checksum: i32,

    pub lod_count: i32,

    pub lod_vertex_counts: [i32; MAX_LODS],

    pub fixup_count: i32,

    pub fixup_table_offset: i32,

    pub vertex_data_offset: i32,

    pub tangent_data_offset: i32,

    pub file_size: usize,
}

#[derive(Debug, Clone)]
pub struct VvdFixup {
    pub lod: i32,

    pub source_vertex_id: i32,

    pub vertex_count: i32,
}

#[derive(Debug, Clone)]
pub struct VvdVertex {
    pub bone_weights: [f32; 4],

    pub bone_indices: [u8; 4],

    pub bone_count: u8,

    pub position: [f32; 3],

    pub normal: [f32; 3],

    pub uv: [f32; 2],
}

#[derive(Debug)]
pub struct VvdFile {
    pub header: VvdHeader,

    pub fixups: Vec<VvdFixup>,

    /*
        Raw on-disk vertex order.
    */
    pub raw_vertices: Vec<VvdVertex>,
}

impl VvdFile {
    /*
        Rebuild the VVD pool into the order that
        mstudiomodel_t.vertexindex expects.

        LOD 0 means full detail.
    */
    pub fn mesh_vertices(&self, root_lod: i32) -> io::Result<Vec<VvdVertex>> {
        if self.fixups.is_empty() {
            let count = self.header.lod_vertex_counts[0].max(0) as usize;

            return Ok(self.raw_vertices[..count.min(self.raw_vertices.len())].to_vec());
        }

        let mut output = Vec::new();

        for fixup in &self.fixups {
            /*
                Source's Studio_LoadVertexes copies
                fixups whose LOD >= requested root LOD.
            */
            if fixup.lod < root_lod {
                continue;
            }

            if fixup.source_vertex_id < 0 || fixup.vertex_count < 0 {
                return Err(invalid_data("Negative VVD fixup range"));
            }

            let start = fixup.source_vertex_id as usize;

            let end = start + fixup.vertex_count as usize;

            if end > self.raw_vertices.len() {
                return Err(invalid_data("VVD fixup exceeds vertex pool"));
            }

            output.extend_from_slice(&self.raw_vertices[start..end]);
        }

        let expected = self.header.lod_vertex_counts[root_lod as usize].max(0) as usize;

        if output.len() != expected {
            return Err(invalid_data(&format!(
                "VVD fixups reconstructed {} vertices, expected {}",
                output.len(),
                expected
            )));
        }

        Ok(output)
    }
}

pub fn read_vvd<P>(path: P) -> io::Result<VvdFile>
where
    P: AsRef<Path>,
{
    let data = fs::read(path)?;

    if data.len() < 64 {
        return Err(invalid_data("VVD is smaller than header"));
    }

    let mut lod_vertex_counts = [0i32; MAX_LODS];

    for index in 0..MAX_LODS {
        lod_vertex_counts[index] = read_i32(&data, 16 + index * 4)?;
    }

    let header = VvdHeader {
        id: read_i32(&data, 0)?,

        version: read_i32(&data, 4)?,

        checksum: read_i32(&data, 8)?,

        lod_count: read_i32(&data, 12)?,

        lod_vertex_counts,

        fixup_count: read_i32(&data, 48)?,

        fixup_table_offset: read_i32(&data, 52)?,

        vertex_data_offset: read_i32(&data, 56)?,

        tangent_data_offset: read_i32(&data, 60)?,

        file_size: data.len(),
    };

    /*
        ========================================================
        FIXUPS
        ========================================================
    */

    let mut fixups = Vec::new();

    for index in 0..header.fixup_count.max(0) as usize {
        let offset = header.fixup_table_offset as usize + index * FIXUP_SIZE;

        fixups.push(VvdFixup {
            lod: read_i32(&data, offset)?,

            source_vertex_id: read_i32(&data, offset + 4)?,

            vertex_count: read_i32(&data, offset + 8)?,
        });
    }

    /*
        ========================================================
        RAW VERTEX POOL
        ========================================================

        mstudiovertex_t:

        0..11   bone weights
        12..14  bone IDs
        15      num bones

        16..27  position xyz
        28..39  normal xyz
        40..47  UV
    */

    let vertex_count = header.lod_vertex_counts[0].max(0) as usize;

    let vertex_start = header.vertex_data_offset as usize;

    let required = vertex_start + vertex_count * VERTEX_SIZE;

    if required > data.len() {
        return Err(invalid_data("VVD vertex pool exceeds file"));
    }

    let mut raw_vertices = Vec::with_capacity(vertex_count);

    for index in 0..vertex_count {
        let offset = vertex_start + index * VERTEX_SIZE;

        raw_vertices.push(VvdVertex {
            bone_weights: [
                read_f32(&data, offset)?,
                read_f32(&data, offset + 4)?,
                read_f32(&data, offset + 8)?,
                0.0,
            ],

            bone_indices: [data[offset + 12], data[offset + 13], data[offset + 14], 0],

            bone_count: data[offset + 15].min(4),

            position: [
                read_f32(&data, offset + 16)?,
                read_f32(&data, offset + 20)?,
                read_f32(&data, offset + 24)?,
            ],

            normal: [
                read_f32(&data, offset + 28)?,
                read_f32(&data, offset + 32)?,
                read_f32(&data, offset + 36)?,
            ],

            uv: [read_f32(&data, offset + 40)?, read_f32(&data, offset + 44)?],
        });
    }

    Ok(VvdFile {
        header,

        fixups,

        raw_vertices,
    })
}

fn read_i32(data: &[u8], offset: usize) -> io::Result<i32> {
    if offset + 4 > data.len() {
        return Err(invalid_data("Unexpected end of VVD"));
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
        return Err(invalid_data("Unexpected end of VVD"));
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

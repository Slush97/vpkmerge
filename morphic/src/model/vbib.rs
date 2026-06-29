//! Vertex/index buffer descriptors and attribute extraction, ported from VRF
//! `Blocks/VBIB.cs` (the `m_inputLayoutFields` / `BufferDataFromDATA` path used
//! by the modern `MVTX`/`MIDX` model format). A descriptor comes from the
//! `CTRL` block's embedded-mesh buffer registry; the raw bytes come from the
//! `MVTX`/`MIDX` block it points at (`m_nBlockIndex`), meshopt-decoded.
//!
//! `GetNormalTangentArray` / `GetBlendIndicesArray` / `GetBlendWeightsArray`
//! and the compressed-normal math are reproduced so the deinterleaved
//! attributes match the golden GLB.

// Faithful port of VRF's VBIB attribute decoders: the byte-level casts and the
// compressed-normal bit-twiddling mirror the reference and are intentional.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use crate::error::DecodeError;
use crate::kv3::Value;
use crate::meshopt::{decode_index_buffer, decode_vertex_buffer};

use super::dxgi::DxgiFormat;

/// One vertex attribute within a vertex buffer's stride.
#[derive(Debug, Clone)]
pub struct InputLayoutField {
    /// Upper-cased semantic (`POSITION`, `NORMAL`, `TEXCOORD`, ...).
    pub semantic_name: String,
    pub semantic_index: i32,
    pub format: DxgiFormat,
    /// Byte offset of this attribute inside one vertex.
    pub offset: usize,
}

/// A buffer descriptor from the `CTRL` embedded-mesh registry. For vertex
/// buffers `element_size` is the stride and `fields` is non-empty; for index
/// buffers `element_size` is the index width (2 or 4) and `fields` is empty.
#[derive(Debug, Clone)]
pub struct BufferDesc {
    /// Global block index of the `MVTX`/`MIDX` payload.
    pub block_index: usize,
    pub element_count: usize,
    pub element_size: usize,
    pub meshopt: bool,
    pub zstd: bool,
    pub fields: Vec<InputLayoutField>,
}

impl BufferDesc {
    /// Parses one entry of `m_vertexBuffers` / `m_indexBuffers`.
    pub fn from_kv(buf: &Value) -> Result<BufferDesc, DecodeError> {
        let block_index = usize::try_from(
            buf.get("m_nBlockIndex")
                .and_then(Value::as_int)
                .ok_or(DecodeError::Model("buffer missing m_nBlockIndex"))?,
        )
        .map_err(|_| DecodeError::Model("negative block index"))?;
        let element_count = usize::try_from(
            buf.get("m_nElementCount")
                .and_then(Value::as_int)
                .ok_or(DecodeError::Model("buffer missing m_nElementCount"))?,
        )
        .map_err(|_| DecodeError::Model("negative element count"))?;
        let element_size = usize::try_from(
            buf.get("m_nElementSizeInBytes")
                .and_then(Value::as_int)
                .ok_or(DecodeError::Model("buffer missing m_nElementSizeInBytes"))?,
        )
        .map_err(|_| DecodeError::Model("negative element size"))?;
        let meshopt = buf
            .get("m_bMeshoptCompressed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let zstd = buf
            .get("m_bCompressedZSTD")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut fields = Vec::new();
        if let Some(arr) = buf.get("m_inputLayoutFields").and_then(Value::as_array) {
            for f in arr {
                fields.push(parse_field(f)?);
            }
        }

        Ok(BufferDesc {
            block_index,
            element_count,
            element_size,
            meshopt,
            zstd,
            fields,
        })
    }

    /// Resolves and decompresses this buffer's raw block into an interleaved
    /// `element_count * element_size` byte stream.
    pub fn decode(&self, block_bytes: &[u8], is_vertex: bool) -> Result<OnDiskBuffer, DecodeError> {
        if self.zstd {
            return Err(DecodeError::Model("ZSTD mesh buffers not supported"));
        }
        let total = self
            .element_count
            .checked_mul(self.element_size)
            .ok_or(DecodeError::Model("buffer size overflow"))?;

        let data = if self.meshopt {
            if is_vertex {
                decode_vertex_buffer(self.element_count, self.element_size, block_bytes)?
            } else {
                decode_index_buffer(self.element_count, self.element_size, block_bytes)?
            }
        } else if block_bytes.len() >= total {
            block_bytes[..total].to_vec()
        } else {
            return Err(DecodeError::Model("uncompressed buffer too short"));
        };

        if data.len() != total {
            return Err(DecodeError::Model("decoded buffer size mismatch"));
        }
        Ok(OnDiskBuffer {
            data,
            element_count: self.element_count,
            element_size: self.element_size,
            fields: self.fields.clone(),
        })
    }
}

fn parse_field(f: &Value) -> Result<InputLayoutField, DecodeError> {
    let semantic_name = match f.get("m_pSemanticName") {
        Some(Value::String(s)) => s.to_uppercase(),
        Some(Value::Binary(b)) => {
            let trimmed: &[u8] = match b.iter().position(|&c| c == 0) {
                Some(n) => &b[..n],
                None => b,
            };
            String::from_utf8_lossy(trimmed).to_uppercase()
        }
        _ => return Err(DecodeError::Model("layout field missing semantic name")),
    };
    let semantic_index = f
        .get("m_nSemanticIndex")
        .and_then(Value::as_int)
        .unwrap_or(0) as i32;
    let format_id = f
        .get("m_Format")
        .and_then(Value::as_uint)
        .ok_or(DecodeError::Model("layout field missing m_Format"))? as u32;
    let format =
        DxgiFormat::from_u32(format_id).ok_or(DecodeError::Model("unsupported vertex format"))?;
    let offset = usize::try_from(
        f.get("m_nOffset")
            .and_then(Value::as_int)
            .ok_or(DecodeError::Model("layout field missing m_nOffset"))?,
    )
    .map_err(|_| DecodeError::Model("negative attribute offset"))?;

    Ok(InputLayoutField {
        semantic_name,
        semantic_index,
        format,
        offset,
    })
}

fn read_u32(block: &[u8], pos: usize) -> Result<u32, DecodeError> {
    let s = block
        .get(pos..pos + 4)
        .ok_or(DecodeError::Model("VBIB block truncated"))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Reads a fixed 32-byte, null-terminated semantic name and upper-cases it to
/// match [`parse_field`] (the deinterleaver keys on `POSITION`/`NORMAL`/...).
fn read_semantic_name(name: &[u8]) -> String {
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).to_uppercase()
}

/// Parses a classic VBIB-format mesh-buffer block (FOURCC `MBUF`, or a `VBIB`
/// block) into decoded vertex and index buffers, in declaration order.
///
/// This is the geometry layout `CTRL.embedded_meshes[].vbib_block` points at.
/// Modern Deadlock models split geometry into one `MVTX`/`MIDX` block per buffer
/// described by KV3 ([`BufferDesc::from_kv`]); older and decompile-recompiled
/// mods ("optimized" soul containers) emit this single self-describing block
/// instead, which the per-descriptor path can't read.
///
/// Layout (a port of VRF `Blocks/VBIB.cs`): a 16-byte header of
/// `(vertexBuffer, indexBuffer)` `(offset, count)` pairs, each offset relative
/// to its own field; then per buffer a 24-byte `OnDiskBufferData`
/// `(elementCount, elementSize, attrRef{off,count}, dataRef{off,size})` with the
/// refs relative to their own field; attributes are 56-byte
/// `RenderInputLayoutField` records (32-byte semantic name, then `semanticIndex`,
/// DXGI `format`, `offset`, `slot`, `slotType`, `instanceStepRate` -- only the
/// first four are needed). A payload shorter than `elementCount * elementSize`
/// is meshopt-compressed, mirroring the `MVTX`/`MIDX` path.
pub fn parse_vbib_block(
    block: &[u8],
) -> Result<(Vec<OnDiskBuffer>, Vec<OnDiskBuffer>), DecodeError> {
    let vb_count = read_u32(block, 4)? as usize;
    let ib_count = read_u32(block, 12)? as usize;
    // Header offsets are relative to their own u32 field (positions 0 and 8).
    let vb_start = (read_u32(block, 0)? as usize)
        .checked_add(0)
        .ok_or(DecodeError::Model("VBIB offset overflow"))?;
    let ib_start = (read_u32(block, 8)? as usize)
        .checked_add(8)
        .ok_or(DecodeError::Model("VBIB offset overflow"))?;
    let vertices = parse_vbib_buffers(block, vb_start, vb_count, true)?;
    let indices = parse_vbib_buffers(block, ib_start, ib_count, false)?;
    Ok((vertices, indices))
}

fn parse_vbib_buffers(
    block: &[u8],
    start: usize,
    count: usize,
    is_vertex: bool,
) -> Result<Vec<OnDiskBuffer>, DecodeError> {
    const REC: usize = 24; // OnDiskBufferData header size
    const FIELD: usize = 56; // RenderInputLayoutField size
    let ovf = || DecodeError::Model("VBIB offset overflow");
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let pos = start
            .checked_add(i.checked_mul(REC).ok_or_else(ovf)?)
            .ok_or_else(ovf)?;
        let element_count = read_u32(block, pos)? as usize;
        let element_size = read_u32(block, pos + 4)? as usize;
        let attr_off = read_u32(block, pos + 8)? as usize;
        let attr_count = read_u32(block, pos + 12)? as usize;
        let data_off = read_u32(block, pos + 16)? as usize;
        let total_size = read_u32(block, pos + 20)? as usize;

        // attrRef / dataRef are relative to their own fields (pos+8, pos+16).
        let attr_start = (pos + 8).checked_add(attr_off).ok_or_else(ovf)?;
        let mut fields = Vec::with_capacity(attr_count);
        for a in 0..attr_count {
            let fp = attr_start
                .checked_add(a.checked_mul(FIELD).ok_or_else(ovf)?)
                .ok_or_else(ovf)?;
            let name = block
                .get(fp..fp + 32)
                .ok_or(DecodeError::Model("VBIB block truncated"))?;
            let format_id = read_u32(block, fp + 36)?;
            let format = DxgiFormat::from_u32(format_id)
                .ok_or(DecodeError::Model("unsupported vertex format"))?;
            fields.push(InputLayoutField {
                semantic_name: read_semantic_name(name),
                semantic_index: read_u32(block, fp + 32)? as i32,
                format,
                offset: read_u32(block, fp + 40)? as usize,
            });
        }

        let data_start = (pos + 16).checked_add(data_off).ok_or_else(ovf)?;
        let data = block
            .get(data_start..data_start.checked_add(total_size).ok_or_else(ovf)?)
            .ok_or(DecodeError::Model("VBIB block truncated"))?;

        // VBIB records the on-disk size; a payload shorter than the unpacked
        // size is meshopt-compressed (same rule the MVTX/MIDX path uses).
        let unpacked = element_count
            .checked_mul(element_size)
            .ok_or(DecodeError::Model("buffer size overflow"))?;
        let desc = BufferDesc {
            block_index: 0,
            element_count,
            element_size,
            meshopt: total_size < unpacked,
            zstd: false,
            fields,
        };
        out.push(desc.decode(data, is_vertex)?);
    }
    Ok(out)
}

/// Decoded normal + tangent attribute arrays. Tangents are empty when the
/// normal format is uncompressed (a standalone `TANGENT` attribute carries them
/// instead).
pub type NormalTangent = (Vec<[f32; 3]>, Vec<[f32; 4]>);

/// A decoded, still-interleaved vertex or index buffer.
#[derive(Debug, Clone)]
pub struct OnDiskBuffer {
    pub data: Vec<u8>,
    pub element_count: usize,
    pub element_size: usize,
    pub fields: Vec<InputLayoutField>,
}

impl OnDiskBuffer {
    /// Reads `count` indices (each `element_size` bytes, 2 or 4) starting at
    /// `start`, adding `base_vertex`. Mirrors VRF `GltfModelExporter.ReadIndices`.
    pub fn read_indices(
        &self,
        start: usize,
        count: usize,
        base_vertex: u32,
    ) -> Result<Vec<u32>, DecodeError> {
        let mut out = Vec::with_capacity(count);
        match self.element_size {
            2 => {
                for i in start..start + count {
                    let off = i * 2;
                    let raw = u16::from_le_bytes(read_n::<2>(&self.data, off)?);
                    out.push(base_vertex.wrapping_add(u32::from(raw)));
                }
            }
            4 => {
                for i in start..start + count {
                    let off = i * 4;
                    let raw = u32::from_le_bytes(read_n::<4>(&self.data, off)?);
                    out.push(base_vertex.wrapping_add(raw));
                }
            }
            _ => return Err(DecodeError::Model("unsupported index width")),
        }
        Ok(out)
    }

    fn field(&self, predicate: impl Fn(&InputLayoutField) -> bool) -> Option<&InputLayoutField> {
        self.fields.iter().find(|f| predicate(f))
    }

    /// `POSITION` (semantic index 0) as `[x, y, z]` per vertex.
    pub fn positions(&self) -> Result<Vec<[f32; 3]>, DecodeError> {
        let attr = self
            .field(|f| f.semantic_name == "POSITION")
            .ok_or(DecodeError::Model("vertex buffer has no POSITION"))?;
        if attr.format != DxgiFormat::R32G32B32Float {
            return Err(DecodeError::Model("unexpected POSITION format"));
        }
        let mut out = Vec::with_capacity(self.element_count);
        for i in 0..self.element_count {
            let o = i * self.element_size + attr.offset;
            out.push([self.f32_at(o)?, self.f32_at(o + 4)?, self.f32_at(o + 8)?]);
        }
        Ok(out)
    }

    /// Overwrites the `POSITION` attribute in-place with `new_positions`, leaving
    /// every other interleaved attribute (normal, uv, blend indices/weights)
    /// byte-for-byte untouched. The basis of a vertex-displacement edit: reshape
    /// existing geometry without changing topology, then re-encode the buffer.
    ///
    /// Errors if the buffer has no `R32G32B32_FLOAT` `POSITION`, or if
    /// `new_positions.len()` differs from the vertex count (count changes are a
    /// topology edit, out of scope for displacement).
    pub fn write_positions(&mut self, new_positions: &[[f32; 3]]) -> Result<(), DecodeError> {
        let attr = self
            .field(|f| f.semantic_name == "POSITION")
            .ok_or(DecodeError::Model("vertex buffer has no POSITION"))?;
        if attr.format != DxgiFormat::R32G32B32Float {
            return Err(DecodeError::Model("unexpected POSITION format"));
        }
        if new_positions.len() != self.element_count {
            return Err(DecodeError::Model(
                "position count does not match vertex count (topology change not supported)",
            ));
        }
        let offset = attr.offset;
        let stride = self.element_size;
        for (i, p) in new_positions.iter().enumerate() {
            let o = i * stride + offset;
            for (c, &v) in p.iter().enumerate() {
                let bytes = v.to_le_bytes();
                self.data
                    .get_mut(o + c * 4..o + c * 4 + 4)
                    .ok_or(DecodeError::Model("position write past buffer"))?
                    .copy_from_slice(&bytes);
            }
        }
        Ok(())
    }

    /// Every `COLOR` attribute in this buffer's layout, cloned so the caller can
    /// hold them while mutating `self` via [`write_colors`]. A buffer usually has
    /// exactly one, but a paint/AO split could carry more (different semantic
    /// indices); the recolor path edits all of them.
    pub fn color_fields(&self) -> Vec<InputLayoutField> {
        self.fields
            .iter()
            .filter(|f| f.semantic_name == "COLOR")
            .cloned()
            .collect()
    }

    /// Overwrites the `attr` lane (a `COLOR` attribute) in-place with `colors`
    /// (RGBA in 0..1), leaving every other interleaved attribute byte-for-byte
    /// untouched. The color analog of [`write_positions`]: the basis of a
    /// vertex-color recolor (rewrite the baked per-vertex tint, keep topology).
    ///
    /// Quantizes back into the attribute's on-disk format, covering the three
    /// formats [`vector4`] reads (`R8G8B8A8_UNORM`, `R16G16B16A16_FLOAT`,
    /// `R32G32B32A32_FLOAT`). Errors on an unsupported format or a count mismatch
    /// (a vertex-count change is a topology edit, out of scope).
    ///
    /// [`vector4`]: Self::vector4
    pub fn write_colors(
        &mut self,
        attr: &InputLayoutField,
        colors: &[[f32; 4]],
    ) -> Result<(), DecodeError> {
        if attr.semantic_name != "COLOR" {
            return Err(DecodeError::Model(
                "write_colors called on a non-COLOR attribute",
            ));
        }
        if colors.len() != self.element_count {
            return Err(DecodeError::Model(
                "color count does not match vertex count (topology change not supported)",
            ));
        }
        let offset = attr.offset;
        let stride = self.element_size;
        for (i, c) in colors.iter().enumerate() {
            let o = i * stride + offset;
            match attr.format {
                DxgiFormat::R8G8B8A8Unorm => {
                    let bytes = [unorm8(c[0]), unorm8(c[1]), unorm8(c[2]), unorm8(c[3])];
                    self.write_at(o, &bytes)?;
                }
                DxgiFormat::R16G16B16A16Float => {
                    for (k, &v) in c.iter().enumerate() {
                        let bits = half::f16::from_f32(v).to_bits().to_le_bytes();
                        self.write_at(o + k * 2, &bits)?;
                    }
                }
                DxgiFormat::R32G32B32A32Float => {
                    for (k, &v) in c.iter().enumerate() {
                        self.write_at(o + k * 4, &v.to_le_bytes())?;
                    }
                }
                _ => return Err(DecodeError::Model("unsupported COLOR format for write")),
            }
        }
        Ok(())
    }

    /// Overwrites the `attr` lane (a `BLENDINDICES` attribute) in-place with
    /// `joints` (four mesh-local palette slots per vertex, as
    /// [`blend_indices`](Self::blend_indices) returns with `remap = None`),
    /// leaving every other interleaved byte untouched. The skinning analog of
    /// [`write_colors`](Self::write_colors): re-point which bones drive a vertex
    /// without changing topology, position, normal, uv, or weights.
    ///
    /// Covers the 4-influence formats `blend_indices` reads (`R8G8B8A8_UINT`,
    /// `R16G16B16A16_SINT`, and the 2-wide `R16G16_SINT` where only the first two
    /// lanes are stored). Errors on an 8-influence format or a count mismatch (a
    /// vertex-count change is a topology edit, out of scope).
    pub fn write_blend_indices(
        &mut self,
        attr: &InputLayoutField,
        joints: &[[u16; 4]],
    ) -> Result<(), DecodeError> {
        if attr.semantic_name != "BLENDINDICES" {
            return Err(DecodeError::Model(
                "write_blend_indices called on a non-BLENDINDICES attribute",
            ));
        }
        if joints.len() != self.element_count {
            return Err(DecodeError::Model(
                "joint count does not match vertex count (topology change not supported)",
            ));
        }
        let offset = attr.offset;
        let stride = self.element_size;
        for (i, j) in joints.iter().enumerate() {
            let o = i * stride + offset;
            match attr.format {
                DxgiFormat::R8G8B8A8Uint => {
                    let mut bytes = [0u8; 4];
                    for k in 0..4 {
                        bytes[k] = u8::try_from(j[k])
                            .map_err(|_| DecodeError::Model("blend index exceeds u8 palette"))?;
                    }
                    self.write_at(o, &bytes)?;
                }
                DxgiFormat::R16G16B16A16Sint => {
                    for (k, &v) in j.iter().enumerate() {
                        self.write_at(o + k * 2, &v.to_le_bytes())?;
                    }
                }
                DxgiFormat::R16G16Sint => {
                    self.write_at(o, &j[0].to_le_bytes())?;
                    self.write_at(o + 2, &j[1].to_le_bytes())?;
                }
                _ => {
                    return Err(DecodeError::Model(
                        "unsupported BLENDINDICES format for write",
                    ))
                }
            }
        }
        Ok(())
    }

    /// Copies `bytes` into `self.data` at `o`, erroring if it would run past the
    /// buffer. The shared bounds-checked write under [`write_colors`].
    fn write_at(&mut self, o: usize, bytes: &[u8]) -> Result<(), DecodeError> {
        self.data
            .get_mut(o..o + bytes.len())
            .ok_or(DecodeError::Model("color write past buffer"))?
            .copy_from_slice(bytes);
        Ok(())
    }

    /// Generic 2-component attribute (UVs), with the half/unorm/snorm variants.
    pub fn vector2(&self, attr: &InputLayoutField) -> Result<Vec<[f32; 2]>, DecodeError> {
        let mut out = Vec::with_capacity(self.element_count);
        for i in 0..self.element_count {
            let o = i * self.element_size + attr.offset;
            let v = match attr.format {
                DxgiFormat::R32G32Float => [self.f32_at(o)?, self.f32_at(o + 4)?],
                DxgiFormat::R16G16Float => [self.half_at(o)?, self.half_at(o + 2)?],
                DxgiFormat::R16G16Unorm => [
                    f32::from(self.u16_at(o)?) / 65535.0,
                    f32::from(self.u16_at(o + 2)?) / 65535.0,
                ],
                DxgiFormat::R16G16Snorm => [
                    f32::from(self.i16_at(o)?) / 32767.0,
                    f32::from(self.i16_at(o + 2)?) / 32767.0,
                ],
                // A 1-component texcoord (the V channel is implicit); read the
                // single float and zero-fill, matching VRF's component-count read.
                DxgiFormat::R32Float => [self.f32_at(o)?, 0.0],
                _ => return Err(DecodeError::Model("unexpected vec2 format")),
            };
            out.push(v);
        }
        Ok(out)
    }

    /// Generic 4-component attribute (TANGENT / COLOR), with the half/unorm variants.
    pub fn vector4(&self, attr: &InputLayoutField) -> Result<Vec<[f32; 4]>, DecodeError> {
        let mut out = Vec::with_capacity(self.element_count);
        for i in 0..self.element_count {
            let o = i * self.element_size + attr.offset;
            let v = match attr.format {
                DxgiFormat::R32G32B32A32Float => [
                    self.f32_at(o)?,
                    self.f32_at(o + 4)?,
                    self.f32_at(o + 8)?,
                    self.f32_at(o + 12)?,
                ],
                DxgiFormat::R16G16B16A16Float => [
                    self.half_at(o)?,
                    self.half_at(o + 2)?,
                    self.half_at(o + 4)?,
                    self.half_at(o + 6)?,
                ],
                DxgiFormat::R8G8B8A8Unorm => [
                    f32::from(self.u8_at(o)?) / 255.0,
                    f32::from(self.u8_at(o + 1)?) / 255.0,
                    f32::from(self.u8_at(o + 2)?) / 255.0,
                    f32::from(self.u8_at(o + 3)?) / 255.0,
                ],
                _ => return Err(DecodeError::Model("unexpected vec4 format")),
            };
            out.push(v);
        }
        Ok(out)
    }

    /// `NORMAL` (and the tangent it may carry). Mirrors
    /// `VBIB.GetNormalTangentArray`: uncompressed `R32G32B32_FLOAT` yields
    /// normals with an empty tangent list; the two compressed encodings yield
    /// both. Tangents are returned separately so the caller can decide whether
    /// a standalone `TANGENT` attribute should win.
    pub fn normal_tangent(&self, attr: &InputLayoutField) -> Result<NormalTangent, DecodeError> {
        match attr.format {
            DxgiFormat::R32G32B32Float => {
                let mut normals = Vec::with_capacity(self.element_count);
                for i in 0..self.element_count {
                    let o = i * self.element_size + attr.offset;
                    normals.push([self.f32_at(o)?, self.f32_at(o + 4)?, self.f32_at(o + 8)?]);
                }
                Ok((normals, Vec::new()))
            }
            DxgiFormat::R32Uint => {
                let mut packed = Vec::with_capacity(self.element_count);
                for i in 0..self.element_count {
                    let o = i * self.element_size + attr.offset;
                    packed.push(u32::from_le_bytes(read_n::<4>(&self.data, o)?));
                }
                Ok(decompress_normal_tangents_v2(&packed))
            }
            DxgiFormat::R8G8B8A8Unorm => {
                let mut normals = Vec::with_capacity(self.element_count);
                let mut tangents = Vec::with_capacity(self.element_count);
                for i in 0..self.element_count {
                    let o = i * self.element_size + attr.offset;
                    normals.push(decompress_normal(
                        f32::from(self.u8_at(o)?),
                        f32::from(self.u8_at(o + 1)?),
                    ));
                    tangents.push(decompress_tangent(
                        f32::from(self.u8_at(o + 2)?),
                        f32::from(self.u8_at(o + 3)?),
                    ));
                }
                Ok((normals, tangents))
            }
            _ => Err(DecodeError::Model("unexpected NORMAL format")),
        }
    }

    /// `BLENDINDICES`, four (or eight) joints per vertex, optionally remapped
    /// through the mesh's bone remap table. Mirrors `VBIB.GetBlendIndicesArray`.
    pub fn blend_indices(
        &self,
        attr: &InputLayoutField,
        remap: Option<&[usize]>,
    ) -> Result<Vec<u16>, DecodeError> {
        let num_joints = match attr.format {
            DxgiFormat::R32G32B32A32Sint | DxgiFormat::R16G16B16A16Uint => 8,
            _ => 4,
        };
        let mut out = vec![0u16; self.element_count * num_joints];

        for i in 0..self.element_count {
            let o = i * self.element_size + attr.offset;
            let base = i * num_joints;
            match attr.format {
                DxgiFormat::R16G16Sint => {
                    let a = self.u16_at(o)?;
                    let b = self.u16_at(o + 2)?;
                    out[base] = a;
                    out[base + 1] = b;
                    out[base + 2] = b;
                    out[base + 3] = b;
                }
                DxgiFormat::R16G16B16A16Sint | DxgiFormat::R32G32B32A32Sint => {
                    for j in 0..num_joints {
                        out[base + j] = self.u16_at(o + j * 2)?;
                    }
                }
                DxgiFormat::R8G8B8A8Uint | DxgiFormat::R16G16B16A16Uint => {
                    for j in 0..num_joints {
                        out[base + j] = u16::from(self.u8_at(o + j)?);
                    }
                }
                _ => return Err(DecodeError::Model("unexpected BLENDINDICES format")),
            }
        }

        if let Some(table) = remap {
            for idx in &mut out {
                let mapped = *table
                    .get(usize::from(*idx))
                    .ok_or(DecodeError::Model("blend index out of remap range"))?;
                *idx = u16::try_from(mapped)
                    .map_err(|_| DecodeError::Model("remapped bone index too large"))?;
            }
        }
        Ok(out)
    }

    /// `BLENDWEIGHT(S)`, flat weights at the format's native influence width (4
    /// or 8 per vertex). Mirrors `VBIB.GetBlendWeightsArray`. The 8-influence
    /// stream is an `R16G16B16A16_UNORM`-tagged block of 8 `u8`s, paired with the
    /// matching 8-wide `R16G16B16A16_UINT` / `R32G32B32A32_SINT` indices that
    /// `blend_indices` already reads. Width is recoverable as `len / count`.
    pub fn blend_weights(&self, attr: &InputLayoutField) -> Result<Vec<f32>, DecodeError> {
        let mut out = Vec::with_capacity(self.element_count * 4);
        for i in 0..self.element_count {
            let o = i * self.element_size + attr.offset;
            match attr.format {
                DxgiFormat::R8G8B8A8Unorm => {
                    for k in 0..4 {
                        out.push(f32::from(self.u8_at(o + k)?) / 255.0);
                    }
                }
                DxgiFormat::R16G16Unorm => {
                    out.push(f32::from(self.u16_at(o)?) / 65535.0);
                    out.push(f32::from(self.u16_at(o + 2)?) / 65535.0);
                    out.push(0.0);
                    out.push(0.0);
                }
                DxgiFormat::R16G16B16A16Unorm => {
                    for k in 0..8 {
                        out.push(f32::from(self.u8_at(o + k)?) / 255.0);
                    }
                }
                _ => return Err(DecodeError::Model("unexpected BLENDWEIGHT format")),
            }
        }
        Ok(out)
    }

    fn f32_at(&self, o: usize) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(read_n::<4>(&self.data, o)?))
    }
    fn u16_at(&self, o: usize) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(read_n::<2>(&self.data, o)?))
    }
    fn i16_at(&self, o: usize) -> Result<i16, DecodeError> {
        Ok(i16::from_le_bytes(read_n::<2>(&self.data, o)?))
    }
    fn u8_at(&self, o: usize) -> Result<u8, DecodeError> {
        self.data
            .get(o)
            .copied()
            .ok_or(DecodeError::Model("attribute read past buffer"))
    }
    fn half_at(&self, o: usize) -> Result<f32, DecodeError> {
        Ok(half::f16::from_bits(self.u16_at(o)?).to_f32())
    }
}

/// Quantize a 0..1 channel to `u8` unorm (the inverse of `vector4`'s
/// `u8 / 255.0`), clamping out-of-range values. The cast is lossless after the
/// clamp+round; truncation/sign-loss are blanket-allowed for this module.
fn unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn read_n<const N: usize>(data: &[u8], o: usize) -> Result<[u8; N], DecodeError> {
    let end = o
        .checked_add(N)
        .ok_or(DecodeError::Model("attribute offset overflow"))?;
    if end > data.len() {
        return Err(DecodeError::Model("attribute read past buffer"));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&data[o..end]);
    Ok(out)
}

// --- compressed normal/tangent math (ports of VBIB.cs) ---

fn decompress_normal(mut x: f32, mut y: f32) -> [f32; 3] {
    x -= 128.0;
    y -= 128.0;

    let z_sign_bit = if x < 0.0 { 1.0 } else { 0.0 };
    let t_sign_bit = if y < 0.0 { 1.0 } else { 0.0 };
    let z_sign = -((2.0 * z_sign_bit) - 1.0);
    let t_sign = -((2.0 * t_sign_bit) - 1.0);

    x = (x * z_sign) - z_sign_bit;
    y = (y * t_sign) - t_sign_bit;
    x -= 64.0;
    y -= 64.0;

    let x_sign_bit = if x < 0.0 { 1.0 } else { 0.0 };
    let y_sign_bit = if y < 0.0 { 1.0 } else { 0.0 };
    let x_sign = -((2.0 * x_sign_bit) - 1.0);
    let y_sign = -((2.0 * y_sign_bit) - 1.0);

    x = ((x * x_sign) - x_sign_bit) / 63.0;
    y = ((y * y_sign) - y_sign_bit) / 63.0;
    let z = 1.0 - x - y;

    let oolen = 1.0 / (x * x + y * y + z * z).sqrt();
    [x * oolen * x_sign, y * oolen * y_sign, z * oolen * z_sign]
}

fn decompress_tangent(x: f32, y: f32) -> [f32; 4] {
    let n = decompress_normal(x, y);
    let t_sign = if y < 128.0 { -1.0 } else { 1.0 };
    [n[0], n[1], n[2], t_sign]
}

fn decompress_normal_tangents_v2(packed_frames: &[u32]) -> (Vec<[f32; 3]>, Vec<[f32; 4]>) {
    use std::f32::consts::TAU;

    let mut normals = Vec::with_capacity(packed_frames.len());
    let mut tangents = Vec::with_capacity(packed_frames.len());

    for &frame in packed_frames {
        let sign_bit = frame & 1;
        let t_bits = ((frame >> 1) & 0x7ff) as f32;
        let x_bits = ((frame >> 12) & 0x3ff) as f32;
        let y_bits = ((frame >> 22) & 0x3ff) as f32;

        let nx = (x_bits / 1023.0) * 2.0 - 1.0;
        let ny = (y_bits / 1023.0) * 2.0 - 1.0;
        let derived_z = 1.0 - nx.abs() - ny.abs();

        let neg_z = (-derived_z).clamp(0.0, 1.0);
        let x_pos = if nx >= 0.0 { 1.0 } else { 0.0 };
        let y_pos = if ny >= 0.0 { 1.0 } else { 0.0 };
        let ux = nx + neg_z * (1.0 - x_pos) + -neg_z * x_pos;
        let uy = ny + neg_z * (1.0 - y_pos) + -neg_z * y_pos;

        let normal = normalize3([ux, uy, derived_z]);
        normals.push(normal);

        let tangent_sign = if normal[2] >= 0.0 { 1.0 } else { -1.0 };
        let rcp_tangent_z = 1.0 / (tangent_sign + normal[2]);

        let unaligned = [
            -tangent_sign * (normal[0] * normal[0]) * rcp_tangent_z + 1.0,
            -tangent_sign * (normal[0] * normal[1]) * rcp_tangent_z,
            -tangent_sign * normal[0],
        ];

        let angle = t_bits / 2047.0 * TAU;
        let cross = cross3(normal, unaligned);
        let (s, c) = (angle.sin(), angle.cos());
        let tangent = [
            unaligned[0] * c + cross[0] * s,
            unaligned[1] * c + cross[1] * s,
            unaligned[2] * c + cross[2] * s,
        ];

        let w = if sign_bit == 0 { -1.0 } else { 1.0 };
        tangents.push([tangent[0], tangent[1], tangent[2], w]);
    }

    (normals, tangents)
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        return v;
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal classic VBIB block (1 vertex buffer, POSITION-only; 1 u16 index
    /// buffer) parses and decodes, exercising the legacy `vbib_block`/`MBUF` path
    /// that "optimized"/recompiled soul-container mods ship.
    #[test]
    fn parse_vbib_block_decodes_a_minimal_block() {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let indices: [u16; 3] = [0, 1, 2];

        // Section layout: 16B header, vb record @16, ib record @40, attr @64,
        // vertex data @120, index data @156. Offsets are relative to their own
        // field, matching VRF Blocks/VBIB.cs.
        let attr_pos = 64usize;
        let vdata_pos = attr_pos + 56;
        let idata_pos = vdata_pos + positions.len() * 12;

        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&16u32.to_le_bytes()); // vb_off (field @0) -> 16
        b.extend_from_slice(&1u32.to_le_bytes()); // vb_count
        b.extend_from_slice(&32u32.to_le_bytes()); // ib_off (field @8) -> 40
        b.extend_from_slice(&1u32.to_le_bytes()); // ib_count
                                                  // vertex buffer record @16
        b.extend_from_slice(&(positions.len() as u32).to_le_bytes());
        b.extend_from_slice(&12u32.to_le_bytes()); // stride
        b.extend_from_slice(&((attr_pos - 24) as u32).to_le_bytes()); // attr_off (field @24)
        b.extend_from_slice(&1u32.to_le_bytes()); // attr_count
        b.extend_from_slice(&((vdata_pos - 32) as u32).to_le_bytes()); // data_off (field @32)
        b.extend_from_slice(&((positions.len() * 12) as u32).to_le_bytes()); // total_size
                                                                             // index buffer record @40
        b.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes()); // index width
        b.extend_from_slice(&0u32.to_le_bytes()); // attr_off (no attrs)
        b.extend_from_slice(&0u32.to_le_bytes()); // attr_count
        b.extend_from_slice(&((idata_pos - 56) as u32).to_le_bytes()); // data_off (field @56)
        b.extend_from_slice(&((indices.len() * 2) as u32).to_le_bytes()); // total_size
                                                                          // attribute record @64 (56 bytes)
        assert_eq!(b.len(), attr_pos);
        let mut name = [0u8; 32];
        name[..8].copy_from_slice(b"POSITION");
        b.extend_from_slice(&name);
        b.extend_from_slice(&0i32.to_le_bytes()); // semanticIndex
        b.extend_from_slice(&6u32.to_le_bytes()); // R32G32B32_FLOAT
        b.extend_from_slice(&0u32.to_le_bytes()); // offset
        b.extend_from_slice(&0i32.to_le_bytes()); // slot
        b.extend_from_slice(&0u32.to_le_bytes()); // slotType
        b.extend_from_slice(&0i32.to_le_bytes()); // instanceStepRate
                                                  // vertex data @120
        assert_eq!(b.len(), vdata_pos);
        for p in &positions {
            for c in p {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        // index data @156
        assert_eq!(b.len(), idata_pos);
        for i in &indices {
            b.extend_from_slice(&i.to_le_bytes());
        }

        let (vbs, ibs) = parse_vbib_block(&b).expect("parse VBIB");
        assert_eq!(vbs.len(), 1);
        assert_eq!(ibs.len(), 1);
        assert_eq!(vbs[0].element_count, 3);
        assert_eq!(vbs[0].element_size, 12);
        assert_eq!(vbs[0].fields.len(), 1);
        assert_eq!(vbs[0].fields[0].semantic_name, "POSITION");
        assert_eq!(vbs[0].positions().unwrap(), positions);
        assert_eq!(ibs[0].element_count, 3);
        assert_eq!(ibs[0].element_size, 2);
    }

    /// `write_positions` overwrites only the POSITION lane: the new positions
    /// read back, and every other byte of the stride is preserved.
    #[test]
    fn write_positions_touches_only_the_position_lane() {
        // 2 vertices, stride 20: POSITION (3 f32) at offset 4, with 4 marker
        // bytes before and 4 after to prove the rest of the stride is untouched.
        let stride = 20usize;
        let count = 2usize;
        let mut data = vec![0u8; count * stride];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(1); // distinct, non-zero filler
        }
        let mut buf = OnDiskBuffer {
            data: data.clone(),
            element_count: count,
            element_size: stride,
            fields: vec![InputLayoutField {
                semantic_name: "POSITION".to_string(),
                semantic_index: 0,
                format: DxgiFormat::R32G32B32Float,
                offset: 4,
            }],
        };

        let new = [[1.5f32, -2.0, 3.25], [10.0, 20.0, 30.0]];
        buf.write_positions(&new).expect("write positions");

        // Positions read back exactly.
        assert_eq!(buf.positions().expect("positions"), new);

        // Bytes outside the POSITION lane (offsets 0..4 and 16..20 of each
        // vertex) are unchanged from the original filler.
        for v in 0..count {
            let base = v * stride;
            assert_eq!(
                &buf.data[base..base + 4],
                &data[base..base + 4],
                "pre-bytes"
            );
            assert_eq!(
                &buf.data[base + 16..base + 20],
                &data[base + 16..base + 20],
                "post-bytes"
            );
        }
    }

    /// `write_colors` overwrites only the 4-byte COLOR lane (`R8G8B8A8_UNORM`),
    /// the values round-trip through `vector4` within the u8 quantum, and every
    /// other byte of the stride is preserved.
    #[test]
    fn write_colors_touches_only_the_color_lane() {
        // 2 vertices, stride 16: COLOR (4 u8) at offset 8, with marker bytes
        // before (0..8) and after (12..16) to prove the rest is untouched.
        let stride = 16usize;
        let count = 2usize;
        let mut data = vec![0u8; count * stride];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(1); // distinct, non-zero filler
        }
        let color = InputLayoutField {
            semantic_name: "COLOR".to_string(),
            semantic_index: 0,
            format: DxgiFormat::R8G8B8A8Unorm,
            offset: 8,
        };
        let mut buf = OnDiskBuffer {
            data: data.clone(),
            element_count: count,
            element_size: stride,
            fields: vec![color.clone()],
        };

        let new = [[0.0f32, 0.5, 1.0, 1.0], [1.0, 0.25, 0.0, 0.75]];
        buf.write_colors(&color, &new).expect("write colors");

        // Colors read back within the unorm quantum.
        let got = buf.vector4(&color).expect("read colors");
        for (g, w) in got.iter().zip(&new) {
            for k in 0..4 {
                assert!((g[k] - w[k]).abs() <= 1.0 / 255.0, "{g:?} vs {w:?}");
            }
        }

        // Bytes outside the COLOR lane (0..8 and 12..16 of each vertex) unchanged.
        for v in 0..count {
            let base = v * stride;
            assert_eq!(
                &buf.data[base..base + 8],
                &data[base..base + 8],
                "pre-bytes"
            );
            assert_eq!(
                &buf.data[base + 12..base + 16],
                &data[base + 12..base + 16],
                "post-bytes"
            );
        }
    }

    /// `write_colors` rejects a count that does not match the vertex count.
    #[test]
    fn write_colors_rejects_count_mismatch() {
        let color = InputLayoutField {
            semantic_name: "COLOR".to_string(),
            semantic_index: 0,
            format: DxgiFormat::R8G8B8A8Unorm,
            offset: 0,
        };
        let mut buf = OnDiskBuffer {
            data: vec![0u8; 4],
            element_count: 1,
            element_size: 4,
            fields: vec![color.clone()],
        };
        assert!(buf.write_colors(&color, &[[0.0; 4], [0.0; 4]]).is_err());
    }

    #[test]
    fn write_positions_rejects_count_mismatch() {
        let mut buf = OnDiskBuffer {
            data: vec![0u8; 12],
            element_count: 1,
            element_size: 12,
            fields: vec![InputLayoutField {
                semantic_name: "POSITION".to_string(),
                semantic_index: 0,
                format: DxgiFormat::R32G32B32Float,
                offset: 0,
            }],
        };
        assert!(buf.write_positions(&[[0.0; 3], [0.0; 3]]).is_err());
    }
}

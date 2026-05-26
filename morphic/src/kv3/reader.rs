//! Binary KV3 reader, ported from `ValveResourceFormat`'s `BinaryKV3` (MIT).
//!
//! Handles versions 1..=5. Real Deadlock files (e.g. `.vsndevts_c`) are v5 with
//! LZ4-compressed buffers; our own encoder emits v4 uncompressed. Both paths are
//! exercised by the round-trip test.
//!
//! Two simplifications vs the reference, neither reached by soundevents data:
//! - KV3 value flags (`Resource`, `SoundEvent`, ...) are consumed but discarded;
//!   the [`Value`](super::Value) tree has no slot for them.
//! - Binary blobs (`countBlocks > 0`) are rejected. No soundevents file ships
//!   them, and our encoder never emits them for these trees.

// The KV3 wire format reinterprets the same bytes as signed/unsigned of various
// widths; these casts are the intended bit-for-bit reinterpretations.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use super::node;
use super::types::Value;
use crate::error::DecodeError;

/// Legacy pre-versioned VKV3 magic (0x03 'V' 'K' 'V'). Unsupported.
const MAGIC_LEGACY: u32 = 0x0356_4B56;
/// High 24 bits shared by all versioned magics: 'V' 'K' '3' little-endian with
/// the version in the low byte (`05 33 56 4B` = v5).
const MAGIC_BASE: u32 = 0x4B56_3300;
const TRAILER: u32 = 0xFFEE_DD00;
const LZ4_FRAME_SIZE: u16 = 16384;

/// Decode a binary KV3 DATA payload into a [`Value`] tree.
#[allow(clippy::too_many_lines)]
pub(super) fn decode(data: &[u8]) -> Result<Value, DecodeError> {
    let mut h = Cursor::new(data);
    let magic = h.u32()?;
    if magic == MAGIC_LEGACY {
        return Err(DecodeError::UnsupportedKv3(magic));
    }
    let version = magic & 0xFF;
    if magic & 0xFFFF_FF00 != MAGIC_BASE || !(1..=5).contains(&version) {
        return Err(DecodeError::UnsupportedKv3(magic));
    }

    let _format_guid = h.bytes(16)?;
    let compression = h.u32()?;

    // Header counts. Field presence is version-gated exactly as in VRF.
    let (mut count_b1, mut count_b4, mut count_b8);
    let mut count_types = 0i64;
    let mut count_blocks = 0i64;
    let size_unc_total: i64;
    let size_comp_total: i64;

    if version == 1 {
        count_b1 = h.i32()?;
        count_b4 = h.i32()?;
        count_b8 = h.i32()?;
        size_unc_total = h.i32()?;
        size_comp_total = (data.len() - h.pos) as i64;
    } else {
        let _dict = h.u16()?;
        let frame = h.u16()?;
        if compression == 1 && frame != LZ4_FRAME_SIZE {
            return Err(DecodeError::Kv3("unexpected LZ4 frame size"));
        }
        count_b1 = h.i32()?;
        count_b4 = h.i32()?;
        count_b8 = h.i32()?;
        count_types = h.i32()?;
        let _count_objects = h.u16()?;
        let _count_arrays = h.u16()?;
        size_unc_total = h.i32()?;
        size_comp_total = h.i32()?;
        count_blocks = h.i32()?;
        let _size_blobs = h.i32()?;
    }

    let mut count_b2 = 0i64;
    if version >= 4 {
        count_b2 = h.i32()?;
        let _size_block_compressed = h.i32()?;
    }

    // v5 splits everything into two buffers: an auxiliary buffer (strings +
    // primitive arrays) and a main buffer (object lengths, scalars, types).
    let mut size_unc_buf2 = 0i64;
    let mut size_comp_buf2 = 0i64;
    let mut aux_counts = SubCounts::default();
    let mut main_obj_count = 0i64;
    let (size_unc_buf1, size_comp_buf1);

    if version >= 5 {
        size_unc_buf1 = h.i32()?;
        size_comp_buf1 = h.i32()?;
        size_unc_buf2 = h.i32()?;
        size_comp_buf2 = h.i32()?;
        // The header's first count block describes the auxiliary buffer in v5.
        aux_counts = SubCounts {
            b1: count_b1,
            b2: count_b2,
            b4: count_b4,
            b8: count_b8,
        };
        let main_b1 = h.i32()?;
        let main_b2 = h.i32()?;
        let main_b4 = h.i32()?;
        let main_b8 = h.i32()?;
        let _unk13 = h.i32()?;
        main_obj_count = h.i32()?;
        let _count_arrays2 = h.i32()?;
        let _unk16 = h.i32()?;
        // After the header, count_b* now mean the main buffer's counts.
        count_b1 = main_b1;
        count_b2 = main_b2;
        count_b4 = main_b4;
        count_b8 = main_b8;
    } else {
        size_unc_buf1 = size_unc_total;
        size_comp_buf1 = size_comp_total;
    }

    if count_blocks > 0 {
        return Err(DecodeError::Kv3(
            "binary blobs (countBlocks > 0) unsupported",
        ));
    }

    // Decompress the buffers off the stream, in order.
    let buf1 = read_buffer(&mut h, compression, size_unc_buf1, size_comp_buf1)?;
    let buf2 = if version >= 5 {
        read_buffer(&mut h, compression, size_unc_buf2, size_comp_buf2)?
    } else {
        Vec::new()
    };

    // Carve the sub-buffers out of the decompressed bytes.
    let mut ctx = if version >= 5 {
        let (aux, strings) = layout_aux_v5(&buf1, &aux_counts)?;
        let main = SubCounts {
            b1: count_b1,
            b2: count_b2,
            b4: count_b4,
            b8: count_b8,
        };
        let (main_buffers, object_lengths, types) =
            layout_main_v5(&buf2, &main, main_obj_count, count_types)?;
        Ctx {
            version,
            strings,
            types: Cursor::new(types),
            object_lengths: Cursor::new(object_lengths),
            main: main_buffers,
            aux,
        }
    } else {
        layout_single(
            &buf1,
            version,
            count_b1,
            count_b2,
            count_b4,
            count_b8,
            count_types,
            size_unc_total,
        )?
    };

    let (root_type, _flag) = read_type(&mut ctx)?;
    read_value(&mut ctx, root_type)
}

#[derive(Default, Clone, Copy)]
struct SubCounts {
    b1: i64,
    b2: i64,
    b4: i64,
    b8: i64,
}

/// Four typed sub-buffers (1/2/4/8-byte lanes) the value tree pulls from.
struct Buffers<'a> {
    b1: Cursor<'a>,
    b2: Cursor<'a>,
    b4: Cursor<'a>,
    b8: Cursor<'a>,
}

struct Ctx<'a> {
    version: u32,
    strings: Vec<String>,
    types: Cursor<'a>,
    /// v5 only: per-OBJECT member counts. Empty for v<5 (counts come from b4).
    object_lengths: Cursor<'a>,
    main: Buffers<'a>,
    aux: Buffers<'a>,
}

fn read_buffer(
    h: &mut Cursor,
    compression: u32,
    size_unc: i64,
    size_comp: i64,
) -> Result<Vec<u8>, DecodeError> {
    let unc = usize::try_from(size_unc).map_err(|_| DecodeError::Kv3("negative buffer size"))?;
    match compression {
        0 => {
            let raw = h.bytes(unc)?;
            Ok(raw.to_vec())
        }
        1 => {
            let comp =
                usize::try_from(size_comp).map_err(|_| DecodeError::Kv3("negative comp size"))?;
            let input = h.bytes(comp)?;
            let mut out = vec![0u8; unc];
            let n = lz4_flex::block::decompress_into(input, &mut out)
                .map_err(|e| DecodeError::Kv3Lz4(e.to_string()))?;
            if n != unc {
                return Err(DecodeError::Kv3Lz4(format!(
                    "expected {unc} bytes, got {n}"
                )));
            }
            Ok(out)
        }
        other => Err(DecodeError::Kv3Compression(other)),
    }
}

/// v5 auxiliary buffer: `[b1][align2 b2][align4 b4][align8 b8]`, where the
/// string table is the null-terminated run at the front of b1, and the string
/// count is the first int of b4.
fn layout_aux_v5<'a>(
    buf: &'a [u8],
    counts: &SubCounts,
) -> Result<(Buffers<'a>, Vec<String>), DecodeError> {
    let mut off = 0usize;
    let b1_start = off;
    off += usize_of(counts.b1)?;
    let (b2_start, b2_len) = lane(&mut off, counts.b2, 2)?;
    let (b4_start, b4_len) = lane(&mut off, counts.b4, 4)?;
    let (b8_start, b8_len) = lane(&mut off, counts.b8, 8)?;

    // String count is the first int of b4.
    let count = read_i32_at(buf, b4_start)? as usize;
    let mut sp = b1_start;
    let mut strings = Vec::with_capacity(count);
    for _ in 0..count {
        strings.push(read_cstr(buf, &mut sp)?);
    }

    let aux = Buffers {
        b1: Cursor::new(slice(buf, sp, b1_start + usize_of(counts.b1)? - sp)?),
        b2: Cursor::new(slice(buf, b2_start, b2_len)?),
        // b4 starts after the consumed string count.
        b4: Cursor::new(slice(buf, b4_start + 4, b4_len.saturating_sub(4))?),
        b8: Cursor::new(slice(buf, b8_start, b8_len)?),
    };
    Ok((aux, strings))
}

/// v5 main buffer: `[object_lengths][b1][align2 b2][align4 b4][align8 b8][types][trailer]`.
fn layout_main_v5<'a>(
    buf: &'a [u8],
    counts: &SubCounts,
    obj_count: i64,
    count_types: i64,
) -> Result<(Buffers<'a>, &'a [u8], &'a [u8]), DecodeError> {
    let mut off = 0usize;
    let ol_start = off;
    let ol_len = usize_of(obj_count)? * 4;
    off += ol_len;
    let b1_start = off;
    off += usize_of(counts.b1)?;
    let (b2_start, b2_len) = lane(&mut off, counts.b2, 2)?;
    let (b4_start, b4_len) = lane(&mut off, counts.b4, 4)?;
    let (b8_start, b8_len) = lane(&mut off, counts.b8, 8)?;
    let types_start = off;
    let types_len = usize_of(count_types)?;
    off += types_len;
    check_trailer(buf, off)?;

    let buffers = Buffers {
        b1: Cursor::new(slice(buf, b1_start, usize_of(counts.b1)?)?),
        b2: Cursor::new(slice(buf, b2_start, b2_len)?),
        b4: Cursor::new(slice(buf, b4_start, b4_len)?),
        b8: Cursor::new(slice(buf, b8_start, b8_len)?),
    };
    let object_lengths = slice(buf, ol_start, ol_len)?;
    let types = slice(buf, types_start, types_len)?;
    Ok((buffers, object_lengths, types))
}

/// v1..=4 single buffer: `[b1][align2 b2][align4 b4][align8 b8][strings][types][trailer]`.
/// Strings and types are inline regions (not sub-buffers); object lengths come
/// from b4 at read time.
#[allow(clippy::too_many_arguments)]
fn layout_single(
    buf: &[u8],
    version: u32,
    count_b1: i64,
    count_b2: i64,
    count_b4: i64,
    count_b8: i64,
    count_types: i64,
    size_unc_total: i64,
) -> Result<Ctx<'_>, DecodeError> {
    let mut off = 0usize;
    let b1_start = off;
    off += usize_of(count_b1)?;
    let (b2_start, b2_len) = lane(&mut off, count_b2, 2)?;
    let (b4_start, b4_len) = lane(&mut off, count_b4, 4)?;
    let (b8_start, b8_len) = if count_b8 > 0 {
        lane(&mut off, count_b8, 8)?
    } else {
        align(&mut off, 8);
        (off, 0)
    };

    let count = read_i32_at(buf, b4_start)? as usize;
    let strings_start = off;
    let mut sp = off;
    let mut strings = Vec::with_capacity(count);
    for _ in 0..count {
        strings.push(read_cstr(buf, &mut sp)?);
    }
    off = sp;

    let types_len = if version == 1 {
        usize_of(size_unc_total)?
            .saturating_sub(off)
            .saturating_sub(4)
    } else {
        // count_types stores (string bytes + type bytes); subtract the string
        // bytes consumed so far to recover the type-byte count.
        usize_of(count_types)?.saturating_sub(off - strings_start)
    };
    let types = slice(buf, off, types_len)?;
    off += types_len;
    check_trailer(buf, off)?;

    let main = Buffers {
        b1: Cursor::new(slice(buf, b1_start, usize_of(count_b1)?)?),
        b2: Cursor::new(slice(buf, b2_start, b2_len)?),
        // Skip the leading string count.
        b4: Cursor::new(slice(buf, b4_start + 4, b4_len.saturating_sub(4))?),
        b8: Cursor::new(slice(buf, b8_start, b8_len)?),
    };
    Ok(Ctx {
        version,
        strings,
        types: Cursor::new(types),
        object_lengths: Cursor::new(&[]),
        main,
        aux: Buffers {
            b1: Cursor::new(&[]),
            b2: Cursor::new(&[]),
            b4: Cursor::new(&[]),
            b8: Cursor::new(&[]),
        },
    })
}

fn read_type(ctx: &mut Ctx) -> Result<(u8, u8), DecodeError> {
    let mut databyte = ctx.types.u8()?;
    let mut flag = 0u8;
    if databyte & 0x80 != 0 {
        // v>=3 masks 0x3F, older versions 0x7F; node ids are < 0x3F so the
        // result is identical for every type we model. The flag byte is
        // consumed (to stay aligned) but not retained.
        databyte &= if ctx.version >= 3 { 0x3F } else { 0x7F };
        flag = ctx.types.u8()?;
    }
    Ok((databyte, flag))
}

#[allow(clippy::wildcard_imports)]
fn read_value(ctx: &mut Ctx, datatype: u8) -> Result<Value, DecodeError> {
    use node::*;
    match datatype {
        NULL => Ok(Value::Null),
        BOOLEAN_TRUE => Ok(Value::Bool(true)),
        BOOLEAN_FALSE => Ok(Value::Bool(false)),
        INT64_ZERO => Ok(Value::Int(0)),
        INT64_ONE => Ok(Value::Int(1)),
        DOUBLE_ZERO => Ok(Value::Double(0.0)),
        DOUBLE_ONE => Ok(Value::Double(1.0)),
        BOOLEAN => Ok(Value::Bool(ctx.main.b1.u8()? == 1)),
        INT32_AS_BYTE => Ok(Value::Int(i64::from(ctx.main.b1.u8()?))),
        INT16 => Ok(Value::Int(i64::from(ctx.main.b2.u16()? as i16))),
        UINT16 => Ok(Value::UInt(u64::from(ctx.main.b2.u16()?))),
        INT32 => Ok(Value::Int(i64::from(ctx.main.b4.u32()? as i32))),
        UINT32 => Ok(Value::UInt(u64::from(ctx.main.b4.u32()?))),
        FLOAT => Ok(Value::Double(f64::from(f32::from_bits(ctx.main.b4.u32()?)))),
        INT64 => Ok(Value::Int(ctx.main.b8.u64()? as i64)),
        UINT64 => Ok(Value::UInt(ctx.main.b8.u64()?)),
        DOUBLE => Ok(Value::Double(f64::from_bits(ctx.main.b8.u64()?))),
        STRING => {
            let id = ctx.main.b4.u32()? as i32;
            Ok(Value::String(lookup_string(ctx, id)?))
        }
        ARRAY => {
            let n = ctx.main.b4.u32()?;
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let (t, _f) = read_type(ctx)?;
                items.push(read_value(ctx, t)?);
            }
            Ok(Value::Array(items))
        }
        ARRAY_TYPED | ARRAY_TYPE_BYTE_LENGTH => {
            let n = if datatype == ARRAY_TYPE_BYTE_LENGTH {
                u32::from(ctx.main.b1.u8()?)
            } else {
                ctx.main.b4.u32()?
            };
            let (sub, _f) = read_type(ctx)?;
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                items.push(read_value(ctx, sub)?);
            }
            Ok(Value::Array(items))
        }
        ARRAY_TYPE_AUXILIARY_BUFFER => {
            let n = u32::from(ctx.main.b1.u8()?);
            let (sub, _f) = read_type(ctx)?;
            std::mem::swap(&mut ctx.main, &mut ctx.aux);
            let mut items = Vec::with_capacity(n as usize);
            let mut err = None;
            for _ in 0..n {
                match read_value(ctx, sub) {
                    Ok(v) => items.push(v),
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
            std::mem::swap(&mut ctx.main, &mut ctx.aux);
            if let Some(e) = err {
                return Err(e);
            }
            Ok(Value::Array(items))
        }
        OBJECT => {
            let n = if ctx.version >= 5 {
                ctx.object_lengths.u32()?
            } else {
                ctx.main.b4.u32()?
            };
            let mut pairs = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let (vt, _f) = read_type(ctx)?;
                let id = ctx.main.b4.u32()? as i32;
                let name = lookup_string(ctx, id)?;
                pairs.push((name, read_value(ctx, vt)?));
            }
            Ok(Value::Object(pairs))
        }
        other => Err(DecodeError::Kv3NodeType(other)),
    }
}

fn lookup_string(ctx: &Ctx, id: i32) -> Result<String, DecodeError> {
    if id == -1 {
        return Ok(String::new());
    }
    ctx.strings
        .get(id as usize)
        .cloned()
        .ok_or(DecodeError::Kv3("string id out of range"))
}

// --- byte-slice helpers ---------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

#[allow(clippy::needless_lifetimes)]
impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(DecodeError::Kv3("overflow"))?;
        if end > self.data.len() {
            return Err(DecodeError::Truncated {
                offset: self.pos as u64,
                needed: n,
                had: self.data.len().saturating_sub(self.pos),
            });
        }
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        self.take(n)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from(self.u32()? as i32))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

fn align(off: &mut usize, alignment: usize) {
    let a = alignment - 1;
    *off = (*off + a) & !a;
}

/// Advance `off` past an aligned typed lane of `count` items of `elem` bytes,
/// returning `(start, byte_len)`.
fn lane(off: &mut usize, count: i64, elem: usize) -> Result<(usize, usize), DecodeError> {
    if count <= 0 {
        return Ok((*off, 0));
    }
    align(off, elem);
    let start = *off;
    let len = usize_of(count)? * elem;
    *off += len;
    Ok((start, len))
}

fn slice(buf: &[u8], start: usize, len: usize) -> Result<&[u8], DecodeError> {
    let end = start.checked_add(len).ok_or(DecodeError::Kv3("overflow"))?;
    buf.get(start..end)
        .ok_or(DecodeError::Kv3("sub-buffer out of range"))
}

fn read_i32_at(buf: &[u8], at: usize) -> Result<i32, DecodeError> {
    let b = slice(buf, at, 4)?;
    Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_cstr(buf: &[u8], pos: &mut usize) -> Result<String, DecodeError> {
    let start = *pos;
    while *pos < buf.len() && buf[*pos] != 0 {
        *pos += 1;
    }
    if *pos >= buf.len() {
        return Err(DecodeError::Kv3("unterminated string"));
    }
    let s = String::from_utf8_lossy(&buf[start..*pos]).into_owned();
    *pos += 1; // skip NUL
    Ok(s)
}

fn check_trailer(buf: &[u8], at: usize) -> Result<(), DecodeError> {
    let t = read_u32_at(buf, at)?;
    if t != TRAILER {
        return Err(DecodeError::Kv3("missing 0xFFEEDD00 trailer"));
    }
    Ok(())
}

fn read_u32_at(buf: &[u8], at: usize) -> Result<u32, DecodeError> {
    let b = slice(buf, at, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn usize_of(v: i64) -> Result<usize, DecodeError> {
    usize::try_from(v).map_err(|_| DecodeError::Kv3("negative count"))
}

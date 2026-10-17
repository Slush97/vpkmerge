//! Pre-versioned VKV3 wire reader. Format reference: `ValveResourceFormat`'s
//! MIT-licensed `BinaryKV3.Legacy` and `BlockCompress` readers. This feeds the same
//! value/flag representation as the versioned reader.
use super::{node, Seg, Value, ValueFlags};
use crate::error::DecodeError;

const RAW: [u8; 16] = [
    0x00, 0x05, 0x86, 0x1b, 0xd8, 0xf7, 0xc1, 0x40, 0xad, 0x82, 0x75, 0xa4, 0x82, 0x67, 0xe7, 0x14,
];
const LZ4: [u8; 16] = [
    0x8a, 0x34, 0x47, 0x68, 0xa1, 0x63, 0x5c, 0x4f, 0xa1, 0x97, 0x53, 0x80, 0x6f, 0xd9, 0xb1, 0x19,
];
const BLOCK: [u8; 16] = [
    0x46, 0x1a, 0x79, 0x95, 0xbc, 0x95, 0x6c, 0x4f, 0xa7, 0x0b, 0x05, 0xbc, 0xa1, 0xb7, 0xdf, 0xd2,
];
const INVALID: DecodeError = DecodeError::Kv3("Invalid legacy KV3 document");

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    remaining_nodes: usize,
}
impl Cursor<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], DecodeError> {
        let end = self.at.checked_add(count).ok_or(INVALID)?;
        let bytes = self.bytes.get(self.at..end).ok_or(INVALID)?;
        self.at = end;
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        self.take(N)?.try_into().map_err(|_| INVALID)
    }
    fn count(&mut self) -> Result<usize, DecodeError> {
        let count = usize::try_from(u32::from_le_bytes(self.fixed()?)).map_err(|_| INVALID)?;
        if count > 128 * 1024 * 1024 {
            return Err(INVALID);
        }
        Ok(count)
    }
    fn string(&mut self) -> Result<String, DecodeError> {
        let length = self.bytes[self.at..]
            .iter()
            .position(|b| *b == 0)
            .ok_or(INVALID)?;
        let text = std::str::from_utf8(self.take(length)?)
            .map_err(|_| INVALID)?
            .to_owned();
        self.byte()?;
        Ok(text)
    }
}

fn body(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let encoding = data.get(4..20).ok_or(INVALID)?;
    let mut cursor = Cursor {
        bytes: data.get(36..).ok_or(INVALID)?,
        at: 0,
        remaining_nodes: 1_000_000,
    };
    if encoding == RAW {
        return Ok(cursor.bytes.to_vec());
    }
    let header = u32::from_le_bytes(cursor.fixed()?);
    let size = usize::try_from(header & 0x7fff_ffff).map_err(|_| INVALID)?;
    if size > 128 * 1024 * 1024 {
        return Err(INVALID);
    }
    if encoding == LZ4 {
        return lz4_flex::block::decompress(&cursor.bytes[cursor.at..], size).map_err(|_| INVALID);
    }
    if encoding != BLOCK {
        return Err(DecodeError::Kv3("Unsupported legacy KV3 encoding"));
    }
    if header & 0x8000_0000 != 0 {
        return Ok(cursor.take(size)?.to_vec());
    }
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let mut bits = u16::from_le_bytes(cursor.fixed()?);
        for _ in 0..16 {
            if out.len() == size {
                break;
            }
            if bits & 1 == 0 {
                out.push(cursor.byte()?);
            } else {
                let word = u16::from_le_bytes(cursor.fixed()?);
                let offset = usize::from(word >> 4) + 1;
                let length = usize::from(word & 15) + 3;
                if offset > out.len() || length > size - out.len() {
                    return Err(INVALID);
                }
                for _ in 0..length {
                    out.push(out[out.len() - offset]);
                }
            }
            bits >>= 1;
        }
    }
    Ok(out)
}

fn tag(cursor: &mut Cursor) -> Result<(u8, u8), DecodeError> {
    let tag = cursor.byte()?;
    let flag = if tag & 0x80 != 0 {
        // Legacy flags are a bitmask. Only flags with an unambiguous modern
        // equivalent are accepted; multiline is a text-formatting hint.
        match cursor.byte()? & !4 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => return Err(DecodeError::Kv3("Unsupported legacy KV3 semantic flag")),
        }
    } else {
        0
    };
    Ok((tag & 0x7f, flag))
}

fn lookup(cursor: &mut Cursor, strings: &[String]) -> Result<String, DecodeError> {
    let index = i32::from_le_bytes(cursor.fixed()?);
    if index == -1 {
        return Ok(String::new());
    }
    strings
        .get(usize::try_from(index).map_err(|_| INVALID)?)
        .cloned()
        .ok_or(INVALID)
}

#[allow(clippy::too_many_lines)]
fn value(
    cursor: &mut Cursor,
    strings: &[String],
    datatype: u8,
    flag: u8,
    path: &mut Vec<Seg>,
    flags: &mut ValueFlags,
) -> Result<Value, DecodeError> {
    if path.len() > 96 || cursor.remaining_nodes == 0 {
        return Err(INVALID);
    }
    cursor.remaining_nodes -= 1;
    if flag != 0 {
        flags.insert(path.clone(), flag);
    }
    Ok(match datatype {
        node::NULL => Value::Null,
        node::BOOLEAN => Value::Bool(cursor.byte()? != 0),
        node::BOOLEAN_TRUE => Value::Bool(true),
        node::BOOLEAN_FALSE => Value::Bool(false),
        node::INT64_ZERO => Value::Int(0),
        node::INT64_ONE => Value::Int(1),
        node::DOUBLE_ZERO => Value::Double(0.),
        node::DOUBLE_ONE => Value::Double(1.),
        node::INT64 => Value::Int(i64::from_le_bytes(cursor.fixed()?)),
        node::UINT64 => Value::UInt(u64::from_le_bytes(cursor.fixed()?)),
        node::INT32 => Value::Int(i64::from(i32::from_le_bytes(cursor.fixed()?))),
        node::UINT32 => Value::UInt(u64::from(u32::from_le_bytes(cursor.fixed()?))),
        node::DOUBLE => Value::Double(f64::from_le_bytes(cursor.fixed()?)),
        node::STRING => Value::String(lookup(cursor, strings)?),
        node::BINARY_BLOB => {
            let length = cursor.count()?;
            Value::Binary(cursor.take(length)?.to_vec())
        }
        node::ARRAY | node::ARRAY_TYPED => {
            let length = cursor.count()?;
            let typed = if datatype == node::ARRAY_TYPED {
                Some(tag(cursor)?)
            } else {
                None
            };
            let mut items = Vec::new();
            for index in 0..length {
                let (datatype, flag) = if let Some(typed) = typed {
                    typed
                } else {
                    tag(cursor)?
                };
                path.push(Seg::Index(index));
                items.push(value(cursor, strings, datatype, flag, path, flags)?);
                path.pop();
            }
            Value::Array(items)
        }
        node::OBJECT => {
            let length = cursor.count()?;
            let mut pairs = Vec::new();
            for _ in 0..length {
                let key = lookup(cursor, strings)?;
                let (datatype, flag) = tag(cursor)?;
                path.push(Seg::Key(key.clone()));
                pairs.push((key, value(cursor, strings, datatype, flag, path, flags)?));
                path.pop();
            }
            Value::Object(pairs)
        }
        other => return Err(DecodeError::Kv3NodeType(other)),
    })
}

pub(super) fn decode(data: &[u8]) -> Result<(Value, ValueFlags), DecodeError> {
    let body = body(data)?;
    let mut cursor = Cursor {
        bytes: &body,
        at: 0,
        remaining_nodes: 1_000_000,
    };
    let length = cursor.count()?;
    let mut strings = Vec::new();
    for _ in 0..length {
        strings.push(cursor.string()?);
    }
    let (datatype, flag) = tag(&mut cursor)?;
    let mut flags = ValueFlags::new();
    let result = value(
        &mut cursor,
        &strings,
        datatype,
        flag,
        &mut Vec::new(),
        &mut flags,
    )?;
    if cursor.fixed::<4>()? != [0xff; 4] || cursor.at != body.len() {
        return Err(INVALID);
    }
    Ok((result, flags))
}

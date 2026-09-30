// Byte-exact KV3 v5 subtree transplant for model MDAT blocks.
//
// Copies the encoded subtree at a root key (default `m_attachments`) from a donor
// MDAT into every MDAT of a target model, lane by lane (main + auxiliary buffers,
// object lengths, types), remapping only string ids. No decode/re-encode, so every
// wire type, flag, typed/aux array form is exactly the donor's.
//
// usage:
//   mdat_transplant stats <file.vmdl_c>
//   mdat_transplant selftest <file.vmdl_c>
//   mdat_transplant graft <target.vmdl_c> <donor.vmdl_c> <donor_block_idx> <out.vmdl_c> [key]
//   mdat_transplant graft-data <target.vmdl_c> <donor.vmdl_c> <out.vmdl_c> <key>
//   mdat_transplant meshgroup-masks <in.vmdl_c> <out.vmdl_c> <default_mask> <old_bit=new_mask>...
//   mdat_transplant retally <orig.vmdl_c> <in.vmdl_c> <out.vmdl_c>
//   mdat_transplant set-number <in.vmdl_c> <out.vmdl_c> <FOURCC> <path/#idx/...> <value>
use morphic::resource::Resource;

const HEADER: usize = 120;
const B1: usize = 0;
const B2: usize = 1;
const B4: usize = 2;
const B8: usize = 3;

const NULL: u8 = 1;
const BOOLEAN: u8 = 2;
const INT64: u8 = 3;
const UINT64: u8 = 4;
const DOUBLE: u8 = 5;
const STRING: u8 = 6;
const ARRAY: u8 = 8;
const OBJECT: u8 = 9;
const ARRAY_TYPED: u8 = 10;
const INT32: u8 = 11;
const UINT32: u8 = 12;
const BTRUE: u8 = 13;
const BFALSE: u8 = 14;
const I64Z: u8 = 15;
const I64O: u8 = 16;
const DZ: u8 = 17;
const DO: u8 = 18;
const FLOAT: u8 = 19;
const INT16: u8 = 20;
const UINT16: u8 = 21;
const I32B: u8 = 23;
const ABL: u8 = 24;
const AAUX: u8 = 25;

fn i32_at(b: &[u8], o: usize) -> usize {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize
}
fn put_i32(b: &mut [u8], o: usize, v: usize) {
    b[o..o + 4].copy_from_slice(&(v as i32).to_le_bytes());
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn align_to(off: usize, a: usize) -> usize {
    off.div_ceil(a) * a
}
fn pad_to(v: &mut Vec<u8>, a: usize) {
    while v.len() % a != 0 {
        v.push(0);
    }
}

#[derive(Clone)]
struct Doc {
    header: Vec<u8>,
    strings: Vec<String>,
    aux: [Vec<u8>; 4],
    main: [Vec<u8>; 4],
    obj: Vec<u8>,
    types: Vec<u8>,
    tail: Vec<u8>,
}

fn parse(raw: &[u8]) -> Doc {
    let u = morphic::kv3::rewrap_uncompressed(raw).unwrap();
    assert_eq!(u32_at(&u, 0) & 0xFF, 5, "KV3 v5 only");
    assert_eq!(i32_at(&u, 56), 0, "blob-bearing block");
    let unc1 = i32_at(&u, 72);
    let unc2 = i32_at(&u, 80);
    assert_eq!(u.len(), HEADER + unc1 + unc2, "unexpected bytes after buf2");
    let buf1 = &u[HEADER..HEADER + unc1];
    let buf2 = &u[HEADER + unc1..];

    let (c1, c4, c8, c2) = (i32_at(&u, 28), i32_at(&u, 32), i32_at(&u, 36), i32_at(&u, 64));
    let mut off = c1;
    let take = |off: &mut usize, n: usize, a: usize| -> (usize, usize) {
        if n == 0 {
            return (*off, 0);
        }
        *off = align_to(*off, a);
        let s = *off;
        *off += n * a;
        (s, n * a)
    };
    let (b2s, b2l) = take(&mut off, c2, 2);
    let (b4s, b4l) = take(&mut off, c4, 4);
    let (b8s, b8l) = take(&mut off, c8, 8);
    assert_eq!(off, unc1, "buf1 has trailing bytes");
    let nstr = u32_at(buf1, b4s) as usize;
    let mut sp = 0;
    let mut strings = Vec::with_capacity(nstr);
    for _ in 0..nstr {
        let e = sp + buf1[sp..].iter().position(|&c| c == 0).unwrap();
        strings.push(String::from_utf8(buf1[sp..e].to_vec()).unwrap());
        sp = e + 1;
    }
    let aux = [
        buf1[sp..c1].to_vec(),
        buf1[b2s..b2s + b2l].to_vec(),
        buf1[b4s + 4..b4s + b4l].to_vec(),
        buf1[b8s..b8s + b8l].to_vec(),
    ];

    let (m1, m2, m4, m8, mo, ct) = (
        i32_at(&u, 88),
        i32_at(&u, 92),
        i32_at(&u, 96),
        i32_at(&u, 100),
        i32_at(&u, 108),
        i32_at(&u, 40),
    );
    let mut off = mo * 4 + m1;
    let (mb2s, mb2l) = take(&mut off, m2, 2);
    let (mb4s, mb4l) = take(&mut off, m4, 4);
    let (mb8s, mb8l) = take(&mut off, m8, 8);
    let main = [
        buf2[mo * 4..mo * 4 + m1].to_vec(),
        buf2[mb2s..mb2s + mb2l].to_vec(),
        buf2[mb4s..mb4s + mb4l].to_vec(),
        buf2[mb8s..mb8s + mb8l].to_vec(),
    ];
    Doc {
        header: u[..HEADER].to_vec(),
        strings,
        aux,
        main,
        obj: buf2[..mo * 4].to_vec(),
        types: buf2[off..off + ct].to_vec(),
        tail: buf2[off + ct..].to_vec(),
    }
}

fn serialize(d: &Doc) -> Vec<u8> {
    let mut buf1 = Vec::new();
    for s in &d.strings {
        buf1.extend_from_slice(s.as_bytes());
        buf1.push(0);
    }
    buf1.extend_from_slice(&d.aux[B1]);
    let c1 = buf1.len();
    if !d.aux[B2].is_empty() {
        pad_to(&mut buf1, 2);
        buf1.extend_from_slice(&d.aux[B2]);
    }
    pad_to(&mut buf1, 4);
    buf1.extend_from_slice(&(d.strings.len() as u32).to_le_bytes());
    buf1.extend_from_slice(&d.aux[B4]);
    if !d.aux[B8].is_empty() {
        pad_to(&mut buf1, 8);
        buf1.extend_from_slice(&d.aux[B8]);
    }

    let mut buf2 = Vec::new();
    buf2.extend_from_slice(&d.obj);
    buf2.extend_from_slice(&d.main[B1]);
    if !d.main[B2].is_empty() {
        pad_to(&mut buf2, 2);
        buf2.extend_from_slice(&d.main[B2]);
    }
    if !d.main[B4].is_empty() {
        pad_to(&mut buf2, 4);
        buf2.extend_from_slice(&d.main[B4]);
    }
    if !d.main[B8].is_empty() {
        pad_to(&mut buf2, 8);
        buf2.extend_from_slice(&d.main[B8]);
    }
    buf2.extend_from_slice(&d.types);
    buf2.extend_from_slice(&d.tail);

    let mut h = d.header.clone();
    put_i32(&mut h, 20, 0);
    put_i32(&mut h, 28, c1);
    put_i32(&mut h, 32, 1 + d.aux[B4].len() / 4);
    put_i32(&mut h, 36, d.aux[B8].len() / 8);
    put_i32(&mut h, 64, d.aux[B2].len() / 2);
    put_i32(&mut h, 40, d.types.len());
    put_i32(&mut h, 48, buf1.len() + buf2.len());
    put_i32(&mut h, 52, buf1.len() + buf2.len());
    put_i32(&mut h, 72, buf1.len());
    put_i32(&mut h, 76, buf1.len());
    put_i32(&mut h, 80, buf2.len());
    put_i32(&mut h, 84, buf2.len());
    put_i32(&mut h, 88, d.main[B1].len());
    put_i32(&mut h, 92, d.main[B2].len() / 2);
    put_i32(&mut h, 96, d.main[B4].len() / 4);
    put_i32(&mut h, 100, d.main[B8].len() / 8);
    put_i32(&mut h, 108, d.obj.len() / 4);
    let mut out = h;
    out.extend_from_slice(&buf1);
    out.extend_from_slice(&buf2);
    out
}

#[derive(Clone, Copy, Default, Debug)]
struct Snap {
    main: [usize; 4],
    aux: [usize; 4],
    obj: usize,
    types: usize,
}

#[derive(Default, Debug)]
struct Stats {
    objects: usize,
    members: usize,
    arrays: [usize; 4], // ARRAY, TYPED, BYTE_LENGTH, AUX
    elems: [usize; 4],
    strings: usize,
    nodes: usize,
    // per-subtype element tallies of typed/aux arrays, and arrays nested in aux
    typed_subtypes: std::collections::BTreeMap<(u8, u8), (usize, usize)>,
}

struct Walker<'a> {
    d: &'a Doc,
    s: Snap,
    swapped: bool,
    path: Vec<String>,
    target: Vec<String>,
    cap: Option<(Snap, Snap, u8)>,
    cap_swapped: bool,
    refs: Vec<(bool, usize)>,
    stats: Stats,
}

impl<'a> Walker<'a> {
    fn new(d: &'a Doc, target: &[&str]) -> Self {
        Self {
            d,
            s: Snap::default(),
            swapped: false,
            path: Vec::new(),
            target: target.iter().map(|s| (*s).to_string()).collect(),
            cap: None,
            cap_swapped: false,
            refs: Vec::new(),
            stats: Stats::default(),
        }
    }
    fn lane(&mut self, k: usize, n: usize) -> usize {
        let (p, len) = if self.swapped {
            (&mut self.s.aux[k], self.d.aux[k].len())
        } else {
            (&mut self.s.main[k], self.d.main[k].len())
        };
        let at = *p;
        *p += n;
        assert!(*p <= len, "lane overrun");
        at
    }
    fn lane_bytes(&self, k: usize) -> &'a [u8] {
        if self.swapped {
            &self.d.aux[k]
        } else {
            &self.d.main[k]
        }
    }
    fn read_type(&mut self) -> u8 {
        let mut t = self.d.types[self.s.types];
        self.s.types += 1;
        if t & 0x80 != 0 {
            t &= 0x3F;
            self.s.types += 1;
        }
        t
    }
    fn string_ref(&mut self) {
        let at = self.lane(B4, 4);
        self.refs.push((self.swapped, at));
        self.stats.strings += 1;
    }
    fn child(&mut self, seg: String, t: u8) {
        self.path.push(seg);
        let hit = self.path == self.target;
        let start = self.s;
        if hit {
            self.cap_swapped = self.swapped;
        }
        self.value(t);
        if hit {
            self.cap = Some((start, self.s, t));
        }
        self.path.pop();
    }
    fn value(&mut self, t: u8) {
        self.stats.nodes += 1;
        match t {
            NULL | BTRUE | BFALSE | I64Z | I64O | DZ | DO => {}
            BOOLEAN | I32B => {
                self.lane(B1, 1);
            }
            INT16 | UINT16 => {
                self.lane(B2, 2);
            }
            INT32 | UINT32 | FLOAT => {
                self.lane(B4, 4);
            }
            STRING => self.string_ref(),
            INT64 | UINT64 | DOUBLE => {
                self.lane(B8, 8);
            }
            ARRAY | ARRAY_TYPED => {
                let at = self.lane(B4, 4);
                let n = u32_at(self.lane_bytes(B4), at) as usize;
                let kind = usize::from(t == ARRAY_TYPED);
                self.stats.arrays[kind] += 1;
                self.stats.elems[kind] += n;
                if t == ARRAY {
                    for i in 0..n {
                        let ct = self.read_type();
                        self.child(format!("#{i}"), ct);
                    }
                } else {
                    let sub = self.read_type();
                    let e = self.stats.typed_subtypes.entry((t, sub)).or_default();
                    e.0 += 1;
                    e.1 += n;
                    for i in 0..n {
                        self.child(format!("#{i}"), sub);
                    }
                }
            }
            ABL | AAUX => {
                let at = self.lane(B1, 1);
                let n = self.lane_bytes(B1)[at] as usize;
                let kind = if t == ABL { 2 } else { 3 };
                self.stats.arrays[kind] += 1;
                self.stats.elems[kind] += n;
                let sub = self.read_type();
                let e = self.stats.typed_subtypes.entry((t, sub)).or_default();
                e.0 += 1;
                e.1 += n;
                if t == AAUX {
                    self.swapped = !self.swapped;
                }
                for i in 0..n {
                    self.child(format!("#{i}"), sub);
                }
                if t == AAUX {
                    self.swapped = !self.swapped;
                }
            }
            OBJECT => {
                let n = u32_at(&self.d.obj, self.s.obj) as usize;
                self.s.obj += 4;
                self.stats.objects += 1;
                self.stats.members += n;
                for _ in 0..n {
                    let vt = self.read_type();
                    let at = self.lane(B4, 4);
                    self.refs.push((self.swapped, at));
                    let id = u32_at(self.lane_bytes(B4), at);
                    let key = self.d.strings[id as usize].clone();
                    self.child(key, vt);
                }
            }
            other => panic!("unsupported node type {other}"),
        }
    }
    fn run(mut self) -> Self {
        let root = self.read_type();
        self.value(root);
        assert_eq!(self.s.types, self.d.types.len(), "types not fully consumed");
        assert_eq!(self.s.obj, self.d.obj.len(), "objects not fully consumed");
        for k in 0..4 {
            assert_eq!(self.s.main[k], self.d.main[k].len(), "main lane {k} not consumed");
            assert_eq!(self.s.aux[k], self.d.aux[k].len(), "aux lane {k} not consumed");
        }
        self
    }
}

fn splice(t: &[u8], ts: usize, te: usize, ins: &[u8]) -> Vec<u8> {
    let mut v = t[..ts].to_vec();
    v.extend_from_slice(ins);
    v.extend_from_slice(&t[te..]);
    v
}

fn transplant(target: &Doc, donor: &Doc, key: &str) -> Doc {
    let tw = Walker::new(target, &[key]).run();
    let dw = Walker::new(donor, &[key]).run();
    assert!(!tw.cap_swapped && !dw.cap_swapped, "subtree inside an aux array");
    let (ts, te, tt) = tw.cap.expect("key not found in target");
    let (ds, de, dt) = dw.cap.expect("key not found in donor");
    assert_eq!(tt, dt, "subtree root types differ");

    let mut out = target.clone();
    let remap = |id: u32, out: &mut Doc| -> u32 {
        if id == u32::MAX {
            return id;
        }
        let name = &donor.strings[id as usize];
        if let Some(p) = out.strings.iter().position(|s| s == name) {
            p as u32
        } else {
            out.strings.push(name.clone());
            (out.strings.len() - 1) as u32
        }
    };

    let mut main_b4 = donor.main[B4][ds.main[B4]..de.main[B4]].to_vec();
    let mut aux_b4 = donor.aux[B4][ds.aux[B4]..de.aux[B4]].to_vec();
    for &(is_aux, at) in &dw.refs {
        let (buf, base, end) = if is_aux {
            (&mut aux_b4, ds.aux[B4], de.aux[B4])
        } else {
            (&mut main_b4, ds.main[B4], de.main[B4])
        };
        if at < base || at >= end {
            continue;
        }
        let rel = at - base;
        let id = u32_at(buf, rel);
        let new = remap(id, &mut out);
        buf[rel..rel + 4].copy_from_slice(&new.to_le_bytes());
    }

    for k in 0..4 {
        let dm = if k == B4 {
            main_b4.clone()
        } else {
            donor.main[k][ds.main[k]..de.main[k]].to_vec()
        };
        out.main[k] = splice(&target.main[k], ts.main[k], te.main[k], &dm);
        let da = if k == B4 {
            aux_b4.clone()
        } else {
            donor.aux[k][ds.aux[k]..de.aux[k]].to_vec()
        };
        out.aux[k] = splice(&target.aux[k], ts.aux[k], te.aux[k], &da);
    }
    out.obj = splice(&target.obj, ts.obj, te.obj, &donor.obj[ds.obj..de.obj]);
    out.types = splice(&target.types, ts.types, te.types, &donor.types[ds.types..de.types]);
    out
}

fn mdat_blocks(res: &Resource) -> Vec<usize> {
    res.blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.kind == b"MDAT")
        .map(|(i, _)| i)
        .collect()
}

/// The header tallies as fitted on stock + Midnight blocks: @44 u16 objects,
/// @46 u16 all arrays, @104 members+1, @108 objects, @112 non-aux arrays,
/// @116 non-aux array elements + untyped array count. Exact on every MDAT and on
/// Midnight's as-compiled DATA; stock DATA (INT32/UINT32 aux arrays) deviates.
fn fit(s: &Stats) -> String {
    format!(
        "@44={} @46={} @104={} @108={} @112={} @116={}",
        s.objects,
        s.arrays.iter().sum::<usize>(),
        s.members + 1,
        s.objects,
        s.arrays[0] + s.arrays[1] + s.arrays[2],
        s.elems[0] + s.elems[1] + s.elems[2] + s.arrays[0]
    )
}

fn header_fields(u: &[u8]) -> String {
    let u16_at = |o: usize| u16::from_le_bytes(u[o..o + 2].try_into().unwrap());
    format!(
        "@44={} @46={} @104={} @108={} @112={} @116={}",
        u16_at(44),
        u16_at(46),
        i32_at(u, 104),
        i32_at(u, 108),
        i32_at(u, 112),
        i32_at(u, 116)
    )
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "stats" | "selftest" => {
            let bytes = std::fs::read(&a[2]).unwrap();
            let res = Resource::parse(&bytes).unwrap();
            let mut idxs = mdat_blocks(&res);
            if a.get(3).is_some_and(|m| m == "data") {
                idxs = res
                    .blocks()
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| &b.kind == b"DATA")
                    .map(|(i, _)| i)
                    .collect();
            }
            for i in idxs {
                let raw = res.get_block_by_index(i).unwrap();
                let d = parse(raw);
                let u = morphic::kv3::rewrap_uncompressed(raw).unwrap();
                let same = serialize(&d) == u;
                let w = Walker::new(&d, &[]).run();
                println!(
                    "block {i}: roundtrip={} {} | {:?}",
                    if same { "identical" } else { "DIFFERS" },
                    header_fields(&u),
                    w.stats
                );
            }
        }
        "graft" => {
            let key = a.get(6).map_or("m_attachments", String::as_str);
            let tbytes = std::fs::read(&a[2]).unwrap();
            let dbytes = std::fs::read(&a[3]).unwrap();
            let didx: usize = a[4].parse().unwrap();
            let dres = Resource::parse(&dbytes).unwrap();
            let donor_raw = dres.get_block_by_index(didx).unwrap();
            let mut cur = tbytes.clone();
            let idxs = mdat_blocks(&Resource::parse(&tbytes).unwrap());
            for i in idxs {
                let res = Resource::parse(&cur).unwrap();
                let raw = res.get_block_by_index(i).unwrap();
                let bytes = graft_block(raw, donor_raw, key);
                println!("block {i}: {} -> {} bytes, {}", raw.len(), bytes.len(), header_fields(&bytes));
                cur = res.rebuild_with_block(i, &bytes).unwrap();
            }
            std::fs::write(&a[5], &cur).unwrap();
            println!("wrote {} ({} bytes)", a[5], cur.len());
        }
        "graft-data" => {
            let tbytes = std::fs::read(&a[2]).unwrap();
            let dbytes = std::fs::read(&a[3]).unwrap();
            let key = &a[5];
            let res = Resource::parse(&tbytes).unwrap();
            let dres = Resource::parse(&dbytes).unwrap();
            let raw = res.data_block().unwrap();
            let bytes = graft_block(raw, dres.data_block().unwrap(), key);
            println!("DATA: {} -> {} bytes, {}", raw.len(), bytes.len(), header_fields(&bytes));
            std::fs::write(&a[4], res.rebuild_with_data(&bytes).unwrap()).unwrap();
        }
        "meshgroup-masks" => {
            // meshgroup-masks <in> <out> <default_mask> <old_bit=new_mask>...
            // Rewrites every m_refMeshGroupMasks entry bit by bit through the map
            // (unmapped bits are dropped) and sets m_nDefaultMeshGroupMask.
            let bytes = std::fs::read(&a[2]).unwrap();
            let default_mask: u64 = a[4].parse().unwrap();
            let map: Vec<(u32, u64)> = a[5..]
                .iter()
                .map(|m| {
                    let (b, v) = m.split_once('=').unwrap();
                    (b.parse().unwrap(), v.parse().unwrap())
                })
                .collect();
            let res = Resource::parse(&bytes).unwrap();
            let raw = res.data_block().unwrap();
            let mut doc = parse(raw);

            let w = Walker::new(&doc, &["m_refMeshGroupMasks"]).run();
            let (s, e, t) = w.cap.expect("m_refMeshGroupMasks missing");
            assert_eq!(t, AAUX, "expected an aux-buffer array");
            let sub = doc.types[s.types] & 0x3F;
            let (k, width) = match sub {
                UINT64 | INT64 => (B8, 8),
                UINT32 | INT32 => (B4, 4),
                other => panic!("unsupported mask subtype {other}"),
            };
            let lane = &mut doc.aux[k];
            let mut olds = Vec::new();
            for at in (s.aux[k]..e.aux[k]).step_by(width) {
                let old = if width == 8 {
                    u64::from_le_bytes(lane[at..at + 8].try_into().unwrap())
                } else {
                    u64::from(u32_at(lane, at))
                };
                let new = map
                    .iter()
                    .filter(|(bit, _)| old & (1 << bit) != 0)
                    .fold(0u64, |acc, (_, v)| acc | v);
                if width == 8 {
                    lane[at..at + 8].copy_from_slice(&new.to_le_bytes());
                } else {
                    lane[at..at + 4].copy_from_slice(&(new as u32).to_le_bytes());
                }
                olds.push((old, new));
            }
            println!("masks: {olds:?}");

            let w = Walker::new(&doc, &["m_nDefaultMeshGroupMask"]).run();
            let (s, _, t) = w.cap.expect("m_nDefaultMeshGroupMask missing");
            assert_eq!(t, UINT32, "expected a UINT32 default mask");
            let at = s.main[B4];
            println!("default mask: {} -> {default_mask}", u32_at(&doc.main[B4], at));
            doc.main[B4][at..at + 4].copy_from_slice(&(default_mask as u32).to_le_bytes());

            let out = serialize(&doc);
            assert_eq!(out.len(), morphic::kv3::rewrap_uncompressed(raw).unwrap().len());
            std::fs::write(&a[3], res.rebuild_with_data(&out).unwrap()).unwrap();
        }
        "retally" => {
            // retally <orig.vmdl_c> <in.vmdl_c> <out.vmdl_c>: for the DATA block and
            // every MDAT, if the ORIGINAL block's header matches the tally formulas
            // exactly, rewrite the edited block's header from a full walk. Clears
            // tallies left stale by other morphic edits (object/array inserts).
            let orig = std::fs::read(&a[2]).unwrap();
            let mut cur = std::fs::read(&a[3]).unwrap();
            let ores = Resource::parse(&orig).unwrap();
            let kinds: Vec<[u8; 4]> = ores.blocks().iter().map(|b| b.kind).collect();
            let nblocks = Resource::parse(&cur).unwrap().blocks().len();
            assert_eq!(kinds.len(), nblocks, "block layout changed");
            for (i, kind) in kinds.iter().enumerate() {
                if kind != b"MDAT" && kind != b"DATA" {
                    continue;
                }
                let o = parse(ores.get_block_by_index(i).unwrap());
                let ou = morphic::kv3::rewrap_uncompressed(ores.get_block_by_index(i).unwrap()).unwrap();
                if fit(&Walker::new(&o, &[]).run().stats) != header_fields(&ou) {
                    println!("block {i}: original header does not fit, left as is");
                    continue;
                }
                let res = Resource::parse(&cur).unwrap();
                let n = parse(res.get_block_by_index(i).unwrap());
                let st = Walker::new(&n, &[]).run().stats;
                let mut bytes = serialize(&n);
                bytes[44..46].copy_from_slice(&(st.objects as u16).to_le_bytes());
                bytes[46..48].copy_from_slice(&(st.arrays.iter().sum::<usize>() as u16).to_le_bytes());
                put_i32(&mut bytes, 104, st.members + 1);
                put_i32(&mut bytes, 112, st.arrays[0] + st.arrays[1] + st.arrays[2]);
                put_i32(&mut bytes, 116, st.elems[0] + st.elems[1] + st.elems[2] + st.arrays[0]);
                assert_eq!(fit(&st), header_fields(&bytes));
                let before = header_fields(&morphic::kv3::rewrap_uncompressed(res.get_block_by_index(i).unwrap()).unwrap());
                let after = header_fields(&bytes);
                if before != after {
                    println!("block {i}: {before} -> {after}");
                }
                cur = res.rebuild_with_block(i, &bytes).unwrap();
            }
            std::fs::write(&a[4], &cur).unwrap();
        }
        "set-number" => {
            // set-number <in> <out> <FOURCC> <path/with/#idx> <value>: overwrite one
            // FLOAT/DOUBLE/INT32/UINT32 in place (width unchanged) in every block of
            // that kind that has the path.
            let mut cur = std::fs::read(&a[2]).unwrap();
            let path: Vec<&str> = a[5].split('/').collect();
            let value: f64 = a[6].parse().unwrap();
            let idxs: Vec<usize> = Resource::parse(&cur)
                .unwrap()
                .blocks()
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == a[4].as_bytes())
                .map(|(i, _)| i)
                .collect();
            let mut hits = 0;
            for i in idxs {
                let res = Resource::parse(&cur).unwrap();
                let mut doc = parse(res.get_block_by_index(i).unwrap());
                let (cap, swapped) = {
                    let w = Walker::new(&doc, &path).run();
                    (w.cap, w.cap_swapped)
                };
                let Some((s, _, t)) = cap else { continue };
                let (lanes, pos) = if swapped { (&mut doc.aux, s.aux) } else { (&mut doc.main, s.main) };
                let old = match t {
                    FLOAT => {
                        let at = pos[B4];
                        let old = f64::from(f32::from_le_bytes(lanes[B4][at..at + 4].try_into().unwrap()));
                        lanes[B4][at..at + 4].copy_from_slice(&(value as f32).to_le_bytes());
                        old
                    }
                    DOUBLE => {
                        let at = pos[B8];
                        let old = f64::from_le_bytes(lanes[B8][at..at + 8].try_into().unwrap());
                        lanes[B8][at..at + 8].copy_from_slice(&value.to_le_bytes());
                        old
                    }
                    other => panic!("set-number: unsupported node type {other} at {}", a[5]),
                };
                println!("block {i}: {} {old} -> {value}", a[5]);
                let bytes = serialize(&doc);
                cur = res.rebuild_with_block(i, &bytes).unwrap();
                hits += 1;
            }
            assert!(hits > 0, "path not found in any {} block", a[4]);
            std::fs::write(&a[3], &cur).unwrap();
        }
        _ => eprintln!("usage: mdat_transplant stats|selftest|graft|graft-data|meshgroup-masks|retally|set-number ..."),
    }
}

/// Transplant `key`'s subtree from `donor_raw` into `target_raw` (both KV3 v5
/// blocks) and fix the header tallies. Returns the new uncompressed block.
fn graft_block(target_raw: &[u8], donor_raw: &[u8], key: &str) -> Vec<u8> {
    let target = parse(target_raw);
    let donor = parse(donor_raw);
    let new_doc = transplant(&target, &donor, key);
    let mut bytes = serialize(&new_doc);
    // Header object/array tallies (@44/@46 u16, @104/@112/@116): shift by the
    // donor-minus-target subtree difference, counted by a full walk.
    let before = Walker::new(&target, &[]).run().stats;
    let after = Walker::new(&new_doc, &[]).run().stats;
    let adj16 = |b: &mut Vec<u8>, o: usize, delta: isize| {
        let v = u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as isize + delta;
        b[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes());
    };
    let adj32 = |b: &mut Vec<u8>, o: usize, delta: isize| {
        let v = i32_at(b, o) as isize + delta;
        put_i32(b, o, v as usize);
    };
    // Fitted on stock + Midnight MDATs (exact on every block): @44 u16 objects,
    // @46 u16 all arrays, @104 members+1, @112 non-aux arrays, @116 main-buffer
    // array elements (+ untyped array count; untyped arrays there were all empty,
    // so that term is ambiguous and must not change here).
    let d = |f: fn(&Stats) -> usize| f(&after) as isize - f(&before) as isize;
    assert_eq!(d(|s| s.arrays[0]), 0, "untyped array count changed");
    assert_eq!(d(|s| s.elems[0]), 0, "untyped array elements changed");
    adj16(&mut bytes, 44, d(|s| s.objects));
    adj16(&mut bytes, 46, d(|s| s.arrays.iter().sum()));
    adj32(&mut bytes, 104, d(|s| s.members));
    adj32(&mut bytes, 112, d(|s| s.arrays[0] + s.arrays[1] + s.arrays[2]));
    adj32(&mut bytes, 116, d(|s| s.elems[1] + s.elems[2]));
    let donor_stats = Walker::new(&donor, &[]).run().stats;
    let donor_u = morphic::kv3::rewrap_uncompressed(donor_raw).unwrap();
    let target_u = morphic::kv3::rewrap_uncompressed(target_raw).unwrap();
    if fit(&donor_stats) == header_fields(&donor_u) && fit(&before) == header_fields(&target_u) {
        assert_eq!(fit(&after), header_fields(&bytes), "header fit fails on output");
    } else {
        println!(
            "  note: header fit not exact on this block kind (donor {} vs {}, target {} vs {}); tallies shifted by delta",
            header_fields(&donor_u),
            fit(&donor_stats),
            header_fields(&target_u),
            fit(&before)
        );
    }
    // The transplanted subtree must decode exactly like the donor's.
    let dv = morphic::kv3::decode(donor_raw).unwrap();
    let nv = morphic::kv3::decode(&bytes).unwrap();
    assert_eq!(dv.get(key), nv.get(key), "transplanted subtree decodes differently");
    bytes
}

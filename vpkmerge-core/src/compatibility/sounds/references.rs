//! RERL layout reference: `ValveResourceFormat`'s `ResourceExtRefList`.
//! <https://github.com/ValveResourceFormat/ValveResourceFormat/blob/master/ValveResourceFormat/Resource/Blocks/ResourceExtRefList.cs>
use std::collections::BTreeSet;

use anyhow::{ensure, Context, Result};
use morphic::resource::Resource;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reference {
    id: u64,
    path: String,
}

fn read(bytes: &[u8]) -> Result<Vec<Reference>> {
    ensure!(bytes.len() >= 8, "Truncated model resource references");
    let start = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
    let count = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
    ensure!(count <= 65_536, "Too many model resource references");
    if count == 0 {
        return Ok(Vec::new());
    }
    let end = start
        .checked_add(count * 16)
        .context("Resource reference extent overflow")?;
    ensure!(
        start >= 8 && end <= bytes.len(),
        "Truncated model resource reference entries"
    );
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut references = Vec::new();
    for at in (start..end).step_by(16) {
        let id = u64::from_le_bytes(bytes[at..at + 8].try_into()?);
        let relative = i32::from_le_bytes(bytes[at + 8..at + 12].try_into()?);
        ensure!(
            bytes[at + 12..at + 16] == [0; 4],
            "Unknown resource reference padding"
        );
        let name = (at + 8)
            .checked_add_signed(relative as isize)
            .context("Resource reference string offset overflow")?;
        ensure!(
            name >= end && name < bytes.len(),
            "Invalid resource reference string offset"
        );
        let tail = &bytes[name..];
        let len = tail
            .iter()
            .position(|byte| *byte == 0)
            .context("Unterminated resource reference")?;
        let path = std::str::from_utf8(&tail[..len])?.to_owned();
        ensure!(
            !path.is_empty() && ids.insert(id) && paths.insert(path.to_ascii_lowercase()),
            "Ambiguous model resource reference"
        );
        references.push(Reference { id, path });
    }
    Ok(references)
}

fn write(references: &[Reference]) -> Result<Vec<u8>> {
    let mut bytes = vec![0; 8 + references.len() * 16];
    bytes[..4].copy_from_slice(&8u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&u32::try_from(references.len())?.to_le_bytes());
    for (index, reference) in references.iter().enumerate() {
        let at = 8 + index * 16;
        bytes[at..at + 8].copy_from_slice(&reference.id.to_le_bytes());
        let relative = i32::try_from(bytes.len() - (at + 8))?;
        bytes[at + 8..at + 12].copy_from_slice(&relative.to_le_bytes());
        bytes.extend(reference.path.as_bytes());
        bytes.push(0);
    }
    ensure!(
        read(&bytes)? == references,
        "Resource reference round-trip changed entries"
    );
    Ok(bytes)
}

fn block(resource: &Resource<'_>) -> Result<usize> {
    let mut matches = resource
        .blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| b.kind == *b"RERL");
    let (index, _) = matches
        .next()
        .context("Model has no resource reference block")?;
    ensure!(
        matches.next().is_none(),
        "Ambiguous model resource reference blocks"
    );
    Ok(index)
}

fn extend(references: &mut Vec<Reference>, donors: &[Reference], paths: &[&str]) -> Result<()> {
    for path in paths {
        let donor = donors
            .iter()
            .find(|r| r.path == *path)
            .context("Stock sound table has no model resource reference")?;
        if let Some(existing) = references
            .iter()
            .find(|r| r.path.eq_ignore_ascii_case(path) || r.id == donor.id)
        {
            ensure!(
                existing == donor,
                "Authored sound resource reference conflicts with the installed model"
            );
        } else {
            references.push(donor.clone());
        }
    }
    Ok(())
}

pub(super) fn restore(source: &[u8], base: &[u8], paths: &[&str]) -> Result<Vec<u8>> {
    let authored = Resource::parse(source)?;
    let stock = Resource::parse(base)?;
    let index = block(&authored)?;
    let mut references = read(
        authored
            .get_block_by_index(index)
            .context("Missing model references")?,
    )?;
    let original_count = references.len();
    let donors = read(
        stock
            .get_block_by_index(block(&stock)?)
            .context("Missing stock references")?,
    )?;
    extend(&mut references, &donors, paths)?;
    if references.len() == original_count {
        return Ok(source.to_vec());
    }
    Ok(authored.rebuild_with_block(index, &write(&references)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_roundtrip_rejects_truncation_bad_offsets_and_duplicates() -> Result<()> {
        let refs = vec![
            Reference {
                id: 5,
                path: "models/custom.vmdl".into(),
            },
            Reference {
                id: 7,
                path: "scripts/tagged_sounds/hero.vdata".into(),
            },
        ];
        let bytes = write(&refs)?;
        assert_eq!(read(&bytes)?, refs);
        for end in [0, 7, 15, 39, bytes.len() - 1] {
            assert!(read(&bytes[..end]).is_err());
        }
        let mut malformed = bytes.clone();
        malformed[16..20].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(read(&malformed).is_err());
        assert!(write(&[refs[0].clone(), refs[0].clone()]).is_err());
        Ok(())
    }

    #[test]
    fn restoring_references_retains_authored_entries_and_declines_id_or_path_conflicts(
    ) -> Result<()> {
        let authored = Reference {
            id: 3,
            path: "materials/custom.vmat".into(),
        };
        let sound = Reference {
            id: 7,
            path: "scripts/tagged_sounds/hero.vdata".into(),
        };
        let mut refs = vec![authored.clone()];
        extend(&mut refs, std::slice::from_ref(&sound), &[&sound.path])?;
        assert_eq!(refs, vec![authored, sound.clone()]);
        extend(&mut refs, std::slice::from_ref(&sound), &[&sound.path])?;
        assert_eq!(refs.len(), 2);
        for conflicting in [
            Reference {
                id: 9,
                path: sound.path.clone(),
            },
            Reference {
                id: sound.id,
                path: "materials/other.vmat".into(),
            },
        ] {
            assert!(extend(
                &mut vec![conflicting],
                std::slice::from_ref(&sound),
                &[&sound.path]
            )
            .is_err());
        }
        assert!(extend(&mut Vec::new(), &[], &[&sound.path]).is_err());
        Ok(())
    }
}

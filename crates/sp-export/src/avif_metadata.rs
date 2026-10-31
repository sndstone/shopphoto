//! Add an ICC property and a standard XMP MIME item to the AVIF muxer's output.
//! Rebuild absolute item offsets after changing the metadata box.
use std::io::{Error, ErrorKind, Result};

#[derive(Clone)]
struct BoxData {
    kind: [u8; 4],
    payload: Vec<u8>,
}
fn invalid() -> Error {
    Error::new(ErrorKind::InvalidData, "unexpected AVIF metadata structure")
}
fn be16(bytes: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        bytes
            .get(at..at + 2)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}
fn be32(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(at..at + 4)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}
fn boxes(mut bytes: &[u8]) -> Result<Vec<BoxData>> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let size = be32(bytes, 0)? as usize;
        if size < 8 || size > bytes.len() {
            return Err(invalid());
        }
        out.push(BoxData {
            kind: bytes[4..8].try_into().map_err(|_| invalid())?,
            payload: bytes[8..size].to_vec(),
        });
        bytes = &bytes[size..];
    }
    Ok(out)
}
fn encoded(kind: [u8; 4], payload: &[u8]) -> Result<Vec<u8>> {
    let length = payload
        .len()
        .checked_add(8)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(invalid)?;
    let mut out = length.to_be_bytes().to_vec();
    out.extend_from_slice(&kind);
    out.extend_from_slice(payload);
    Ok(out)
}
fn join(items: &[BoxData]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for item in items {
        out.extend(encoded(item.kind, &item.payload)?);
    }
    Ok(out)
}
fn item_mut<'a>(items: &'a mut [BoxData], kind: &[u8; 4]) -> Result<&'a mut BoxData> {
    items
        .iter_mut()
        .find(|item| &item.kind == kind)
        .ok_or_else(invalid)
}
fn locations(items: &[(u16, Vec<u8>)], first_offset: u32) -> Result<Vec<u8>> {
    let mut out = vec![0, 0, 0, 0, 0x44, 0];
    out.extend_from_slice(
        &u16::try_from(items.len())
            .map_err(|_| invalid())?
            .to_be_bytes(),
    );
    let mut offset = first_offset;
    for (id, bytes) in items {
        let length = u32::try_from(bytes.len()).map_err(|_| invalid())?;
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
        offset = offset.checked_add(length).ok_or_else(invalid)?;
    }
    Ok(out)
}

pub(crate) fn attach(bytes: &[u8], icc: Option<&[u8]>, xmp: Option<&str>) -> Result<Vec<u8>> {
    if icc.is_none() && xmp.is_none() {
        return Ok(bytes.to_vec());
    }
    let mut top = boxes(bytes)?;
    let meta = item_mut(&mut top, b"meta")?;
    if meta.payload.get(..4) != Some(&[0, 0, 0, 0]) {
        return Err(invalid());
    }
    let mut children = boxes(&meta.payload[4..])?;
    let pitm = item_mut(&mut children, b"pitm")?;
    if pitm.payload.get(..4) != Some(&[0, 0, 0, 0]) {
        return Err(invalid());
    }
    let primary_id = be16(&pitm.payload, 4)?;
    let iloc = item_mut(&mut children, b"iloc")?;
    if iloc.payload.get(..6) != Some(&[0, 0, 0, 0, 0x44, 0]) {
        return Err(invalid());
    }
    let mut items = Vec::new();
    let mut cursor = 8;
    for _ in 0..be16(&iloc.payload, 6)? {
        let id = be16(&iloc.payload, cursor)?;
        if be16(&iloc.payload, cursor + 2)? != 0 {
            return Err(invalid());
        }
        let count = be16(&iloc.payload, cursor + 4)?;
        cursor += 6;
        let mut data = Vec::new();
        for _ in 0..count {
            let offset = be32(&iloc.payload, cursor)? as usize;
            let length = be32(&iloc.payload, cursor + 4)? as usize;
            let end = offset.checked_add(length).ok_or_else(invalid)?;
            data.extend_from_slice(bytes.get(offset..end).ok_or_else(invalid)?);
            cursor += 8;
        }
        items.push((id, data));
    }
    if cursor != iloc.payload.len() {
        return Err(invalid());
    }
    if let Some(icc) = icc {
        let iprp = item_mut(&mut children, b"iprp")?;
        let mut properties = boxes(&iprp.payload)?;
        let ipco = item_mut(&mut properties, b"ipco")?;
        let index = u8::try_from(boxes(&ipco.payload)?.len() + 1).map_err(|_| invalid())?;
        if index >= 128 {
            return Err(invalid());
        }
        let mut profile = b"prof".to_vec();
        profile.extend_from_slice(icc);
        ipco.payload.extend(encoded(*b"colr", &profile)?);
        let ipma = item_mut(&mut properties, b"ipma")?;
        if ipma.payload.get(..4) != Some(&[0, 0, 0, 0]) {
            return Err(invalid());
        }
        let mut updated = ipma.payload.get(..8).ok_or_else(invalid)?.to_vec();
        let mut cursor = 8;
        let mut attached = false;
        for _ in 0..be32(&ipma.payload, 4)? {
            let id = be16(&ipma.payload, cursor)?;
            let count = *ipma.payload.get(cursor + 2).ok_or_else(invalid)?;
            updated.extend_from_slice(&id.to_be_bytes());
            updated.push(if id == primary_id {
                count.checked_add(1).ok_or_else(invalid)?
            } else {
                count
            });
            updated.extend_from_slice(
                ipma.payload
                    .get(cursor + 3..cursor + 3 + count as usize)
                    .ok_or_else(invalid)?,
            );
            if id == primary_id {
                updated.push(index);
                attached = true;
            }
            cursor += 3 + count as usize;
        }
        if !attached || cursor != ipma.payload.len() {
            return Err(invalid());
        }
        ipma.payload = updated;
        iprp.payload = join(&properties)?;
    }
    if let Some(xmp) = xmp {
        let id = items
            .iter()
            .map(|(id, _)| *id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(invalid)?;
        let iinf = item_mut(&mut children, b"iinf")?;
        if iinf.payload.get(..4) != Some(&[0, 0, 0, 0]) {
            return Err(invalid());
        }
        let count = be16(&iinf.payload, 4)?.checked_add(1).ok_or_else(invalid)?;
        iinf.payload[4..6].copy_from_slice(&count.to_be_bytes());
        let mut entry = vec![2, 0, 0, 0];
        entry.extend_from_slice(&id.to_be_bytes());
        entry.extend_from_slice(b"\0\0mimeXMP\0application/rdf+xml\0\0");
        iinf.payload.extend(encoded(*b"infe", &entry)?);
        if !children.iter().any(|item| item.kind == *b"iref") {
            children.push(BoxData {
                kind: *b"iref",
                payload: vec![0; 4],
            });
        }
        let iref = item_mut(&mut children, b"iref")?;
        if iref.payload.get(..4) != Some(&[0, 0, 0, 0]) {
            return Err(invalid());
        }
        let mut reference = id.to_be_bytes().to_vec();
        reference.extend_from_slice(&1u16.to_be_bytes());
        reference.extend_from_slice(&primary_id.to_be_bytes());
        iref.payload.extend(encoded(*b"cdsc", &reference)?);
        items.push((id, xmp.as_bytes().to_vec()));
    }
    item_mut(&mut children, b"iloc")?.payload = locations(&items, 0)?;
    let mut meta_payload = vec![0; 4];
    meta_payload.extend(join(&children)?);
    item_mut(&mut top, b"meta")?.payload = meta_payload;
    let mdat_index = top
        .iter()
        .position(|item| item.kind == *b"mdat")
        .ok_or_else(invalid)?;
    let offset = u32::try_from(join(&top[..mdat_index])?.len() + 8).map_err(|_| invalid())?;
    item_mut(&mut children, b"iloc")?.payload = locations(&items, offset)?;
    let mut meta_payload = vec![0; 4];
    meta_payload.extend(join(&children)?);
    item_mut(&mut top, b"meta")?.payload = meta_payload;
    top[mdat_index].payload = items.into_iter().flat_map(|(_, data)| data).collect();
    join(&top)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_boxes_are_rejected_without_panics() {
        for bytes in [
            vec![],
            vec![0; 20],
            b"\0\0\0\x01meta".to_vec(),
            b"\xff\xff\xff\xffmeta".to_vec(),
        ] {
            assert!(attach(&bytes, Some(b"icc"), None).is_err());
        }
    }
}

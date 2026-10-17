use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use symphonia::core::io::MediaSource;

use super::http_source::HttpSource;

const BOX_HEADER: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fragment {
    pub start_secs: f64,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct FragmentIndex {
    header: Arc<[u8]>,
    fragments: Vec<Fragment>,
}

impl FragmentIndex {
    pub fn parse(prefix: &[u8]) -> Option<Self> {
        let mut pos = 0;
        let mut saw_moov = false;
        while pos + BOX_HEADER <= prefix.len() {
            let size = usize::try_from(read_u32(prefix, pos)?).ok()?;
            let kind = prefix.get(pos + 4..pos + 8)?;
            if size < BOX_HEADER || pos + size > prefix.len() {
                return None;
            }
            match kind {
                b"moov" => saw_moov = true,
                b"sidx" if saw_moov => {
                    let fragments = parse_sidx(&prefix[pos..pos + size], (pos + size) as u64)?;
                    let header = Arc::from(&prefix[..pos + size]);
                    return Some(Self { header, fragments });
                }
                _ => {}
            }
            pos += size;
        }
        None
    }

    pub fn locate(&self, target_secs: f64) -> Fragment {
        let index = self
            .fragments
            .partition_point(|fragment| fragment.start_secs <= target_secs);
        self.fragments[index.saturating_sub(1)]
    }

    pub fn splice(&self, source: HttpSource, fragment: Fragment) -> SplicedSource {
        let total = self.header.len() as u64 + source.len().saturating_sub(fragment.offset);
        SplicedSource {
            header: self.header.clone(),
            inner: source,
            base: fragment.offset,
            pos: 0,
            len: total,
        }
    }
}

fn parse_sidx(sidx: &[u8], sidx_end: u64) -> Option<Vec<Fragment>> {
    let version = *sidx.get(8)?;
    let timescale = f64::from(read_u32(sidx, 16)?);
    let (earliest, first_offset, mut pos) = if version == 0 {
        (
            u64::from(read_u32(sidx, 20)?),
            u64::from(read_u32(sidx, 24)?),
            28,
        )
    } else {
        (read_u64(sidx, 20)?, read_u64(sidx, 28)?, 36)
    };
    let count = usize::from(u16::from_be_bytes(
        sidx.get(pos + 2..pos + 4)?.try_into().ok()?,
    ));
    pos += 4;
    if timescale == 0.0 || count == 0 {
        return None;
    }
    let mut fragments = Vec::with_capacity(count);
    let (mut offset, mut time) = (sidx_end + first_offset, earliest);
    for _ in 0..count {
        let reference = read_u32(sidx, pos)?;
        if reference & 0x8000_0000 != 0 {
            return None;
        }
        fragments.push(Fragment {
            start_secs: time as f64 / timescale,
            offset,
        });
        offset += u64::from(reference & 0x7FFF_FFFF);
        time += u64::from(read_u32(sidx, pos + 4)?);
        pos += 12;
    }
    Some(fragments)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

pub struct SplicedSource {
    header: Arc<[u8]>,
    inner: HttpSource,
    base: u64,
    pos: u64,
    len: u64,
}

impl Read for SplicedSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let header_len = self.header.len() as u64;
        if self.pos < header_len {
            let rest = &self.header[self.pos as usize..];
            let count = rest.len().min(buf.len());
            buf[..count].copy_from_slice(&rest[..count]);
            self.pos += count as u64;
            return Ok(count);
        }
        self.inner
            .seek(SeekFrom::Start(self.base + self.pos - header_len))?;
        let count = self.inner.read(buf)?;
        self.pos += count as u64;
        Ok(count)
    }
}

impl Seek for SplicedSource {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let new_pos = match target {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        self.pos = new_pos;
        Ok(new_pos)
    }
}

impl MediaSource for SplicedSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + BOX_HEADER) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn sidx_v0(
        timescale: u32,
        first_offset: u32,
        refs: &[(u32, u32)],
        hierarchical: bool,
    ) -> Vec<u8> {
        let mut body = vec![0, 0, 0, 0];
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&first_offset.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&(refs.len() as u16).to_be_bytes());
        for &(size, duration) in refs {
            let flag = if hierarchical { 0x8000_0000 } else { 0 };
            body.extend_from_slice(&(size | flag).to_be_bytes());
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&0x9000_0000u32.to_be_bytes());
        }
        boxed(b"sidx", &body)
    }

    fn file(sidx: Vec<u8>) -> Vec<u8> {
        let mut bytes = boxed(b"ftyp", b"dash0000");
        bytes.extend(boxed(b"moov", &[0; 20]));
        bytes.extend(sidx);
        bytes.extend(boxed(b"moof", &[0; 4]));
        bytes
    }

    #[test]
    fn maps_time_to_fragment_offsets() {
        let bytes = file(sidx_v0(
            1000,
            0,
            &[(500, 10_000), (700, 10_000), (900, 5_000)],
            false,
        ));
        let index = FragmentIndex::parse(&bytes).unwrap();
        let header_len = index.header.len() as u64;
        assert_eq!(&index.header[4..8], b"ftyp");
        assert_eq!(
            index.fragments,
            [
                Fragment {
                    start_secs: 0.0,
                    offset: header_len
                },
                Fragment {
                    start_secs: 10.0,
                    offset: header_len + 500
                },
                Fragment {
                    start_secs: 20.0,
                    offset: header_len + 1200
                },
            ]
        );
        assert_eq!(index.locate(0.0).start_secs, 0.0);
        assert_eq!(index.locate(15.0).start_secs, 10.0);
        assert_eq!(index.locate(99.0).start_secs, 20.0);
    }

    #[test]
    fn rejects_unsupported_or_truncated_indexes() {
        assert!(FragmentIndex::parse(&file(sidx_v0(1000, 0, &[(500, 10)], true))).is_none());
        assert!(FragmentIndex::parse(&file(sidx_v0(0, 0, &[(500, 10)], false))).is_none());
        let full = file(sidx_v0(1000, 0, &[(500, 10)], false));
        assert!(FragmentIndex::parse(&full[..full.len() - 20]).is_none());
        assert!(FragmentIndex::parse(&boxed(b"ftyp", b"dash0000")).is_none());
        assert!(FragmentIndex::parse(&[]).is_none());
    }
}

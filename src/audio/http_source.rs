use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_RANGE, HeaderMap, HeaderName, HeaderValue, RANGE};

use super::resolve::StreamInfo;

const FIRST_CHUNK_BYTES: u64 = 256 * 1024;
const CHUNK_BYTES: u64 = 1024 * 1024;
const ATTEMPTS: usize = 2;

pub type Refresh = Arc<dyn Fn() -> Option<StreamInfo> + Send + Sync>;

pub struct HttpSource {
    client: Client,
    url: String,
    headers: HeaderMap,
    len: u64,
    pos: u64,
    chunk_start: u64,
    chunk: Vec<u8>,
    refresh: Option<Refresh>,
}

impl HttpSource {
    pub fn open(stream: &StreamInfo, refresh: Option<Refresh>) -> io::Result<Self> {
        let client = Client::builder().build().map_err(io::Error::other)?;
        let mut source = Self {
            client,
            url: stream.url.clone(),
            headers: header_map(&stream.headers)?,
            len: 0,
            pos: 0,
            chunk_start: 0,
            chunk: Vec::with_capacity(CHUNK_BYTES as usize),
            refresh,
        };
        source.len = source.fetch(0, FIRST_CHUNK_BYTES)?;
        Ok(source)
    }

    pub fn reopen(&self) -> Self {
        Self {
            client: self.client.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            len: self.len,
            pos: 0,
            chunk_start: 0,
            chunk: Vec::new(),
            refresh: self.refresh.clone(),
        }
    }

    pub fn prefix(&self) -> &[u8] {
        if self.chunk_start == 0 {
            &self.chunk
        } else {
            &[]
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    fn fetch(&mut self, start: u64, size: u64) -> io::Result<u64> {
        let mut last_error = None;
        for _ in 0..ATTEMPTS {
            match self.try_fetch(start, size) {
                Ok(total) => return Ok(total),
                Err(err) => {
                    if err.kind() == io::ErrorKind::PermissionDenied {
                        self.refresh_url();
                    }
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("no fetch attempted")))
    }

    fn refresh_url(&mut self) {
        let Some(stream) = self.refresh.as_ref().and_then(|refresh| refresh()) else {
            return;
        };
        if let Ok(headers) = header_map(&stream.headers) {
            self.url = stream.url;
            self.headers = headers;
        }
    }

    fn try_fetch(&mut self, start: u64, size: u64) -> io::Result<u64> {
        let end = start + size - 1;
        let response = self
            .client
            .get(&self.url)
            .headers(self.headers.clone())
            .header(RANGE, format!("bytes={start}-{end}"))
            .send()
            .map_err(io::Error::other)?;
        if response.status() == StatusCode::FORBIDDEN {
            // Stream URLs expire after a few hours; `fetch` refreshes and retries.
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "stream URL expired",
            ));
        }
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(io::Error::other(format!(
                "stream returned {} for a range request",
                response.status()
            )));
        }
        let total = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(total_length)
            .ok_or_else(|| io::Error::other("stream response has no Content-Range"))?;
        self.chunk.clear();
        response.take(size).read_to_end(&mut self.chunk)?;
        self.chunk_start = start;
        Ok(total)
    }

    fn buffered(&self) -> Option<&[u8]> {
        let offset = usize::try_from(self.pos.checked_sub(self.chunk_start)?).ok()?;
        self.chunk.get(offset..).filter(|rest| !rest.is_empty())
    }
}

impl Read for HttpSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        if self.buffered().is_none() {
            let size = if self.chunk.is_empty() {
                FIRST_CHUNK_BYTES
            } else {
                CHUNK_BYTES
            };
            self.fetch(self.pos, size)?;
        }
        let available = self.buffered().unwrap_or_default();
        let count = available.len().min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        self.pos += count as u64;
        Ok(count)
    }
}

impl Seek for HttpSource {
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

fn header_map(headers: &std::collections::HashMap<String, String>) -> io::Result<HeaderMap> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(io::Error::other)?;
            let value = HeaderValue::from_str(value).map_err(io::Error::other)?;
            Ok((name, value))
        })
        .collect()
}

fn total_length(content_range: &str) -> Option<u64> {
    content_range.rsplit_once('/')?.1.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_total_from_content_range() {
        assert_eq!(total_length("bytes 0-262143/3046886"), Some(3_046_886));
        assert_eq!(total_length("bytes 0-10/*"), None);
        assert_eq!(total_length("garbage"), None);
    }
}

//! Bounded version 1 frames shared by both sides of the session socket.

use super::{Joined, PermitDenial, PermitGranted};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};

const VERSION: u16 = 1;
pub(super) const MAX_FRAME_BYTES: usize = 4096;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Frame<T> {
    version: u16,
    body: T,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Join { token: String },
    BorrowPermit { token: String },
    Release { member_id: u64 },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Response {
    Joined(Joined),
    PermitGranted(PermitGranted),
    PermitDenied { reason: PermitDenial },
    Released {},
}

pub(super) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn read<T: DeserializeOwned>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_FRAME_BYTES as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.pop() != Some(b'\n') {
        return Err(invalid("incomplete or oversized session broker frame"));
    }
    let frame: Frame<T> =
        serde_json::from_slice(&bytes).map_err(|_| invalid("malformed session broker frame"))?;
    if frame.version != VERSION {
        return Err(invalid("unsupported session broker protocol version"));
    }
    Ok(Some(frame.body))
}

pub(super) fn write<T: Serialize>(writer: &mut impl Write, body: T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&Frame {
        version: VERSION,
        body,
    })
    .map_err(io::Error::other)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(invalid("oversized session broker frame"));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes)
}

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[cfg(feature = "runtime")]
mod runtime;
#[cfg(feature = "runtime")]
pub use runtime::serve;

const MAX_FRAME_PART: usize = 256 << 20;

#[derive(Debug, Serialize, Deserialize)]
pub struct Start {
    pub fuel: u64,
    pub memory_bytes: usize,
    pub input_len: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    ReadFile { name: String },
    WriteFile { name: String },
    Run,
    Done { error: Option<String> },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
}

fn write_part(to: &mut impl Write, part: &[u8]) -> io::Result<()> {
    let len = u32::try_from(part.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
    to.write_all(&len.to_le_bytes())?;
    to.write_all(part)
}

fn read_part(from: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0; 4];
    from.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME_PART {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut part = vec![0; len];
    from.read_exact(&mut part)?;
    Ok(part)
}

pub fn write_frame(to: &mut impl Write, message: &impl Serialize, bytes: &[u8]) -> io::Result<()> {
    write_part(to, &serde_json::to_vec(message)?)?;
    write_part(to, bytes)?;
    to.flush()
}

pub fn read_frame<T: DeserializeOwned>(from: &mut impl Read) -> io::Result<(T, Vec<u8>)> {
    let message = serde_json::from_slice(&read_part(from)?)?;
    Ok((message, read_part(from)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_comes_back_as_it_went_with_any_bytes() {
        let mut pipe = Vec::new();
        let request = Request::WriteFile {
            name: "a/б.txt".to_owned(),
        };
        write_frame(&mut pipe, &request, &[0, 255, 10]).unwrap_or_else(|e| panic!("{e}"));
        let mut from = pipe.as_slice();
        let (back, bytes): (Request, _) = read_frame(&mut from).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, request);
        assert_eq!(bytes, [0, 255, 10]);
        assert!(from.is_empty());
    }

    #[test]
    fn a_cut_or_absurd_frame_is_an_error_not_a_huge_allocation() {
        let mut pipe = Vec::new();
        write_frame(&mut pipe, &Reply { ok: true }, b"abc").unwrap_or_else(|e| panic!("{e}"));
        let cut = &pipe[..pipe.len() - 1];
        assert!(read_frame::<Reply>(&mut &*cut).is_err());
        let absurd = u32::MAX.to_le_bytes();
        assert!(read_frame::<Reply>(&mut absurd.as_slice()).is_err());
    }
}

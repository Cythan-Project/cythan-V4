use std::io::{Cursor, Error, Read, Write};

use varint::{VarintRead, VarintWrite};

pub struct HeaderData {
    pub header_version: u32,
    pub version: u32,                // The Cythan specification
    pub interupt_configuration: u32, // The interupt configuration
    pub base: u8,
    pub info_string: String,
}

impl Default for HeaderData {
    fn default() -> Self {
        HeaderData {
            header_version: 1,
            version: 4,
            interupt_configuration: 1,
            base: 4,
            info_string: String::new(),
        }
    }
}

fn encode_u32(encoder: &mut Cursor<Vec<u8>>, data: u32) -> Result<(), Error> {
    encoder.write_unsigned_varint_32(data)
}

fn encode_str(encoder: &mut Cursor<Vec<u8>>, data: &str) -> Result<(), Error> {
    encode_u32(encoder, data.len() as u32)?;
    encoder.write(data.as_bytes()).map(|_| ())
}

fn decode_str(encoder: &mut Cursor<Vec<u8>>) -> Result<String, Error> {
    let len = decode_u32(encoder)?;
    let mut buf = vec![0; len as usize];
    encoder.read(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn decode_u8(encoder: &mut Cursor<Vec<u8>>) -> Result<u8, Error> {
    let mut k = [0];
    encoder.read(&mut k)?;
    Ok(k[0])
}

fn decode_u32(encoder: &mut Cursor<Vec<u8>>) -> Result<u32, Error> {
    encoder.read_unsigned_varint_32()
}

pub fn decode_bytes(bytes: &[u8]) -> Result<(HeaderData, Vec<u32>), Error> {
    let mut encoded = Cursor::new(bytes.to_vec());
    let header = HeaderData {
        header_version: decode_u32(&mut encoded)?,
        version: decode_u32(&mut encoded)?,
        interupt_configuration: decode_u32(&mut encoded)?,
        base: decode_u8(&mut encoded)?,
        info_string: decode_str(&mut encoded)?,
    };

    let length = decode_u32(&mut encoded)?;
    Ok((
        header,
        (0..length)
            .map(|_| decode_u32(&mut encoded))
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

pub fn encode_to_bytes(header: HeaderData, cythan_memory: &[u32]) -> Result<Vec<u8>, Error> {
    let mut encoded = Cursor::new(Vec::new());
    encode_u32(&mut encoded, header.header_version)?;
    encode_u32(&mut encoded, header.version)?;
    encode_u32(&mut encoded, header.interupt_configuration)?;
    encoded.write(&[header.base])?;
    encode_str(&mut encoded, &header.info_string)?;
    encode_u32(&mut encoded, cythan_memory.len() as u32)?;
    for x in cythan_memory {
        encode_u32(&mut encoded, *x)?;
    }
    Ok(encoded.into_inner())
}

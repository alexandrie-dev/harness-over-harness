//! Canonical base64 and RFC 9562 random v4 UUIDs.
use base64::{Engine, engine::general_purpose::STANDARD};
hoh_brand::branded_string!(pub Uuid);

pub fn bytes_to_base64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

pub fn random_uuid() -> Result<Uuid, getrandom::Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut output = String::with_capacity(36);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, byte) in bytes.into_iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            output.push('-');
        }
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(output.into())
}

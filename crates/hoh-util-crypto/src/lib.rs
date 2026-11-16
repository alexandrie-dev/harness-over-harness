//! Canonical base64 and RFC 9562 random v4 UUIDs.
use base64::{Engine, engine::general_purpose::STANDARD};
hoh_brand::branded_string!(pub Uuid);

pub fn bytes_to_base64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

pub fn random_uuid() -> Uuid {
    let mut bytes = [0u8; 16];
    // The operating system's cryptographic random source must be available.
    getrandom::fill(&mut bytes).expect("OS random source is unavailable");
    uuid_from_bytes(bytes)
}

fn uuid_from_bytes(mut bytes: [u8; 16]) -> Uuid {
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
    output.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_from_bytes_pinning_version_and_variant_without_platform_uuid() {
        assert_eq!(
            uuid_from_bytes([0; 16]).as_str(),
            "00000000-0000-4000-8000-000000000000"
        );
        assert_eq!(
            uuid_from_bytes([255; 16]).as_str(),
            "ffffffff-ffff-4fff-bfff-ffffffffffff"
        );
        assert_eq!(
            uuid_from_bytes(std::array::from_fn(|index| index as u8)).as_str(),
            "00010203-0405-4607-8809-0a0b0c0d0e0f"
        );
    }
}

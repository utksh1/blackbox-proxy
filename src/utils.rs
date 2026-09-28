

pub fn random_id(len: usize) -> String {
    let mut hex = String::with_capacity(len * 2);
    for _ in 0..len {
        let byte: u8 = rand::random();
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

pub fn generate_uuid() -> String {
    let mut bytes = [0u8; 16];
    for b in bytes.iter_mut() {
        *b = rand::random();
    }
    // UUID v4 variant
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_id() {
        let id = random_id(16);
        assert_eq!(id.len(), 32);
    }

    #[test]
    fn generate_uuid_has_v4_shape_and_variant_bits() {
        let uuid = generate_uuid();
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(uuid.len(), 36);
        assert_eq!(&parts[2][..1], "4"); // version 4
        assert!("89ab".contains(&parts[3][..1])); // variant bits
    }

    #[test]
    fn random_ids_are_unique() {
        assert_ne!(random_id(16), random_id(16));
    }
}

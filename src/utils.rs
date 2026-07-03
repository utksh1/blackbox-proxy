use rand::Rng;

pub fn random_id(len: usize) -> String {
    let mut hex = String::with_capacity(len * 2);
    for _ in 0..len {
        let byte: u8 = rand::random();
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_id() {
        let id = random_id(16);
        assert_eq!(id.len(), 32);
    }
}

use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier},
    Argon2,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub exp: usize,
    pub iat: usize,
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("Argon2 hash error: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, password_hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(password_hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

pub fn create_jwt(user_id: &str, role: &str, secret: &str) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        role: role.to_string(),
        iat: now,
        exp: now + 7 * 24 * 3600, // 7 days expiration
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| anyhow::anyhow!("JWT encode error: {e}"))
}

pub fn verify_jwt(token: &str, secret: &str) -> anyhow::Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| anyhow::anyhow!("Invalid token: {e}"))?;

    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argon2_hash_and_verify() {
        let password = "SecretPassword123!";
        let hash = hash_password(password).expect("Hashing should succeed");
        assert!(verify_password(password, &hash));
        assert!(!verify_password("WrongPassword", &hash));
    }

    #[test]
    fn test_jwt_create_and_verify() {
        let secret = "super_secret_jwt_key_12345";
        let user_id = "test-user-id";
        let role = "admin";
        let token = create_jwt(user_id, role, secret).expect("Token creation should succeed");
        let claims = verify_jwt(&token, secret).expect("Token verification should succeed");
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.role, role);
    }
}

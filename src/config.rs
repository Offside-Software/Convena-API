use std::env;
use std::fs;
use std::io::Cursor;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub database_url: String,
    pub supabase_url: String,
    pub supabase_key: String,
}

fn clean_val(v: String) -> String {
    v.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn get_clean_env(key: &str) -> Option<String> {
    env::var(key).ok().map(clean_val).filter(|s| !s.is_empty())
}

fn load_env_file(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut normalized = String::with_capacity(content.len());

    for line in content.lines() {
        normalized.push_str(line);
        normalized.push('\n');
    }

    dotenvy::from_read(Cursor::new(normalized))
        .map_err(|error| error.to_string())
}

impl Config {
    pub fn load() -> Self {
        let candidates = [
            Path::new(".env"),
            Path::new(".env.local"),
        ];

        for path in candidates {
            if path.exists() {
                if let Err(error) = load_env_file(path) {
                    tracing::warn!("Failed to load environment file {}: {error}", path.display());
                }
            }
        }

        let database_url = Self::resolve_database_url();
        let port = get_clean_env("PORT").and_then(|p| p.parse().ok()).unwrap_or(8000);
        Self {
            port,
            database_url,
            supabase_url: get_clean_env("SUPABASE_URL").unwrap_or_default(),
            supabase_key: get_clean_env("SUPABASE_KEY").unwrap_or_default()
        }
    }

    fn resolve_database_url() -> String {
        if let Some(url) = get_clean_env("POSTGRESQL_DATABASE_URL").or_else(|| get_clean_env("DATABASE_URL")){
            // Keep Supabase Session Pooler (port 5432) for SQLx.
            // SQLx uses the PostgreSQL extended protocol for bound parameters,
            // and Supavisor transaction pooling (port 6543) can route Parse and
            // Bind messages to different backend sessions, returning another
            // query's rows (e.g. "Value Buffer exceeds 8 bytes" decode errors).
            if url.contains(".pooler.supabase.com:6543") {
                tracing::warn!("Supabase transaction pooler detected; switching from :6543 to Session pooler :5432 for SQLx compatibility");
                return url.replace(".pooler.supabase.com:6543", ".pooler.supabase.com:5432");
            }
            return url;
        }

        let user = get_clean_env("POSTGRESQL_USERNAME").unwrap_or_else(|| "postgres".to_string());
        let pwd = get_clean_env("POSTGRESQL_PASSWORD").unwrap_or_default();
        let host = get_clean_env("POSTGRESQL_HOST").unwrap_or_else(|| "localhost".to_string());
        let port = get_clean_env("POSTGRESQL_PORT").unwrap_or_else(|| "5432".to_string());
        let db = get_clean_env("POSTGRESQL_DATABASE").unwrap_or_else(|| "postgres".to_string());

        format!("postgres://{user}:{pwd}@{host}:{port}/{db}")
    }
}
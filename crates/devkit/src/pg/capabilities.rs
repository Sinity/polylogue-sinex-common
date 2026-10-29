use super::*;

impl Server {
    pub(super) fn verify(&self, options: &ClusterOptions) -> Result {
        // Schema identities use the development shell's PostgreSQL version.
        // A socket accepting connections does not establish that identity.
        let expected = tool_major()?;
        let actual = self.sql("postgres", "SELECT current_setting('server_version_num')")?;
        let actual = actual
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("invalid PostgreSQL server version: {actual:?}"))?
            / 10_000;
        if actual != expected {
            return Err(format!(
                "PostgreSQL major {actual} differs from development tools major {expected}"
            ));
        }
        if !options.required_extensions.is_empty() {
            let available = self.sql(
                "postgres",
                "SELECT coalesce(json_object_agg(name,default_version),'{}'::json) FROM pg_available_extensions",
            )?;
            let available: std::collections::BTreeMap<String, Option<String>> =
                serde_json::from_str(&available)
                    .map_err(|error| format!("PostgreSQL extension catalog: {error}"))?;
            for name in &options.required_extensions {
                if !available
                    .get(name)
                    .and_then(Option::as_deref)
                    .is_some_and(|version| !version.is_empty())
                {
                    return Err(format!(
                        "required PostgreSQL extension {name:?} is unavailable"
                    ));
                }
            }
        }
        if !options.preload.is_empty() {
            let loaded = self.sql("postgres", "SHOW shared_preload_libraries")?;
            for name in &options.preload {
                if !loaded
                    .trim()
                    .split(',')
                    .any(|entry| entry.trim().trim_matches('"') == name)
                {
                    return Err(format!(
                        "required PostgreSQL preload {name:?} is not loaded"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn tool_major() -> Result<u32> {
    let output = Command::new("postgres")
        .arg("--version")
        .output()
        .map_err(|error| format!("postgres --version: {error}"))?;
    if !output.status.success() {
        return Err(format!("postgres --version failed: {}", output.status));
    }
    let version = String::from_utf8_lossy(&output.stdout);
    version
        .split_whitespace()
        .find_map(|part| part.split('.').next()?.parse().ok())
        .ok_or_else(|| format!("invalid postgres --version output: {version:?}"))
}

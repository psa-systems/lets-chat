use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// Build a stable fingerprint string from the captured `(user_agent, ip)`
/// pair. The components are joined with `\0` so a UA whose tail happens to
/// match an IP prefix can't collide with a swapped pairing. Missing fields
/// are represented as the literal empty string so a row with no UA but a
/// known IP still hashes consistently.
fn fingerprint(user_agent: Option<&str>, ip: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_agent.unwrap_or("").as_bytes());
    hasher.update([0u8]);
    hasher.update(ip.unwrap_or("").as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// True if the device fingerprint for `(user_agent, ip)` has been seen
/// before for `user_id`. Inserts the row on the first call so subsequent
/// logins from the same device are recognized.
///
/// Returns `Ok(true)` only when the row was *new* and the caller should
/// dispatch a login-alert email. Returns `Ok(false)` when the device was
/// already on file or when both `user_agent` and `ip` are absent (no
/// usable fingerprint, so we suppress to avoid alerting on every login
/// from clients we can't identify, e.g. tests).
pub async fn check_and_record_device(
    pool: &SqlitePool,
    user_id: &str,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if user_agent.is_none() && ip.is_none() {
        return Ok(false);
    }
    let fp = fingerprint(user_agent, ip);
    let res = sqlx::query(
        "INSERT OR IGNORE INTO login_alert_devices (user_id, fingerprint) \
         VALUES (?, ?)",
    )
    .bind(user_id)
    .bind(&fp)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_distinguishes_components() {
        let a = fingerprint(Some("UA"), Some("1.1.1.1"));
        let b = fingerprint(Some("UA"), Some("1.1.1.1"));
        assert_eq!(a, b);
        let c = fingerprint(Some("UA"), Some("1.1.1.2"));
        assert_ne!(a, c);
        let d = fingerprint(Some("Other"), Some("1.1.1.1"));
        assert_ne!(a, d);
        // Joining without a separator would let "UA" + "1.1.1.1" collide
        // with "UA1.1.1" + ".1".
        let e = fingerprint(Some("UA1.1.1"), Some(".1"));
        assert_ne!(a, e);
    }
}

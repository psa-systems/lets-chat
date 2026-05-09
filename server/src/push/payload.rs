use bytes::Bytes;

use crate::ws::events::ChatEvent;

#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("not a Mentioned event")]
    WrongEvent,
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Build the Push payload JSON for a `Mentioned` event. Matches the
/// shape consumed by `server/assets/sw.js`'s `push` handler.
pub fn build(event: &ChatEvent) -> Result<Bytes, PayloadError> {
    let ChatEvent::Mentioned {
        kind,
        room_id,
        room_label,
        author_label,
        snippet,
        target_path,
        ..
    } = event
    else {
        return Err(PayloadError::WrongEvent);
    };
    let title = if kind == "dm" {
        format!("{author_label} (DM)")
    } else {
        format!("{author_label} in {room_label}")
    };
    let value = serde_json::json!({
        "title": title,
        "body":  snippet,
        "icon":  "/assets/notification-icon.png",
        "tag":   format!("lc-{room_id}"),
        "data":  { "target_path": target_path },
    });
    Ok(Bytes::from(serde_json::to_vec(&value)?))
}

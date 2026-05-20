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
    let (title, body, tag, target_path) = match event {
        ChatEvent::Mentioned {
            kind,
            room_id,
            room_label,
            author_label,
            snippet,
            target_path,
            ..
        } => {
            let title = if kind == "dm" {
                format!("{author_label} (DM)")
            } else {
                format!("{author_label} in {room_label}")
            };
            (title, snippet.clone(), format!("lc-{room_id}"), target_path)
        }
        // LC-63: reminder. Distinct tag so a reminder does not collapse onto
        // a room's mention notification.
        ChatEvent::Reminder {
            room_label,
            message_id,
            snippet,
            target_path,
            ..
        } => (
            format!("Reminder: {room_label}"),
            snippet.clone(),
            format!("lc-reminder-{message_id}"),
            target_path,
        ),
        _ => return Err(PayloadError::WrongEvent),
    };
    let value = serde_json::json!({
        "title": title,
        "body":  body,
        "icon":  "/assets/notification-icon.png",
        "tag":   tag,
        "data":  { "target_path": target_path },
    });
    Ok(Bytes::from(serde_json::to_vec(&value)?))
}

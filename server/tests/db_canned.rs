use lets_chat::db;
mod common;

// LC-487: per-user canned responses, surfaced as StaticText CustomCommands.

#[tokio::test]
async fn canned_insert_get_list_scoped_to_user() {
    let chat = common::chat_pool().await;

    let id = db::slash::insert_canned(&chat, "u-1", "sig", "my signature", "Cheers,\n{args}")
        .await
        .unwrap();

    let got = db::slash::get_canned_for_user(&chat, "u-1", "sig")
        .await
        .unwrap()
        .expect("present");
    assert_eq!(got.id, id);
    assert_eq!(got.name, "sig");
    assert_eq!(got.description, "my signature");
    assert_eq!(got.target, "Cheers,\n{args}");
    assert_eq!(got.kind, db::slash::CustomKind::StaticText);
    assert!(!got.admin_only);

    // Another user does not see it.
    assert!(db::slash::get_canned_for_user(&chat, "u-2", "sig")
        .await
        .unwrap()
        .is_none());
    assert!(db::slash::list_canned_for_user(&chat, "u-2")
        .await
        .unwrap()
        .is_empty());

    let mine = db::slash::list_canned_for_user(&chat, "u-1").await.unwrap();
    assert_eq!(mine.len(), 1);
}

#[tokio::test]
async fn canned_name_unique_per_user_not_globally() {
    let chat = common::chat_pool().await;
    db::slash::insert_canned(&chat, "u-1", "ty", "", "Thank you!")
        .await
        .unwrap();
    // Same name, same user -> unique violation.
    let err = db::slash::insert_canned(&chat, "u-1", "ty", "", "Thanks again")
        .await
        .unwrap_err();
    assert!(matches!(&err, sqlx::Error::Database(d) if d.is_unique_violation()));
    // Same name, different user -> allowed.
    db::slash::insert_canned(&chat, "u-2", "ty", "", "Gracias")
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_canned_is_owner_scoped() {
    let chat = common::chat_pool().await;
    let id = db::slash::insert_canned(&chat, "owner", "x", "", "hi")
        .await
        .unwrap();
    // Non-owner delete removes nothing.
    assert_eq!(
        db::slash::delete_canned(&chat, "intruder", id)
            .await
            .unwrap(),
        0
    );
    assert!(db::slash::get_canned_for_user(&chat, "owner", "x")
        .await
        .unwrap()
        .is_some());
    // Owner delete works.
    assert_eq!(
        db::slash::delete_canned(&chat, "owner", id).await.unwrap(),
        1
    );
    assert!(db::slash::get_canned_for_user(&chat, "owner", "x")
        .await
        .unwrap()
        .is_none());
}

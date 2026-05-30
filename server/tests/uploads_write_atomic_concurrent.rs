//! LC-208: regression guard for the `routes_uploads` flake.
//!
//! `uploads::write_atomic` is called with the SAME content-addressed final
//! path by concurrent uploads of identical bytes (the dedup check in
//! `post_upload` is TOCTOU, so both writers enter the write branch). Before
//! LC-208 the staging file was a fixed `{final}.partial`, so the two writers
//! raced on one staging path: the second `rename` hit ENOENT after the first
//! consumed it, and the upload 500'd. This pins the fix (unique staging name
//! per write) directly and deterministically - no dependence on the flaky
//! interleave that made the original failure intermittent.

use std::path::PathBuf;

fn unique_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "lc208-write-atomic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_identical_writes_to_same_path_all_succeed() {
    let dir = unique_dir();
    let final_path = dir.join("deadbeef.png");
    let bytes: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();

    // Many writers, same final path, identical bytes - the production race.
    let mut handles = Vec::new();
    for _ in 0..32 {
        let p = final_path.clone();
        let b = bytes.clone();
        handles.push(tokio::spawn(async move {
            lets_chat::uploads::write_atomic(&p, &b).await
        }));
    }

    let mut errors = Vec::new();
    for h in handles {
        match h.await.expect("join") {
            Ok(()) => {}
            Err(e) => errors.push(format!("{e}")),
        }
    }
    assert!(
        errors.is_empty(),
        "every concurrent write_atomic to the same path must succeed; got errors: {errors:?}"
    );

    // Final file exists with the exact bytes (atomic, last writer wins with
    // identical content).
    let on_disk = std::fs::read(&final_path).expect("final file present");
    assert_eq!(on_disk, bytes, "final bytes intact");

    // No staging `.partial` files leaked into the directory.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".partial"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no staging files should remain; found: {leftovers:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

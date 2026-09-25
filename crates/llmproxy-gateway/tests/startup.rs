use std::{process::Command, time::SystemTime};

#[test]
fn startup_requires_database_url_and_master_key() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-startup-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();

    for (database_url, message) in [
        (None, "LLMPROXY_DATABASE_URL is required"),
        (
            Some("postgresql://127.0.0.1/unused"),
            "LLMPROXY_MASTER_KEY is required",
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_llmproxy"));
        command.env_clear().current_dir(&directory);
        if let Some(database_url) = database_url {
            command.env("LLMPROXY_DATABASE_URL", database_url);
        }
        let output = command.output().expect("start without required setting");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
    }

    std::fs::remove_dir_all(directory).unwrap();
}

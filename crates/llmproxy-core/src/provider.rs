pub fn validate_upstream(
    host: &str,
    port: u16,
    anthropic_version: Option<&str>,
    connect_timeout_ms: u64,
    read_timeout_ms: u64,
    write_timeout_ms: u64,
) -> Result<(), &'static str> {
    if host.is_empty()
        || host.contains('/')
        || host.contains(':')
        || host.chars().any(char::is_whitespace)
    {
        return Err("host must be a DNS name or IPv4 address without scheme or path");
    }
    if port == 0 {
        return Err("port must be nonzero");
    }
    if connect_timeout_ms == 0 {
        return Err("connect_timeout_ms must be nonzero");
    }
    if read_timeout_ms == 0 {
        return Err("read_timeout_ms must be nonzero");
    }
    if write_timeout_ms == 0 {
        return Err("write_timeout_ms must be nonzero");
    }
    if let Some(version) = anthropic_version
        && (version.trim().is_empty()
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' '))
    {
        return Err("anthropic_version must be a nonempty printable ASCII header value");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_upstream;

    #[test]
    fn validates_provider_connection_settings() {
        assert!(validate_upstream("api.example.com", 443, None, 10_000, 60_000, 30_000).is_ok());
        assert!(validate_upstream("https://api.example.com", 443, None, 1, 1, 1).is_err());
        assert_eq!(
            validate_upstream("api.example.com", 0, None, 1, 1, 1),
            Err("port must be nonzero")
        );
        for timeouts in [(0, 1, 1), (1, 0, 1), (1, 1, 0)] {
            assert!(
                validate_upstream(
                    "api.example.com",
                    443,
                    None,
                    timeouts.0,
                    timeouts.1,
                    timeouts.2
                )
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_unsafe_anthropic_version() {
        for version in [
            "",
            "  ",
            "2023-06-01\r\nx-header: value",
            "\t",
            "\0",
            "版本",
            "\u{7f}",
        ] {
            assert!(validate_upstream("api.example.com", 443, Some(version), 1, 1, 1).is_err());
        }
        assert!(validate_upstream("api.example.com", 443, Some("2023-06-01"), 1, 1, 1).is_ok());
    }
}

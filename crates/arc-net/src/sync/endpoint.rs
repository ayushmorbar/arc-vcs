use std::fmt::{Display, Formatter};

use thiserror::Error;

/// Parsed native sync endpoint with a redacted display form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncEndpoint {
    socket_addr: String,
    redacted: String,
}

impl SyncEndpoint {
    /// Parse native sync endpoint input.
    ///
    /// Accepted forms:
    /// - `host:port`
    /// - `tcp://host:port`
    /// - `arc://host:port`
    /// - `arc+tcp://host:port`
    pub fn parse(input: &str) -> Result<Self, EndpointParseError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(EndpointParseError::Empty);
        }

        let authority = if let Some((scheme, rest)) = trimmed.split_once("://") {
            match scheme {
                "tcp" | "arc" | "arc+tcp" => {}
                _ => return Err(EndpointParseError::UnsupportedScheme),
            }

            let rest = rest.trim();
            if rest.is_empty() || rest == "/" {
                return Err(EndpointParseError::MissingAuthority);
            }

            if rest.contains('?') || rest.contains('#') {
                return Err(EndpointParseError::UnexpectedQueryOrFragment);
            }
            if rest.contains('@') {
                return Err(EndpointParseError::CredentialsNotAllowed);
            }
            if rest.contains('/') && rest != "/" {
                return Err(EndpointParseError::UnexpectedPath);
            }
            rest.trim_end_matches('/')
        } else {
            if trimmed.contains('?') || trimmed.contains('#') {
                return Err(EndpointParseError::UnexpectedQueryOrFragment);
            }
            if trimmed.contains('@') {
                return Err(EndpointParseError::CredentialsNotAllowed);
            }
            if trimmed.contains('/') {
                return Err(EndpointParseError::UnexpectedPath);
            }
            trimmed
        };

        let (host, port_text) =
            split_host_port(authority).ok_or(EndpointParseError::MissingPort)?;
        if host.is_empty() {
            return Err(EndpointParseError::MissingHost);
        }

        let port: u16 = port_text.parse().map_err(|_| EndpointParseError::InvalidPort)?;
        if port == 0 {
            return Err(EndpointParseError::InvalidPort);
        }

        let normalized_host = if is_bracketed_ipv6(host) {
            host.to_ascii_lowercase()
        } else {
            host.trim().to_ascii_lowercase()
        };

        let socket_addr = format!("{normalized_host}:{port}");
        Ok(Self { redacted: socket_addr.clone(), socket_addr })
    }

    /// Socket address suitable for `TcpStream::connect()`.
    pub fn socket_addr(&self) -> &str {
        &self.socket_addr
    }
}

impl Display for SyncEndpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.redacted)
    }
}

fn split_host_port(authority: &str) -> Option<(&str, &str)> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let rest = authority.get(end + 1..)?;
        let port = rest.strip_prefix(':')?;
        return Some((host, port));
    }
    authority.rsplit_once(':')
}

fn is_bracketed_ipv6(host: &str) -> bool {
    host.starts_with('[') && host.ends_with(']')
}

/// Endpoint parsing failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EndpointParseError {
    /// Endpoint string was empty after trimming.
    #[error("sync endpoint is empty")]
    Empty,
    /// Endpoint scheme is not one of the supported native sync schemes.
    #[error("sync endpoint uses unsupported scheme")]
    UnsupportedScheme,
    /// Endpoint contained query or fragment components which are disallowed.
    #[error("sync endpoint must not contain query or fragment")]
    UnexpectedQueryOrFragment,
    /// Endpoint contained a path component which is disallowed.
    #[error("sync endpoint must not contain path")]
    UnexpectedPath,
    /// Endpoint embedded credentials which are disallowed.
    #[error("sync endpoint credentials are not allowed")]
    CredentialsNotAllowed,
    /// Endpoint omitted authority section after scheme.
    #[error("sync endpoint is missing authority")]
    MissingAuthority,
    /// Endpoint omitted hostname.
    #[error("sync endpoint is missing host")]
    MissingHost,
    /// Endpoint omitted port.
    #[error("sync endpoint is missing port")]
    MissingPort,
    /// Endpoint port is not a valid integer in range 1..=65535.
    #[error("sync endpoint port must be in range 1..=65535")]
    InvalidPort,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parses_plain_host_port() {
        let endpoint = SyncEndpoint::parse("EXAMPLE.COM:4312").expect("valid endpoint");
        assert_eq!(endpoint.socket_addr(), "example.com:4312");
        assert_eq!(endpoint.to_string(), "example.com:4312");
    }

    #[test]
    fn parses_supported_schemes() {
        for value in ["tcp://localhost:4200", "arc://localhost:4200", "arc+tcp://localhost:4200"] {
            let endpoint = SyncEndpoint::parse(value).expect("valid endpoint");
            assert_eq!(endpoint.socket_addr(), "localhost:4200");
        }
    }

    #[test]
    fn rejects_credentials_and_query() {
        assert_eq!(
            SyncEndpoint::parse("tcp://user:secret@host:7777")
                .expect_err("must reject credentials"),
            EndpointParseError::CredentialsNotAllowed
        );
        assert_eq!(
            SyncEndpoint::parse("tcp://host:7777?token=abc").expect_err("must reject query"),
            EndpointParseError::UnexpectedQueryOrFragment
        );
    }

    #[test]
    fn rejects_invalid_port() {
        assert_eq!(
            SyncEndpoint::parse("host:0").expect_err("must reject port zero"),
            EndpointParseError::InvalidPort
        );
        assert_eq!(
            SyncEndpoint::parse("host:not-a-port").expect_err("must reject invalid port"),
            EndpointParseError::InvalidPort
        );
    }

    #[test]
    fn parser_handles_long_inputs_within_budget() {
        let samples = [
            format!("tcp://{}:4312", "a".repeat(8192)),
            format!("arc://[{}]:4312", "1:".repeat(2048)),
            "tcp://localhost:4312?token=secret".to_string(),
        ];

        let start = std::time::Instant::now();
        for sample in samples {
            let _ = SyncEndpoint::parse(&sample);
        }
        assert!(start.elapsed() < Duration::from_secs(1), "endpoint parsing exceeded budget");
    }
}

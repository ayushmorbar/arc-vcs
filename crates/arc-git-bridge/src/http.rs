use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::protocol::{pkt_flush, pkt_line};

fn service_base(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

pub async fn discover_refs(url: &str) -> Result<HashMap<String, String>> {
    let endpoint = format!("{}/info/refs?service=git-receive-pack", service_base(url));
    let response = reqwest::get(&endpoint)
        .await
        .with_context(|| format!("failed to GET {endpoint}"))?
        .error_for_status()
        .with_context(|| format!("remote returned error for {endpoint}"))?;
    let body = response.bytes().await.context("failed to read refs response")?;
    parse_info_refs_response(&body)
}

pub async fn push_packfile(
    url: &str,
    old_sha: &str,
    new_sha: &str,
    ref_name: &str,
    packfile: &[u8],
) -> Result<()> {
    let endpoint = format!("{}/git-receive-pack", service_base(url));
    let body = build_push_body(old_sha, new_sha, ref_name, packfile);

    let client = reqwest::Client::new();
    let response = client
        .post(&endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/x-git-receive-pack-request")
        .body(body)
        .send()
        .await
        .with_context(|| format!("failed to POST {endpoint}"))?
        .error_for_status()
        .with_context(|| format!("remote returned error for {endpoint}"))?;

    let payload = response.bytes().await.context("failed to read receive-pack response")?;
    validate_receive_pack_response(&payload)?;

    Ok(())
}

fn parse_info_refs_response(data: &[u8]) -> Result<HashMap<String, String>> {
    let mut refs = HashMap::new();
    let mut cursor = 0usize;

    while cursor + 4 <= data.len() {
        let len_hex = std::str::from_utf8(&data[cursor..cursor + 4])
            .context("invalid pkt-line length prefix encoding")?;
        let frame_len = usize::from_str_radix(len_hex, 16)
            .with_context(|| format!("invalid pkt-line length: {len_hex}"))?;
        cursor += 4;

        if frame_len == 0 {
            continue;
        }
        if frame_len < 4 {
            bail!("invalid pkt-line frame length: {frame_len}");
        }
        let payload_len = frame_len - 4;
        if cursor + payload_len > data.len() {
            bail!("pkt-line frame overruns response body");
        }

        let payload = &data[cursor..cursor + payload_len];
        cursor += payload_len;

        if payload.starts_with(b"#") {
            continue;
        }

        let line = payload.strip_suffix(b"\n").unwrap_or(payload);
        let line = line.split(|b| *b == 0).next().unwrap_or(line);
        let text = std::str::from_utf8(line).context("non-utf8 refs line")?;
        let mut parts = text.split_whitespace();

        let Some(sha) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };

        if sha.len() == 40 {
            refs.insert(name.to_string(), sha.to_string());
        }
    }

    Ok(refs)
}

pub(crate) fn build_push_body(
    old_sha: &str,
    new_sha: &str,
    ref_name: &str,
    packfile: &[u8],
) -> Vec<u8> {
    let command = format!("{old_sha} {new_sha} {ref_name}\0 report-status\n");
    let mut body = pkt_line(command.as_bytes());
    body.extend_from_slice(pkt_flush());
    body.extend_from_slice(packfile);
    body
}

fn validate_receive_pack_response(data: &[u8]) -> Result<()> {
    let mut cursor = 0usize;
    let mut saw_unpack_ok = false;
    let mut saw_ref_ok = false;

    while cursor + 4 <= data.len() {
        let len_hex = std::str::from_utf8(&data[cursor..cursor + 4])
            .context("invalid receive-pack pkt-line length prefix")?;
        let frame_len = usize::from_str_radix(len_hex, 16)
            .with_context(|| format!("invalid receive-pack pkt-line length: {len_hex}"))?;
        cursor += 4;

        if frame_len == 0 {
            continue;
        }
        if frame_len < 4 {
            bail!("invalid receive-pack pkt-line frame length: {frame_len}");
        }

        let payload_len = frame_len - 4;
        if cursor + payload_len > data.len() {
            bail!("receive-pack pkt-line frame overruns response body");
        }

        let payload = &data[cursor..cursor + payload_len];
        cursor += payload_len;

        let line = payload.strip_suffix(b"\n").unwrap_or(payload);
        let text = std::str::from_utf8(line).context("non-utf8 receive-pack status line")?;

        if text.starts_with("unpack ok") {
            saw_unpack_ok = true;
            continue;
        }
        if text.starts_with("ok ") {
            saw_ref_ok = true;
            continue;
        }
        if let Some(rest) = text.strip_prefix("ng ") {
            bail!("git receive-pack rejected update: {rest}");
        }
    }

    if !saw_unpack_ok {
        bail!("git receive-pack response missing 'unpack ok' status");
    }
    if !saw_ref_ok {
        bail!("git receive-pack response missing ref update status");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_push_body, validate_receive_pack_response};
    use crate::protocol::{pkt_flush, pkt_line};

    #[test]
    fn push_body_concatenates_command_flush_and_packfile() {
        let old_sha = "0000000000000000000000000000000000000000";
        let new_sha = "1111111111111111111111111111111111111111";
        let ref_name = "refs/heads/main";
        let packfile = b"PACK....";

        let body = build_push_body(old_sha, new_sha, ref_name, packfile);

        let cmd = format!("{old_sha} {new_sha} {ref_name}\0 report-status\n");
        let expected_prefix = pkt_line(cmd.as_bytes());
        let flush = pkt_flush();

        assert!(body.starts_with(&expected_prefix));
        assert_eq!(&body[expected_prefix.len()..expected_prefix.len() + flush.len()], flush);
        assert_eq!(&body[expected_prefix.len() + flush.len()..], packfile);
    }

    #[test]
    fn receive_pack_status_accepts_ok_reply() {
        let mut reply = Vec::new();
        reply.extend_from_slice(&pkt_line(b"unpack ok\n"));
        reply.extend_from_slice(&pkt_line(b"ok refs/heads/main\n"));
        reply.extend_from_slice(pkt_flush());

        validate_receive_pack_response(&reply).expect("ok receive-pack reply must pass");
    }

    #[test]
    fn receive_pack_status_rejects_ng_reply() {
        let mut reply = Vec::new();
        reply.extend_from_slice(&pkt_line(b"unpack ok\n"));
        reply.extend_from_slice(&pkt_line(b"ng refs/heads/main non-fast-forward\n"));
        reply.extend_from_slice(pkt_flush());

        let err = validate_receive_pack_response(&reply).expect_err("ng status must fail push");
        assert!(err.to_string().contains("rejected update"));
    }
}

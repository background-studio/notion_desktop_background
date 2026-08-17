use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use reqwest::{
    header::{CONTENT_LENGTH, CONTENT_TYPE},
    redirect::Policy,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::models::{DisplaySettings, MediaKind};

pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_URL_CHARS: usize = 2048;
pub const MAX_REVISION_CHARS: usize = 128;
pub const MAX_MIME_CHARS: usize = 64;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
pub struct PluginRequest {
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct MediaSpec {
    pub url: String,
    pub kind: MediaKind,
    pub mime_type: String,
    pub sha256: String,
    pub byte_size: u64,
}

#[derive(Clone, Debug)]
pub struct ConfigureCommand {
    pub revision: String,
    pub media: MediaSpec,
    pub display: DisplaySettings,
}

pub fn reject_oversized_line(line: &str) -> Result<(), String> {
    if line.len() > MAX_REQUEST_BYTES {
        return Err(format!("请求超过 {MAX_REQUEST_BYTES} 字节上限。"));
    }
    Ok(())
}

pub fn parse_request(line: &str) -> Result<PluginRequest, String> {
    reject_oversized_line(line)?;
    let request: PluginRequest =
        serde_json::from_str(line).map_err(|error| format!("无效请求：{error}"))?;
    if request.id.is_empty() || request.id.len() > 128 {
        return Err("请求 id 无效。".to_string());
    }
    if request.cmd.is_empty() || request.cmd.len() > 64 {
        return Err("请求 cmd 无效。".to_string());
    }
    Ok(request)
}

pub fn parse_configure(params: Option<&Value>) -> Result<ConfigureCommand, String> {
    let params = params.ok_or_else(|| "configure 缺少 params。".to_string())?;
    if params.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err("configure.schemaVersion 必须为 1。".to_string());
    }
    let revision = params
        .get("revision")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少必需字段：revision".to_string())?;
    if revision.is_empty() || revision.len() > MAX_REVISION_CHARS {
        return Err("revision 长度无效。".to_string());
    }
    let media = params
        .get("media")
        .and_then(Value::as_object)
        .ok_or_else(|| "缺少必需字段：media".to_string())?;
    let url = media
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少必需字段：media.url".to_string())?;
    validate_media_url(url)?;
    let kind = match media
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少必需字段：media.kind".to_string())?
    {
        "image" => MediaKind::Image,
        "video" => MediaKind::Video,
        other => return Err(format!("不支持的 media.kind：{other}")),
    };
    let mime_type = media
        .get("mimeType")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少必需字段：media.mimeType".to_string())?
        .to_ascii_lowercase();
    if mime_type.is_empty() || mime_type.len() > MAX_MIME_CHARS {
        return Err("media.mimeType 无效。".to_string());
    }
    if expected_kind(&mime_type) != Some(kind) {
        return Err("media.mimeType 与 media.kind 不匹配。".to_string());
    }
    let sha256 = media
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "缺少必需字段：media.sha256".to_string())?
        .to_ascii_lowercase();
    if sha256.len() != 64
        || !sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("media.sha256 必须是 64 位十六进制。".to_string());
    }
    let byte_size = media
        .get("byteSize")
        .and_then(Value::as_u64)
        .ok_or_else(|| "缺少必需字段：media.byteSize".to_string())?;
    if byte_size == 0 || byte_size > MAX_MEDIA_BYTES {
        return Err(format!(
            "media.byteSize 必须在 1..={MAX_MEDIA_BYTES} 之间。"
        ));
    }
    Ok(ConfigureCommand {
        revision: revision.to_string(),
        media: MediaSpec {
            url: url.to_string(),
            kind,
            mime_type,
            sha256,
            byte_size,
        },
        display: DisplaySettings::from_configure(params)?,
    })
}

pub fn validate_media_url(value: &str) -> Result<Url, String> {
    if value.len() > MAX_URL_CHARS {
        return Err("媒体地址过长。".to_string());
    }
    let url = Url::parse(value).map_err(|_| "媒体地址无效。".to_string())?;
    if url.scheme() != "http" {
        return Err("媒体地址只允许 http://127.0.0.1 或 http://localhost。".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("媒体地址不能包含账号信息。".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "媒体地址缺少主机名。".to_string())?;
    if !host.eq_ignore_ascii_case("127.0.0.1") && !host.eq_ignore_ascii_case("localhost") {
        return Err("媒体地址只允许 127.0.0.1 或 localhost。".to_string());
    }
    match url.port() {
        Some(0) => return Err("媒体地址端口无效。".to_string()),
        Some(_) | None => {}
    }
    Ok(url)
}

pub fn expected_kind(mime_type: &str) -> Option<MediaKind> {
    match mime_type {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" | "image/avif" => {
            Some(MediaKind::Image)
        }
        "video/mp4" | "video/webm" | "video/ogg" | "video/quicktime" => Some(MediaKind::Video),
        _ => None,
    }
}

fn normalize_content_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

async fn resolve_loopback(url: &Url) -> Result<Vec<SocketAddr>, String> {
    let host = url
        .host_str()
        .ok_or_else(|| "媒体地址缺少主机名。".to_string())?;
    let port = url
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or_else(|| "媒体地址端口无效。".to_string())?;
    if host == "127.0.0.1" {
        return Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]);
    }
    let mut addresses = Vec::new();
    for address in tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("本机地址解析失败：{error}"))?
    {
        if !is_loopback_ip(address.ip()) {
            return Err("localhost 解析到了非回环地址。".to_string());
        }
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    if addresses.is_empty() {
        return Err("localhost 没有可用的回环地址。".to_string());
    }
    Ok(addresses)
}

fn is_loopback_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_loopback(),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return mapped.is_loopback();
            }
            address.is_loopback()
        }
    }
}

pub async fn fetch_configured_media(spec: &MediaSpec) -> Result<Vec<u8>, String> {
    let url = validate_media_url(&spec.url)?;
    let addresses = resolve_loopback(&url).await?;
    let host = url.host_str().unwrap_or("127.0.0.1");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(FETCH_TIMEOUT)
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = client
        .get(url)
        .header("User-Agent", "Notion-Background-Studio-Worker/2")
        .header("Accept", "image/*,video/*;q=0.9")
        .send()
        .await
        .map_err(|error| format!("获取媒体失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "获取媒体失败，服务器返回 HTTP {}。",
            response.status().as_u16()
        ));
    }
    if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        if length != spec.byte_size {
            return Err("Content-Length 与 media.byteSize 不一致。".to_string());
        }
        if length > MAX_MEDIA_BYTES {
            return Err("媒体超过 64 MiB 上限。".to_string());
        }
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(normalize_content_type)
        .unwrap_or_default();
    if content_type != spec.mime_type {
        return Err("响应 Content-Type 与 media.mimeType 不一致。".to_string());
    }
    let initial_capacity =
        usize::try_from(spec.byte_size.min(MAX_MEDIA_BYTES)).unwrap_or(MAX_MEDIA_BYTES as usize);
    let mut bytes = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("读取媒体失败：{error}"))?
    {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| "媒体大小溢出。".to_string())?;
        if next_len as u64 > MAX_MEDIA_BYTES || next_len as u64 > spec.byte_size {
            return Err("实际媒体大小超过 media.byteSize 或 64 MiB 上限。".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != spec.byte_size {
        return Err("实际媒体大小与 media.byteSize 不一致。".to_string());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != spec.sha256 {
        return Err("媒体 sha256 与配置不一致。".to_string());
    }
    Ok(bytes)
}

pub fn hello_result() -> Value {
    serde_json::json!({
        "pluginProtocol": crate::plugin::PLUGIN_PROTOCOL,
        "pluginId": crate::plugin::PLUGIN_ID,
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": {
            "mediaKinds": ["image", "video"],
            "managedLaunch": true,
            "autoTakeover": true,
            "hotUpdate": true,
            "blobInject": true,
            "loopbackMediaOnly": true,
            "keepsTargetOnShutdown": true,
            "maxMediaBytes": MAX_MEDIA_BYTES,
            "commands": [
                "hello",
                "configure",
                "status",
                "apply",
                "pause",
                "restore",
                "shutdown"
            ],
            "displayKeys": [
                "fit",
                "positionX",
                "positionY",
                "opacity",
                "blur",
                "scale",
                "overlayColor",
                "overlayOpacity",
                "blockFillOpacity",
                "homeIntensity",
                "taskIntensity",
                "sidebarOpacity",
                "surfaceOpacity",
                "composerOpacity",
                "menuOpacity",
                "terminalOpacity",
                "enabledOnHome",
                "enabledOnTasks",
                "videoMuted",
                "videoPlaybackRate"
            ]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn valid_display() -> Value {
        serde_json::to_value(DisplaySettings::default()).unwrap()
    }

    fn configure_json(url: &str, sha256: &str, byte_size: u64, mime: &str, kind: &str) -> Value {
        json!({
            "schemaVersion": 1,
            "revision": "rev-1",
            "media": {
                "url": url,
                "kind": kind,
                "mimeType": mime,
                "sha256": sha256,
                "byteSize": byte_size
            },
            "display": valid_display()
        })
    }

    #[test]
    fn rejects_oversized_and_invalid_requests() {
        assert!(reject_oversized_line(&"x".repeat(MAX_REQUEST_BYTES + 1)).is_err());
        assert!(parse_request(r#"{"id":"1","cmd":"hello"}"#).is_ok());
        assert!(parse_request(r#"{"id":"","cmd":"hello"}"#).is_err());
    }

    #[test]
    fn rejects_unsafe_media_urls() {
        for url in [
            "https://127.0.0.1/media",
            "http://user@127.0.0.1/media",
            "http://user:pass@localhost/media",
            "http://192.168.1.2/media",
            "http://8.8.8.8/media",
            "http://127.0.0.2/media",
            "http://[::1]/media",
            "file:///C:/secret.png",
            &format!("http://127.0.0.1/{}", "a".repeat(MAX_URL_CHARS)),
        ] {
            assert!(validate_media_url(url).is_err(), "{url}");
        }
        assert!(validate_media_url("http://127.0.0.1:47821/media/abc?v=1").is_ok());
        assert!(validate_media_url("http://localhost:47821/media/abc").is_ok());
    }

    #[test]
    fn rejects_invalid_configure_fields() {
        assert!(parse_configure(None).is_err());
        assert!(parse_configure(Some(&json!({ "schemaVersion": 2 }))).is_err());
        let sha = "a".repeat(64);
        assert!(parse_configure(Some(&configure_json(
            "http://127.0.0.1:9/x",
            &sha,
            12,
            "image/png",
            "video"
        )))
        .is_err());
        assert!(parse_configure(Some(&configure_json(
            "http://127.0.0.1:9/x",
            "zzz",
            12,
            "image/png",
            "image"
        )))
        .is_err());
        assert!(parse_configure(Some(&configure_json(
            "http://127.0.0.1:9/x",
            &sha,
            0,
            "image/png",
            "image"
        )))
        .is_err());
    }

    fn serve_http(status: &str, content_type: &str, body: &[u8], extra_length: Option<u64>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_string();
        let content_type = content_type.to_string();
        let body = body.to_vec();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let length = extra_length.unwrap_or(body.len() as u64);
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        });
        port
    }

    #[tokio::test]
    async fn fetches_valid_loopback_media() {
        let body = b"png-bytes";
        let digest = format!("{:x}", Sha256::digest(body));
        let port = serve_http("200 OK", "image/png", body, None);
        let spec = MediaSpec {
            url: format!("http://127.0.0.1:{port}/media/1"),
            kind: MediaKind::Image,
            mime_type: "image/png".to_string(),
            sha256: digest,
            byte_size: body.len() as u64,
        };
        assert_eq!(fetch_configured_media(&spec).await.unwrap(), body);
    }

    #[tokio::test]
    async fn rejects_content_length_and_hash_mismatches() {
        let body = b"png-bytes";
        let digest = format!("{:x}", Sha256::digest(body));
        let port = serve_http("200 OK", "image/png", body, Some(4));
        let spec = MediaSpec {
            url: format!("http://127.0.0.1:{port}/media/1"),
            kind: MediaKind::Image,
            mime_type: "image/png".to_string(),
            sha256: digest.clone(),
            byte_size: body.len() as u64,
        };
        assert!(fetch_configured_media(&spec)
            .await
            .unwrap_err()
            .contains("Content-Length"));

        let port = serve_http("200 OK", "image/jpeg", body, None);
        let spec = MediaSpec {
            url: format!("http://127.0.0.1:{port}/media/1"),
            kind: MediaKind::Image,
            mime_type: "image/png".to_string(),
            sha256: digest.clone(),
            byte_size: body.len() as u64,
        };
        assert!(fetch_configured_media(&spec)
            .await
            .unwrap_err()
            .contains("Content-Type"));

        let port = serve_http("200 OK", "image/png", body, None);
        let spec = MediaSpec {
            url: format!("http://127.0.0.1:{port}/media/1"),
            kind: MediaKind::Image,
            mime_type: "image/png".to_string(),
            sha256: "b".repeat(64),
            byte_size: body.len() as u64,
        };
        assert!(fetch_configured_media(&spec)
            .await
            .unwrap_err()
            .contains("sha256"));
    }
}

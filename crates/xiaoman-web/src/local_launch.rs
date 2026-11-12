//! The `--local` classroom mode: port fallback, a generated access code, the LAN addresses and the
//! `/connect` QR page. Ported from `web-server/.../LocalLaunch.kt`.
//!
//! The connect page is served only to requests whose peer is loopback and whose `Host` header is a
//! localhost name, so the access code it prints cannot be read through a DNS-rebinding request.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::random;
use crate::server::AppState;

/// No `0/o/1/l/i`, so the code is easy to read off a screen and type on a phone.
const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";

const CONNECT_CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self'; connect-src 'self'; frame-ancestors 'none'";

pub const CONNECT_HTML: &[u8] = include_bytes!("../resources/connect/index.html");
pub const CONNECT_CSS: &[u8] = include_bytes!("../resources/connect/connect.css");
pub const CONNECT_JS: &[u8] = include_bytes!("../resources/connect/connect.js");
pub const QRCODE_JS: &[u8] = include_bytes!("../resources/connect/qrcode.js");

/// `LocalLaunch.accessCode`: a six-character code from [`ALPHABET`].
pub fn access_code(length: usize) -> String {
    let mut bytes = vec![0u8; length.max(1)];
    random::fill(&mut bytes);
    bytes
        .into_iter()
        .map(|byte| ALPHABET[byte as usize % ALPHABET.len()] as char)
        .collect()
}

/// One address the phone can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanAddress {
    pub address: String,
    pub label: String,
    pub interface: String,
}

/// Private IPv4 addresses of this computer, Wi-Fi first, then wired, then everything else.
pub fn lan_addresses() -> Vec<LanAddress> {
    let mut found: Vec<(String, String, String)> = Vec::new();
    for interface in if_addrs::get_if_addrs().unwrap_or_default() {
        if interface.is_loopback() {
            continue;
        }
        let IpAddr::V4(v4) = interface.ip() else {
            continue;
        };
        if !v4.is_private() {
            continue;
        }
        let name = interface.name.clone();
        let display = interface.name.clone();
        let address = v4.to_string();
        found.push((name, display, address));
    }
    found.sort_by(|a, b| {
        rank(&a.0, &a.1, &a.2)
            .cmp(&rank(&b.0, &b.1, &b.2))
            .then_with(|| a.2.cmp(&b.2))
    });
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|(_, _, address)| seen.insert(address.clone()))
        .map(|(name, display, address)| LanAddress {
            label: label(&name, &display).to_string(),
            interface: if display.is_empty() { name } else { display },
            address,
        })
        .collect()
}

const VIRTUAL: &[&str] = &[
    "docker",
    "veth",
    "br-",
    "virbr",
    "vmnet",
    "vbox",
    "virtualbox",
    "vmware",
    "hyper-v",
    "vethernet",
    "utun",
    "tun",
    "tap",
    "wsl",
    "tailscale",
    "zerotier",
    "awdl",
    "llw",
    "anpi",
    "loopback",
    "bluetooth",
    "vpn",
];
const WIFI: &[&str] = &[
    "wi-fi", "wifi", "wlan", "wireless", "802.11", "airport", "无线",
];
const WIRED: &[&str] = &["eth", "ethernet", "以太网", "enp", "eno", "ens"];

fn kind(name: &str, display: &str) -> &'static str {
    let text = format!("{name} {display}").to_lowercase();
    if VIRTUAL.iter().any(|item| text.contains(item)) {
        "virtual"
    } else if WIFI.iter().any(|item| text.contains(item)) || name.starts_with("wl") || name == "en0"
    {
        "wifi"
    } else if WIRED.iter().any(|item| text.contains(item)) || is_en_number(name) {
        "wired"
    } else {
        "other"
    }
}

/// `en0`, `en1`, …: the macOS wired/wireless interface names.
fn is_en_number(name: &str) -> bool {
    name.strip_prefix("en")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

fn rank(name: &str, display: &str, address: &str) -> u8 {
    let kind = kind(name, display);
    let base = match kind {
        "wifi" => 0,
        "wired" => 1,
        "other" => 2,
        _ => 4,
    };
    let private_first = if address.starts_with("192.168.") || address.starts_with("172.20.10.") {
        0
    } else {
        1
    };
    base + private_first
}

fn label(name: &str, display: &str) -> &'static str {
    match kind(name, display) {
        "wifi" => "Wi-Fi/热点",
        "wired" => "有线网络",
        "virtual" => "虚拟网卡（手机一般连不上）",
        _ => "其他网络",
    }
}

/// The terminal block the student reads: LAN links, the access code and the connect page.
pub fn announce(port: u16, access_code: Option<&str>, out: &mut dyn Write) {
    let addresses = lan_addresses();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "================ 小满网页版已在这台电脑上启动 ================"
    );
    let _ = writeln!(
        out,
        "电脑浏览器打开：http://127.0.0.1:{port}/connect （扫码页，会自动打开）"
    );
    if addresses.is_empty() {
        let _ = writeln!(
            out,
            "没找到局域网地址：请先连上 Wi-Fi，或让电脑连手机热点，然后在扫码页点“刷新地址”。"
        );
    } else {
        let _ = writeln!(out, "手机（和电脑连同一个 Wi-Fi）在浏览器里打开：");
        for (index, address) in addresses.iter().enumerate() {
            let marker = if index == 0 { "→" } else { " " };
            let _ = writeln!(
                out,
                "  {marker} http://{}:{port}/   （{}：{}）",
                address.address, address.label, address.interface
            );
        }
    }
    if let Some(code) = access_code.filter(|code| !code.is_empty()) {
        let _ = writeln!(out, "访问码：{code}");
    }
    let _ = writeln!(out, "关掉这个窗口（或按 Ctrl+C）就会停止小满。");
    let _ = writeln!(
        out,
        "=============================================================="
    );
    let _ = writeln!(out);
}

/// Opens the connect page in the default browser; a failure is ignored, the address was printed.
pub fn open_browser(url: &str) {
    let command: Vec<&str> = if cfg!(target_os = "macos") {
        vec!["open", url]
    } else if cfg!(windows) {
        vec!["rundll32", "url.dll,FileProtocolHandler", url]
    } else {
        vec!["xdg-open", url]
    };
    let _ = std::process::Command::new(command[0])
        .args(&command[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// `skills/examples` (then `skills`) near the working directory or the executable.
pub fn local_skills_root() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        places.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            places.push(dir.to_path_buf());
            if let Some(parent) = dir.parent() {
                places.push(parent.to_path_buf());
            }
        }
    }
    for place in places {
        for candidate in [place.join("skills/examples"), place.join("skills")] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn is_connect_path(path: &str) -> bool {
    path == "/connect" || path.starts_with("/connect/")
}

fn from_this_computer(peer: Option<SocketAddr>, host: Option<&str>) -> bool {
    let Some(peer) = peer else { return false };
    if !peer.ip().is_loopback() {
        return false;
    }
    let host = host.unwrap_or_default().to_lowercase();
    let host = if host.starts_with('[') {
        match host.find(']') {
            Some(end) => host[..=end].to_string(),
            None => host.clone(),
        }
    } else {
        host.split(':').next().unwrap_or_default().to_string()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

fn bytes_response(
    status: StatusCode,
    content_type: &'static str,
    body: &'static [u8],
    csp: bool,
) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .expect("a fixed response is valid");
    if csp {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(CONNECT_CSP),
        );
    }
    response
}

/// Serves `/connect` (page, script, styles, QR library) and `/connect/info`.
pub async fn serve(state: &AppState, peer: Option<SocketAddr>, request: Request<Body>) -> Response {
    if !from_this_computer(
        peer,
        request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
    ) {
        return (
            StatusCode::FORBIDDEN,
            "扫码页只能在运行小满的这台电脑上打开（地址用 127.0.0.1 或 localhost）。",
        )
            .into_response();
    }
    if request.method() != axum::http::Method::GET {
        return (StatusCode::METHOD_NOT_ALLOWED, "请求方法不支持").into_response();
    }
    let path = request.uri().path().to_string();
    let options = state.options();
    if path == "/connect/info" {
        let addresses: Vec<_> = lan_addresses()
            .into_iter()
            .map(|address| {
                json!({
                    "address": address.address,
                    "label": address.label,
                    "interface": address.interface,
                    "url": format!("http://{}:{}/", address.address, options.port),
                })
            })
            .collect();
        let body = json!({
            "port": options.port,
            "accessCode": options.access_code.clone().unwrap_or_default(),
            "stdioMcp": options.stdio_mcp,
            "addresses": addresses,
        });
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(body.to_string()))
            .expect("a JSON response is valid");
    }
    let name = match path.as_str() {
        "/connect" | "/connect/" => "index.html",
        other => other.trim_start_matches("/connect/"),
    };
    let (content_type, bytes) = match name {
        "index.html" => ("text/html; charset=utf-8", CONNECT_HTML),
        "connect.css" => ("text/css; charset=utf-8", CONNECT_CSS),
        "connect.js" => ("text/javascript; charset=utf-8", CONNECT_JS),
        "qrcode.js" => ("text/javascript; charset=utf-8", QRCODE_JS),
        _ => return (StatusCode::NOT_FOUND, "文件不存在").into_response(),
    };
    bytes_response(StatusCode::OK, content_type, bytes, name == "index.html")
}

/// True when `path` names a file the connect handler serves; used by the tests.
pub fn is_connect_file(path: &str) -> bool {
    matches!(
        path,
        "/connect/connect.css" | "/connect/connect.js" | "/connect/qrcode.js"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_use_the_readable_alphabet() {
        for _ in 0..50 {
            let code = access_code(6);
            assert_eq!(6, code.chars().count());
            assert!(
                code.bytes().all(|byte| ALPHABET.contains(&byte)),
                "unexpected character in {code}"
            );
        }
    }

    #[test]
    fn connect_paths_are_the_page_and_its_assets() {
        assert!(is_connect_path("/connect"));
        assert!(is_connect_path("/connect/"));
        assert!(is_connect_path("/connect/info"));
        assert!(is_connect_path("/connect/connect.js"));
        assert!(!is_connect_path("/"));
        assert!(!is_connect_path("/connectx"));
        assert!(is_connect_file("/connect/qrcode.js"));
    }

    #[test]
    fn only_a_loopback_peer_with_a_local_host_gets_the_page() {
        let local: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let remote: SocketAddr = "192.168.0.9:5000".parse().unwrap();
        assert!(from_this_computer(Some(local), Some("localhost:8787")));
        assert!(from_this_computer(Some(local), Some("127.0.0.1")));
        assert!(from_this_computer(
            Some("[::1]:5000".parse().unwrap()),
            Some("[::1]:8787")
        ));
        assert!(!from_this_computer(Some(remote), Some("localhost:8787")));
        assert!(!from_this_computer(Some(local), Some("evil.example:8787")));
        assert!(!from_this_computer(None, Some("localhost:8787")));
    }

    #[test]
    fn the_announcement_carries_the_code_and_the_connect_hint() {
        let mut buffer = Vec::new();
        announce(8787, Some("abc234"), &mut buffer);
        let text = String::from_utf8(buffer).unwrap();
        assert!(text.contains("http://127.0.0.1:8787/connect"), "{text}");
        assert!(text.contains("访问码：abc234"), "{text}");
    }
}

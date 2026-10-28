//! base64 节点链接列表（vless/vmess/ss/trojan/socks5/hysteria2）转换为 Clash 配置。
//!
//! 不少机场只提供 `vmess://`、`vless://` 这类节点链接列表，并用 base64 整体编码；
//! 内核无法直接消费。这里在保存订阅前完成转换，落盘的始终是内核可用的 YAML。
//!
//! 转换失败时返回 `None`，调用方回退为原始内容，避免把可用订阅改坏。

/// 尝试把订阅正文转换为 Clash YAML；不是节点链接列表时返回 `None`。
pub(crate) fn decode_subscription(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() || looks_like_clash_config(trimmed) {
        return None;
    }
    let decoded = decode_base64(trimmed)?;
    let proxies = parse_uri_list(&decoded);
    if proxies.is_empty() {
        return None;
    }
    Some(build_yaml(&proxies))
}

fn looks_like_clash_config(text: &str) -> bool {
    text.starts_with("proxies:")
        || text.starts_with("proxy-groups:")
        || text.starts_with("mixed-port:")
        || text.starts_with("port:")
        || text.contains("\nproxies:")
}

fn decode_base64(input: &str) -> Option<String> {
    let mut buf = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut n = 0usize;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => 64,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        chunk[n] = value;
        n += 1;
        if n == 4 {
            buf.push((chunk[0] << 2) | (chunk[1] >> 4));
            if chunk[2] != 64 {
                buf.push((chunk[1] << 4) | (chunk[2] >> 2));
                if chunk[3] != 64 {
                    buf.push((chunk[2] << 6) | chunk[3]);
                }
            }
            n = 0;
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 一条解析出来的节点；字段很少，够生成内核配置即可。
struct Node {
    name: String,
    kind: String,
    server: String,
    port: u16,
    extra: Vec<(String, String)>,
}

fn parse_uri_list(text: &str) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(mut node) = parse_uri(line) {
            let mut name = node.name.clone();
            let mut i = 2;
            while seen.contains(&name) {
                name = format!("{}-{}", node.name, i);
                i += 1;
            }
            seen.insert(name.clone());
            node.name = name;
            nodes.push(node);
        }
    }
    nodes
}

fn split_query(body: &str) -> (&str, Vec<(String, String)>) {
    match body.split_once('?') {
        Some((head, query)) => {
            let params = query
                .split('&')
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                    Some((k.to_owned(), percent_decode(v)?))
                })
                .collect();
            (head, params)
        }
        None => (body, Vec::new()),
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn parse_host_port(value: &str) -> Option<(String, u16)> {
    let value = value.split('/').next().unwrap_or(value);
    let (host, port) = value.rsplit_once(':')?;
    Some((host.trim_matches(['[', ']']).to_owned(), port.parse().ok()?))
}

fn get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn parse_uri(uri: &str) -> Option<Node> {
    let (scheme, rest) = uri.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let scheme = if scheme == "hy2" { "hysteria2".to_owned() } else { scheme };
    let (rest, name) = match rest.split_once('#') {
        Some((head, frag)) => (head, percent_decode(frag)?),
        None => (rest, String::new()),
    };

    match scheme.as_str() {
        "vless" => {
            let (head, params) = split_query(rest);
            let (uuid, host_port) = head.rsplit_once('@')?;
            let (server, port) = parse_host_port(host_port)?;
            let net = get(&params, "type").unwrap_or("tcp");
            let security = get(&params, "security").unwrap_or("");
            let sni = get(&params, "sni")
                .or_else(|| get(&params, "host"))
                .unwrap_or(&server)
                .to_owned();
            let mut extra = vec![("uuid".to_owned(), uuid.to_owned())];
            if security == "tls" || security == "reality" {
                extra.push(("tls".to_owned(), "true".to_owned()));
                extra.push(("servername".to_owned(), sni.clone()));
            }
            if security == "reality" {
                if let Some(pbk) = get(&params, "pbk") {
                    extra.push(("reality-public-key".to_owned(), pbk.to_owned()));
                }
                if let Some(sid) = get(&params, "sid") {
                    extra.push(("reality-short-id".to_owned(), sid.to_owned()));
                }
            }
            if let Some(fp) = get(&params, "fp") {
                extra.push(("client-fingerprint".to_owned(), fp.to_owned()));
            }
            if let Some(flow) = get(&params, "flow") {
                extra.push(("flow".to_owned(), flow.to_owned()));
            }
            extra.push(("network".to_owned(), net.to_owned()));
            if net == "ws" {
                extra.push(("ws-path".to_owned(), get(&params, "path").unwrap_or("/").to_owned()));
                extra.push((
                    "ws-host".to_owned(),
                    get(&params, "host").unwrap_or(&sni).to_owned(),
                ));
            }
            Some(Node {
                name: if name.is_empty() { format!("{server}:{port}") } else { name },
                kind: "vless".to_owned(),
                server,
                port,
                extra,
            })
        }
        "vmess" => {
            let json = decode_base64(rest)?;
            let node: serde_json::Value = serde_json::from_str(&json).ok()?;
            let server = node.get("add")?.as_str()?.to_owned();
            let port = node.get("port")?.as_str().and_then(|p| p.parse().ok())?;
            let mut extra = vec![("uuid".to_owned(), node.get("id")?.as_str()?.to_owned())];
            extra.push(("alterId".to_owned(), "0".to_owned()));
            extra.push(("cipher".to_owned(), "auto".to_owned()));
            let net = node.get("net").and_then(|v| v.as_str()).unwrap_or("tcp");
            extra.push(("network".to_owned(), net.to_owned()));
            if node.get("tls").and_then(|v| v.as_str()) == Some("tls") {
                extra.push(("tls".to_owned(), "true".to_owned()));
                let sni = node
                    .get("sni")
                    .and_then(|v| v.as_str())
                    .or_else(|| node.get("host").and_then(|v| v.as_str()))
                    .unwrap_or(&server)
                    .to_owned();
                extra.push(("servername".to_owned(), sni));
            }
            if net == "ws" {
                extra.push((
                    "ws-path".to_owned(),
                    node.get("path").and_then(|v| v.as_str()).unwrap_or("/").to_owned(),
                ));
                extra.push((
                    "ws-host".to_owned(),
                    node.get("host").and_then(|v| v.as_str()).unwrap_or(&server).to_owned(),
                ));
            }
            Some(Node {
                name: if name.is_empty() {
                    node.get("ps")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&format!("{server}:{port}"))
                        .to_owned()
                } else {
                    name
                },
                kind: "vmess".to_owned(),
                server,
                port,
                extra,
            })
        }
        "trojan" => {
            let (head, params) = split_query(rest);
            let (password, host_port) = head.rsplit_once('@')?;
            let (server, port) = parse_host_port(host_port)?;
            let mut extra = vec![("password".to_owned(), password.to_owned())];
            if let Some(sni) = get(&params, "sni") {
                extra.push(("sni".to_owned(), sni.to_owned()));
            }
            if get(&params, "allowInsecure") == Some("1") {
                extra.push(("skip-cert-verify".to_owned(), "true".to_owned()));
            }
            Some(Node {
                name: if name.is_empty() { format!("{server}-{port}") } else { name },
                kind: "trojan".to_owned(),
                server,
                port,
                extra,
            })
        }
        "socks5" | "socks" => {
            let (server, port) = parse_host_port(rest)?;
            Some(Node {
                name: if name.is_empty() { format!("socks5-{server}:{port}") } else { name },
                kind: "socks5".to_owned(),
                server,
                port,
                extra: Vec::new(),
            })
        }
        "hysteria2" => {
            let (head, params) = split_query(rest);
            let (auth, host_port) = head.rsplit_once('@')?;
            let (server, port) = parse_host_port(host_port)?;
            let mut extra = vec![("password".to_owned(), auth.to_owned())];
            if let Some(sni) = get(&params, "sni") {
                extra.push(("sni".to_owned(), sni.to_owned()));
            }
            if get(&params, "insecure") == Some("1") {
                extra.push(("skip-cert-verify".to_owned(), "true".to_owned()));
            }
            Some(Node {
                name: if name.is_empty() { format!("{server}:{port}") } else { name },
                kind: "hysteria2".to_owned(),
                server,
                port,
                extra,
            })
        }
        _ => None,
    }
}

fn yaml_escape(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn build_yaml(nodes: &[Node]) -> String {
    let mut out = String::from("proxies:\n");
    for node in nodes {
        out.push_str(&format!("  - name: {}\n", yaml_escape(&node.name)));
        out.push_str(&format!("    type: {}\n", node.kind));
        out.push_str(&format!("    server: {}\n", yaml_escape(&node.server)));
        out.push_str(&format!("    port: {}\n", node.port));
        out.push_str("    udp: true\n");
        for (key, value) in &node.extra {
            match key.as_str() {
                "tls" => out.push_str("    tls: true\n"),
                "skip-cert-verify" => out.push_str("    skip-cert-verify: true\n"),
                "ws-path" => {}
                "ws-host" => {}
                "reality-public-key" => {
                    out.push_str("    reality-opts:\n");
                    out.push_str(&format!("      public-key: {}\n", yaml_escape(value)));
                }
                "reality-short-id" => {
                    out.push_str(&format!("      short-id: {}\n", yaml_escape(value)));
                }
                "network" => {
                    out.push_str(&format!("    network: {}\n", value));
                }
                _ => out.push_str(&format!("    {}: {}\n", key, yaml_escape(value))),
            }
        }
        // ws 选项整体输出，避免与逐项输出重复。
        if let (Some(path), Some(host)) = (
            node.extra.iter().find(|(k, _)| k == "ws-path").map(|(_, v)| v),
            node.extra.iter().find(|(k, _)| k == "ws-host").map(|(_, v)| v),
        ) {
            out.push_str("    ws-opts:\n");
            out.push_str(&format!("      path: {}\n", yaml_escape(path)));
            out.push_str("      headers:\n");
            out.push_str(&format!("        Host: {}\n", yaml_escape(host)));
        }
    }
    out.push_str("proxy-groups:\n");
    out.push_str("  - name: \"PROXY\"\n    type: select\n    proxies:\n");
    for node in nodes {
        out.push_str(&format!("      - {}\n", yaml_escape(&node.name)));
    }
    out.push_str("      - \"DIRECT\"\n");
    out.push_str("rules:\n  - \"GEOIP,LAN,DIRECT,no-resolve\"\n  - \"GEOIP,CN,DIRECT\"\n  - \"MATCH,PROXY\"\n");
    out
}

//! OpenConnect provider: GlobalProtect, AnyConnect, Fortinet, Pulse/Ivanti, F5, Array.

use tauri::{AppHandle, Runtime};

use super::{components, Action, ConnectReq, Launch, Parser};

const PROTOCOLS: [&str; 6] = ["gp", "anyconnect", "fortinet", "pulse", "f5", "array"];

pub fn launch<R: Runtime>(app: &AppHandle<R>, req: &ConnectReq, socks_port: Option<u16>) -> Result<Launch, String> {
    let program = components::find(app, "openconnect", "openconnect").ok_or("COMPONENT_MISSING: openconnect")?;
    let proto = req.oc_protocol.as_deref().filter(|p| PROTOCOLS.contains(p)).unwrap_or("anyconnect");
    let mut args = vec![format!("--protocol={proto}")];
    if !req.user.is_empty() {
        args.push(format!("--user={}", req.user));
    }
    if let Some(g) = req.group.as_deref().filter(|g| !g.is_empty()) {
        args.push(format!("--authgroup={g}"));
    }
    if let Some(c) = req.cert_path.as_deref().filter(|c| !c.is_empty()) {
        args.push(format!("--certificate={c}"));
    }
    if let Some(pin) = req.servercert.as_deref().filter(|p| !p.is_empty()) {
        args.push(format!("--servercert={pin}"));
    }
    match socks_port {
        // App mode: no tun device, no root — ocproxy serves a SOCKS5 proxy on localhost.
        Some(port) => {
            let ocproxy = components::find(app, "openconnect", "ocproxy").ok_or("COMPONENT_MISSING: ocproxy")?;
            let q = ocproxy.to_string_lossy().replace('\'', "'\\''");
            args.push("--script-tun".into());
            args.push(format!("--script='{q}' -D {port} -k 30"));
        }
        // Global mode: routes/DNS via vpnc-script (shipped with the component).
        None => {
            if let Some(s) = components::support_file(
                app,
                "openconnect",
                if cfg!(windows) { "vpnc-script-win.js" } else { "vpnc-script" },
                if cfg!(windows) { &["vpnc-script-win.js"] } else {
                    &["/opt/homebrew/etc/vpnc/vpnc-script", "/usr/local/etc/vpnc/vpnc-script", "/etc/vpnc/vpnc-script", "/usr/share/vpnc-scripts/vpnc-script"]
                },
            ) {
                let path = s.to_string_lossy().into_owned();
                if cfg!(windows) {
                    // Windows: vpnc-script-win.js, run by openconnect via cscript; no shell quoting.
                    args.push(format!("--script={path}"));
                } else {
                    // openconnect runs --script through /bin/sh: quote (path may contain spaces).
                    args.push(format!("--script='{}'", path.replace('\'', "'\\''")));
                }
            }
        }
    }
    args.push("--reconnect-timeout=60".into());
    args.push(req.server.clone());
    Ok(Launch { program, args })
}

pub struct OcParser {
    user: String,
    password: Option<String>,
    group: Option<String>,
    passwords_sent: u32,
    login_failed: bool,
    pin: Option<String>,
    last_info: String,
}

impl OcParser {
    pub fn new(req: &ConnectReq) -> Self {
        Self {
            user: req.user.clone(),
            password: req.password.clone(),
            group: req.group.clone(),
            passwords_sent: 0,
            login_failed: false,
            pin: None,
            last_info: String::new(),
        }
    }
}

fn bytes_in(seg: &str) -> Option<u64> {
    // "10 packets (564 B)"
    let a = seg.find('(')?;
    seg[a + 1..].split_whitespace().next()?.parse().ok()
}

impl Parser for OcParser {
    fn line(&mut self, line: &str) -> Vec<Action> {
        let l = line.trim();
        let low = l.to_ascii_lowercase();
        if let Some(rest) = l.strip_prefix("Configured as ") {
            let ip = rest.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("").to_string();
            return vec![Action::Connected { ip }];
        }
        if l.starts_with("RX: ") {
            let mut parts = l.split(';');
            let rx = parts.next().and_then(bytes_in).unwrap_or(0);
            let tx = parts.next().and_then(bytes_in).unwrap_or(0);
            return vec![Action::Stats { rx, tx }];
        }
        if let Some(i) = l.find("--servercert ") {
            self.pin = Some(l[i + 13..].trim().to_string());
            return vec![];
        }
        if low.contains("login failed") || low.contains("authentication failed") || low.contains("invalid username or password")
            || low.contains("auth-failed") || low.contains("incorrect username or password")
        {
            self.login_failed = true;
            return vec![];
        }
        if low.contains("hip") && (low.contains("fail") || low.contains("denied") || low.contains("reject") || low.contains("not compliant")) {
            return vec![Action::Fail { kind: "hip", message: format!("Cihaz uyumluluk kontrolü (HIP) reddedildi: {l}"), pin: None }];
        }
        if low.starts_with("failed to complete authentication") || low.contains("failed to obtain webvpn cookie") {
            return vec![Action::Fail {
                kind: if self.login_failed { "auth" } else if self.pin.is_some() { "cert" } else { "auth" },
                message: if self.login_failed { "Kullanıcı adı veya şifre hatalı".into() } else { format!("Kimlik doğrulama tamamlanamadı: {l}") },
                pin: self.pin.clone(),
            }];
        }
        if low.contains("certificate") && (low.contains("verify failed") || low.contains("error in the certificate")) && self.pin.is_some() {
            // Wait for the explicit failure line; the pin is already captured.
            return vec![];
        }
        if low.starts_with("failed to connect to host") || low.starts_with("failed to open https connection") || low.contains("name or service not known") || low.contains("could not resolve") {
            return vec![Action::Fail { kind: "network", message: format!("Sunucuya ulaşılamadı: {l}"), pin: None }];
        }
        if low.contains("saml") && (low.contains("not supported") || low.contains("external browser") || low.contains("prelogin")) && low.contains("saml") {
            self.last_info = l.to_string();
        }
        if low.contains("tun device") && (low.contains("fail") || low.contains("cannot") || low.contains("permission") || low.contains("not permitted")) {
            return vec![Action::Fail { kind: "denied", message: "TUN aygıtı açılamadı (yönetici izni gerekli)".into(), pin: None }];
        }
        if low.starts_with("reconnect") || low.contains("attempting to reconnect") || low.contains("dpd detected dead peer") {
            return vec![Action::Reconnecting];
        }
        if !l.starts_with("POST ") && !l.starts_with("GET ") && !l.starts_with("Connected to ") && !l.starts_with("SSL negotiation") {
            self.last_info = l.to_string();
        }
        vec![]
    }

    fn prompt(&mut self, tail: &str) -> Vec<Action> {
        let t = tail.trim();
        if !(t.ends_with(':') || t.ends_with("]:")) {
            return vec![];
        }
        let low = t.to_ascii_lowercase();
        if low.contains("enter 'yes' to accept") {
            return vec![
                Action::Send("no".into()),
                Action::Fail { kind: "cert", message: "Sunucu sertifikası doğrulanamadı".into(), pin: self.pin.clone() },
            ];
        }
        // GROUP: [a|b]:  /  GATEWAY: [a|b]:
        if let (Some(a), Some(b)) = (t.find('['), t.rfind(']')) {
            if a < b && (low.starts_with("group") || low.starts_with("gateway") || low.contains("realm")) {
                let options: Vec<String> = t[a + 1..b].split('|').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if let Some(g) = self.group.as_ref().filter(|g| options.iter().any(|o| o == *g)) {
                    return vec![Action::Send(g.clone())];
                }
                let kind = if low.starts_with("gateway") { "gateway" } else { "group" };
                return vec![Action::Ask { kind, message: t[..a].trim().trim_end_matches(':').to_string(), options }];
            }
        }
        if low.starts_with("username:") || low.starts_with("user name:") {
            return if self.user.is_empty() {
                vec![Action::Ask { kind: "text", message: "Kullanıcı adı".into(), options: vec![] }]
            } else {
                vec![Action::Send(self.user.clone())]
            };
        }
        if low.starts_with("password:") || low.starts_with("passwd:") {
            self.passwords_sent += 1;
            if self.login_failed {
                self.login_failed = false;
                self.password = None;
                return vec![Action::Ask { kind: "password", message: "Şifre reddedildi — tekrar gir".into(), options: vec![] }];
            }
            if self.passwords_sent == 1 {
                if let Some(p) = self.password.clone() {
                    return vec![Action::Send(p)];
                }
                return vec![Action::Ask { kind: "password", message: "VPN şifresi".into(), options: vec![] }];
            }
            // A second password prompt without a failure = second factor.
            return vec![Action::Ask { kind: "mfa", message: self.last_info.clone(), options: vec![] }];
        }
        if low.contains("response")
            || low.contains("passcode")
            || low.contains("token")
            || low.contains("otp")
            || low.contains("verification")
            || low.contains("code")
            || low.contains("challenge")
            || low.contains("pin:")
        {
            return vec![Action::Ask { kind: "mfa", message: format!("{} {}", self.last_info, t.trim_end_matches(':')).trim().to_string(), options: vec![] }];
        }
        // Unknown prompt: let the user answer it.
        vec![Action::Ask { kind: "text", message: t.trim_end_matches(':').to_string(), options: vec![] }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ConnectReq {
        serde_json::from_value(serde_json::json!({
            "id": "p", "name": "t", "protocol": "openconnect", "ocProtocol": "gp",
            "server": "vpn.example.com", "user": "me", "group": "EU", "mode": "app", "password": "pw"
        }))
        .unwrap()
    }

    #[test]
    fn parses_flow() {
        let mut p = OcParser::new(&req());
        assert!(matches!(&p.prompt("Username:")[..], [Action::Send(u)] if u == "me"));
        assert!(matches!(&p.prompt("Password:")[..], [Action::Send(x)] if x == "pw"));
        assert!(matches!(&p.prompt("GATEWAY: [US|EU]:")[..], [Action::Send(g)] if g == "EU"));
        assert!(matches!(&p.prompt("GROUP: [a|b]:")[..], [Action::Ask { kind: "group", options, .. }] if options.len() == 2));
        assert!(matches!(&p.prompt("Password:")[..], [Action::Ask { kind: "mfa", .. }]));
        assert!(matches!(&p.line("Configured as 10.77.0.23, with SSL connected")[..], [Action::Connected { ip }] if ip == "10.77.0.23"));
        assert!(matches!(&p.line("RX: 10 packets (564 B); TX: 6 packets (248 B)")[..], [Action::Stats { rx: 564, tx: 248 }]));
        p.line("    --servercert pin-sha256:abc=");
        assert!(matches!(&p.prompt("Enter 'yes' to accept, 'no' to abort; anything else to view:")[..], [Action::Send(_), Action::Fail { kind: "cert", pin: Some(_), .. }]));
        p.line("Login failed.");
        assert!(matches!(&p.line("Failed to complete authentication")[..], [Action::Fail { kind: "auth", .. }]));
        assert!(matches!(&p.prompt("Please enter your password.")[..], []));
    }
}

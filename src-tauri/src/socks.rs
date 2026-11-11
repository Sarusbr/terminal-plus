//! Minimal SOCKS5 client (no auth, CONNECT by hostname) — SSH/SFTP through an app-mode VPN.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn connect(proxy: &str, host: &str, port: u16) -> Result<TcpStream, String> {
    let mut s = TcpStream::connect(proxy).await.map_err(|e| format!("SOCKS {proxy}: {e}"))?;
    s.write_all(&[5, 1, 0]).await.map_err(|e| e.to_string())?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.map_err(|e| e.to_string())?;
    if r != [5, 0] {
        return Err("SOCKS: proxy refused no-auth".into());
    }
    let h = host.as_bytes();
    if h.len() > 255 {
        return Err("SOCKS: host too long".into());
    }
    let mut req = vec![5, 1, 0, 3, h.len() as u8];
    req.extend_from_slice(h);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.map_err(|e| e.to_string())?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.map_err(|e| e.to_string())?;
    if head[1] != 0 {
        let why = match head[1] {
            2 => "not allowed", 3 => "network unreachable", 4 => "host unreachable",
            5 => "connection refused", 6 => "TTL expired", _ => "failure",
        };
        return Err(format!("VPN üzerinden bağlanılamadı ({why})"));
    }
    // Skip the bound address.
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).await.map_err(|e| e.to_string())?;
            l[0] as usize
        }
        _ => return Err("SOCKS: bad reply".into()),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest).await.map_err(|e| e.to_string())?;
    Ok(s)
}

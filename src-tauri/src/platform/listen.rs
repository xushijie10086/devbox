//! Linux / Windows 上枚举监听端口：用跨平台的 `listeners` 库，
//! 不依赖 lsof（精简的 Linux 发行版常常没装，Windows 根本没有）。

use crate::models::PortInfo;
use listeners::{Protocol, SocketState};
use std::net::SocketAddr;

/// 地址展示：监听所有网卡的写成 `*:端口`，和 macOS 上 lsof 的显示一致
fn format_addr(sock: &SocketAddr) -> String {
    if sock.ip().is_unspecified() {
        format!("*:{}", sock.port())
    } else if sock.is_ipv6() {
        format!("[{}]:{}", sock.ip(), sock.port())
    } else {
        sock.to_string()
    }
}

/// 所有处于 LISTEN 状态的 TCP 端口，按端口排序并去重
pub fn list() -> Result<Vec<PortInfo>, String> {
    let all = listeners::get_all().map_err(|e| format!("读取监听端口失败: {e}"))?;
    let mut out: Vec<PortInfo> = all
        .into_iter()
        .filter(|l| l.protocol == Protocol::TCP && l.state == SocketState::Listen)
        .map(|l| PortInfo {
            port: l.socket.port(),
            pid: l.process.pid,
            process: l.process.name,
            protocol: "TCP".into(),
            address: format_addr(&l.socket),
            project: None,
            group: None,
        })
        .collect();
    out.sort_by_key(|p| p.port);
    out.dedup_by_key(|p| (p.port, p.pid));
    Ok(out)
}

pub fn pids_on_port(port: u16) -> Vec<u32> {
    list().map(|v| v.into_iter().filter(|p| p.port == port).map(|p| p.pid).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn address_formatting_matches_lsof_style() {
        assert_eq!(format_addr(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3000)), "*:3000");
        assert_eq!(format_addr(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 3000)), "*:3000");
        assert_eq!(format_addr(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)), "127.0.0.1:8080");
        assert_eq!(format_addr(&SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080)), "[::1]:8080");
    }

    #[test]
    fn our_own_listener_is_found_with_its_pid() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let found = list().unwrap();
        let mine = found.iter().find(|p| p.port == port).unwrap_or_else(|| panic!("没找到 {port}：{found:?}"));
        assert_eq!(mine.pid, std::process::id());
        assert_eq!(mine.address, format!("127.0.0.1:{port}"));
        assert!(pids_on_port(port).contains(&std::process::id()));
        drop(l);
    }
}

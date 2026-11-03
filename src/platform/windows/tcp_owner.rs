//! Who owns a TCP connection or a listening port: the machine's TCP table (`GetExtendedTcpTable`), the
//! program name of a pid, and the same-user check on the peer of a loopback connection.

use std::net::{Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    TCP_TABLE_CLASS, TCP_TABLE_OWNER_PID_CONNECTIONS, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use crate::api_peer::{judge, owner, port_from_table, v4_from_table, Row, NO_OWNER};
use crate::platform::windows::ipc;

/// The raw table for one family, in u64 cells so the rows are aligned. The table can grow
/// between the size call and the data call, so it retries a few times.
fn table(family: u32, class: TCP_TABLE_CLASS) -> Result<(Vec<u64>, usize), String> {
    let mut size = 0u32;
    for _ in 0..8 {
        let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let mut len = (buf.len() * 8) as u32;
        let r = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut len, 0, family, class, 0) };
        match r {
            NO_ERROR => return Ok((buf, len as usize)),
            // Room for a few more rows that may appear before the next call.
            ERROR_INSUFFICIENT_BUFFER => size = len + 4096,
            e => return Err(format!("GetExtendedTcpTable failed: {}", std::io::Error::from_raw_os_error(e as i32))),
        }
    }
    Err("GetExtendedTcpTable failed: the table kept growing".into())
}

/// The rows of a table whose header is `dwNumEntries` followed by rows starting at `first`.
unsafe fn rows_of<R: Copy>(buf: &[u64], len: usize, count: u32, first: *const R) -> Result<&[R], String> {
    let start = first as usize - buf.as_ptr() as usize;
    let need = start + count as usize * size_of::<R>();
    if need > len || need > buf.len() * 8 {
        return Err("GetExtendedTcpTable failed: the table is shorter than its row count".into());
    }
    Ok(unsafe { std::slice::from_raw_parts(first, count as usize) })
}

fn v4_rows(class: TCP_TABLE_CLASS) -> Result<Vec<Row>, String> {
    let (buf, len) = table(AF_INET as u32, class)?;
    let t = buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID;
    let rows: &[MIB_TCPROW_OWNER_PID] = unsafe { rows_of(&buf, len, (*t).dwNumEntries, (*t).table.as_ptr())? };
    Ok(rows
        .iter()
        .map(|r| Row {
            local: SocketAddr::V4(SocketAddrV4::new(v4_from_table(r.dwLocalAddr), port_from_table(r.dwLocalPort))),
            remote: SocketAddr::V4(SocketAddrV4::new(v4_from_table(r.dwRemoteAddr), port_from_table(r.dwRemotePort))),
            pid: r.dwOwningPid,
        })
        .collect())
}

fn v6_rows(class: TCP_TABLE_CLASS) -> Result<Vec<Row>, String> {
    let (buf, len) = table(AF_INET6 as u32, class)?;
    let t = buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID;
    let rows: &[MIB_TCP6ROW_OWNER_PID] = unsafe { rows_of(&buf, len, (*t).dwNumEntries, (*t).table.as_ptr())? };
    Ok(rows
        .iter()
        .map(|r| Row {
            local: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(r.ucLocalAddr), port_from_table(r.dwLocalPort), 0, r.dwLocalScopeId)),
            remote: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(r.ucRemoteAddr), port_from_table(r.dwRemotePort), 0, r.dwRemoteScopeId)),
            pid: r.dwOwningPid,
        })
        .collect())
}

/// Every TCP connection on the machine, IPv4 then IPv6.
pub fn tcp_rows() -> Result<Vec<Row>, String> {
    let mut rows = v4_rows(TCP_TABLE_OWNER_PID_CONNECTIONS)?;
    rows.extend(v6_rows(TCP_TABLE_OWNER_PID_CONNECTIONS)?);
    Ok(rows)
}

/// Every listening TCP socket on the machine, IPv4 then IPv6 (a machine without IPv6 lists
/// only IPv4).
pub fn listen_rows() -> Result<Vec<Row>, String> {
    let mut rows = v4_rows(TCP_TABLE_OWNER_PID_LISTENER)?;
    rows.extend(v6_rows(TCP_TABLE_OWNER_PID_LISTENER).unwrap_or_default());
    Ok(rows)
}

/// The file name of process `pid`'s program ("python.exe"), when it can be read.
pub fn process_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
    unsafe { CloseHandle(h) };
    let path = ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))?;
    path.rsplit(['\\', '/']).next().map(str::to_string)
}

/// The pid of the process that owns the other end of the connection that reached `local`
/// from `peer`, when its token passes `judge`; otherwise the reason it is refused. Every
/// failure refuses.
pub fn check(peer: SocketAddr, local: SocketAddr) -> Result<u32, String> {
    // Only the peer's family: a machine without IPv6 still checks IPv4 peers.
    let rows = if peer.is_ipv4() { v4_rows(TCP_TABLE_OWNER_PID_CONNECTIONS)? } else { v6_rows(TCP_TABLE_OWNER_PID_CONNECTIONS)? };
    let pid = owner(&rows, peer, local).ok_or_else(|| NO_OWNER.to_string())?;
    // Each token is opened once; any query that fails refuses.
    judge(&ipc::process_token_facts(pid)?, &ipc::own_token_facts()?)?;
    Ok(pid)
}

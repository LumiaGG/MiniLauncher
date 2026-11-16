use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    DeleteTcb,
    Unknown,
}

impl TcpState {
    pub fn from_raw(state: u32) -> Self {
        match state {
            1 => Self::Closed,
            2 => Self::Listen,
            3 => Self::SynSent,
            4 => Self::SynReceived,
            5 => Self::Established,
            6 => Self::FinWait1,
            7 => Self::FinWait2,
            8 => Self::CloseWait,
            9 => Self::Closing,
            10 => Self::LastAck,
            11 => Self::TimeWait,
            12 => Self::DeleteTcb,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "CLOSED",
            Self::Listen => "LISTENING",
            Self::SynSent => "SYN_SENT",
            Self::SynReceived => "SYN_RCVD",
            Self::Established => "ESTABLISHED",
            Self::FinWait1 => "FIN_WAIT_1",
            Self::FinWait2 => "FIN_WAIT_2",
            Self::CloseWait => "CLOSE_WAIT",
            Self::Closing => "CLOSING",
            Self::LastAck => "LAST_ACK",
            Self::TimeWait => "TIME_WAIT",
            Self::DeleteTcb => "DELETE_TCB",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortBinding {
    pub protocol: Protocol,
    pub local_ip: IpAddr,
    pub local_port: u16,
    pub remote_ip: Option<IpAddr>,
    pub remote_port: Option<u16>,
    pub state: Option<TcpState>,
    pub pid: u32,
}

/// Fetches all active TCP (IPv4 + IPv6) and UDP (IPv4 + IPv6) bindings on the system.
pub fn get_all_port_bindings() -> Vec<PortBinding> {
    let mut bindings = Vec::new();
    append_tcp4(&mut bindings);
    append_tcp6(&mut bindings);
    append_udp4(&mut bindings);
    append_udp6(&mut bindings);
    bindings
}

/// Builds a PID -> Vec<PortBinding> mapping for quick lookup.
pub fn get_bindings_by_pid(bindings: &[PortBinding]) -> HashMap<u32, Vec<PortBinding>> {
    let mut map: HashMap<u32, Vec<PortBinding>> = HashMap::new();
    for b in bindings {
        map.entry(b.pid).or_default().push(b.clone());
    }
    map
}

fn append_tcp4(out: &mut Vec<PortBinding>) {
    let mut size: u32 = 0;
    unsafe {
        let ret = GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        if ret != ERROR_INSUFFICIENT_BUFFER && ret != NO_ERROR {
            return;
        }
        let mut buf = vec![0u8; size as usize];
        if GetExtendedTcpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        ) != NO_ERROR
        {
            return;
        }

        let table = &*buf.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        for row in rows {
            let local_ip = IpAddr::V4(Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()));
            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);
            let remote_ip = (row.dwRemoteAddr != 0)
                .then(|| IpAddr::V4(Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes())));
            let remote_port = (row.dwRemotePort != 0)
                .then(|| u16::from_be((row.dwRemotePort & 0xFFFF) as u16));
            let state = Some(TcpState::from_raw(row.dwState));

            out.push(PortBinding {
                protocol: Protocol::Tcp,
                local_ip,
                local_port,
                remote_ip,
                remote_port,
                state,
                pid: row.dwOwningPid,
            });
        }
    }
}

fn append_tcp6(out: &mut Vec<PortBinding>) {
    let mut size: u32 = 0;
    unsafe {
        let ret = GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET6 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        if ret != ERROR_INSUFFICIENT_BUFFER && ret != NO_ERROR {
            return;
        }
        let mut buf = vec![0u8; size as usize];
        if GetExtendedTcpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET6 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        ) != NO_ERROR
        {
            return;
        }

        let table = &*buf.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        for row in rows {
            let local_ip = IpAddr::V6(Ipv6Addr::from(row.ucLocalAddr));
            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);
            let remote_ip = (row.ucRemoteAddr != [0; 16])
                .then(|| IpAddr::V6(Ipv6Addr::from(row.ucRemoteAddr)));
            let remote_port = (row.dwRemotePort != 0)
                .then(|| u16::from_be((row.dwRemotePort & 0xFFFF) as u16));
            let state = Some(TcpState::from_raw(row.dwState));

            out.push(PortBinding {
                protocol: Protocol::Tcp,
                local_ip,
                local_port,
                remote_ip,
                remote_port,
                state,
                pid: row.dwOwningPid,
            });
        }
    }
}

fn append_udp4(out: &mut Vec<PortBinding>) {
    let mut size: u32 = 0;
    unsafe {
        let ret = GetExtendedUdpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET as u32,
            UDP_TABLE_OWNER_PID,
            0,
        );
        if ret != ERROR_INSUFFICIENT_BUFFER && ret != NO_ERROR {
            return;
        }
        let mut buf = vec![0u8; size as usize];
        if GetExtendedUdpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET as u32,
            UDP_TABLE_OWNER_PID,
            0,
        ) != NO_ERROR
        {
            return;
        }

        let table = &*buf.as_ptr().cast::<MIB_UDPTABLE_OWNER_PID>();
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        for row in rows {
            let local_ip = IpAddr::V4(Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()));
            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);

            out.push(PortBinding {
                protocol: Protocol::Udp,
                local_ip,
                local_port,
                remote_ip: None,
                remote_port: None,
                state: None,
                pid: row.dwOwningPid,
            });
        }
    }
}

fn append_udp6(out: &mut Vec<PortBinding>) {
    let mut size: u32 = 0;
    unsafe {
        let ret = GetExtendedUdpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET6 as u32,
            UDP_TABLE_OWNER_PID,
            0,
        );
        if ret != ERROR_INSUFFICIENT_BUFFER && ret != NO_ERROR {
            return;
        }
        let mut buf = vec![0u8; size as usize];
        if GetExtendedUdpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET6 as u32,
            UDP_TABLE_OWNER_PID,
            0,
        ) != NO_ERROR
        {
            return;
        }

        let table = &*buf.as_ptr().cast::<MIB_UDP6TABLE_OWNER_PID>();
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        for row in rows {
            let local_ip = IpAddr::V6(Ipv6Addr::from(row.ucLocalAddr));
            let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);

            out.push(PortBinding {
                protocol: Protocol::Udp,
                local_ip,
                local_port,
                remote_ip: None,
                remote_port: None,
                state: None,
                pid: row.dwOwningPid,
            });
        }
    }
}

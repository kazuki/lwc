use std::net::{UdpSocket, SocketAddr, ToSocketAddrs};
use std::io::{Result, Error, ErrorKind};
use std::os::raw::{c_short, c_int, c_uint};
use std::mem::transmute;

#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

/// ポーリングで確認するソケットの状態を定義
#[derive(Debug, PartialEq)]
#[repr(C)]
pub enum SocketEventType {
    /// POLLIN(Linux) or POLLRDNORM(Windows)
    Read = 0x01,

    /// POLLOUT(LINUX) or POLLWRNORM(Windows)
    Write = 0x04,

    /// POLLERR(Linux) or POLLERR(Windows)
    Error = 0x08,

    /// POLLHUP(Linux) or POLLHUP(Windows)
    HangUp = 0x10,

    /// POLLPRI(Linux) or POLLRDBAND(Windows)
    Priority = 0x02,
}

/// UdpSocketのサプリメント
pub trait DatagramSocket {
    /// See [std::net::UdpSocket::recv_from](http://doc.rust-lang.org/stable/std/net/struct.UdpSocket.html#method.recv_from)
    fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)>;

    /// See [std::net::UdpSocket::send_to](http://doc.rust-lang.org/stable/std/net/struct.UdpSocket.html#method.send_to)
    fn send_to<A>(&self, buf: &[u8], addr: A) -> Result<usize>
        where A: ToSocketAddrs;

    /// See [std::net::UdpSocket::local_addr](http://doc.rust-lang.org/stable/std/net/struct.UdpSocket.html#method.local_addr)
    fn local_addr(&self) -> Result<SocketAddr>;

    /// ソケットをブロッキングモードまたはノンブロッキングモードに設定します．
    /// Sets a value that indicates whether the Socket is in blocking mode.
    fn set_blocking(&self, blocking: bool) -> Result<()>;

    /// ソケットの状態を確認します．
    /// Determines the status of the Socket.
    fn poll(&self, millisec: i32, events: SocketEventType) -> Result<SocketEventType>;

    /// ノンブロッキングモードにおいて，recv_from/send_toのエラーが
    /// データ未着・バッファ不足によるものか確認します．
    fn is_would_block_err(err: &Error) -> bool;
}

impl DatagramSocket for UdpSocket {
    fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        self.recv_from(buf)
    }

    fn send_to<A>(&self, buf: &[u8], addr: A) -> Result<usize> where A: ToSocketAddrs {
        self.send_to(buf, addr)
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        self.local_addr()
    }

    #[cfg(target_os = "linux")]
    fn set_blocking(&self, blocking: bool) -> Result<()> {
        let fd = self.as_raw_fd();
        let v = match blocking {
            true => 0,
            false => 1,
        };
        unsafe {
            if ioctl(fd, 0x5421 /*FIONBIO*/, &v) == 0 {
                Ok(())
            } else {
                Err(Error::new(ErrorKind::Other, "ioctl failed"))
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn poll(&self, millisec: i32, events: SocketEventType) -> Result<SocketEventType> {
        let fd = self.as_raw_fd();
        let v: c_short = events as c_short;
        let mut pfd = PollFD {
            fd: fd,
            events: v,
            revents: 0,
        };
        let ret = unsafe { poll(&mut pfd, 1, millisec) };
        match ret {
            0 => Err(Error::new(ErrorKind::TimedOut, "")),
            1 => Ok(unsafe { transmute(pfd.revents as c_uint) }),
            _ => Err(Error::last_os_error()),
        }
    }

    #[cfg(target_os = "linux")]
    fn is_would_block_err(err: &Error) -> bool {
        match err.raw_os_error() {
            Some(errno) if errno == 11 => true, // EAGAIN or EWOULDBLOCK
            _ => false,
        }
    }
}

#[cfg(target_os = "linux")]
#[link(name = "c")]
extern {
    fn ioctl(fd: c_int, request: c_int, value: &c_int) -> c_int;
    fn poll(fds: *mut PollFD, nfds: c_int, timeout: c_int) -> c_int;
    //fn setsockopt(sockfd: c_int, level: c_int, optname: c_int, optval: c_void, optlen: c_int) -> c_int;
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct PollFD {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

use byteorder::{BigEndian, ReadBytesExt};
use bytes::BufMut;
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    io::{Cursor, Error, ErrorKind, Result},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Address {
    SocketAddress(SocketAddr),
    DomainAddress(String, u16),
}

impl Address {
    const ATYP_IPV4: u8 = 0x01;
    const ATYP_DOMAIN: u8 = 0x03;
    const ATYP_IPV6: u8 = 0x04;

    pub fn unspecified() -> Self {
        Address::SocketAddress(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
    }

    pub async fn addr_data_from_stream<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Vec<u8>> {
        let mut addr_data = Vec::new();
        let atyp = stream.read_u8().await?;
        addr_data.push(atyp);
        match atyp {
            Self::ATYP_IPV4 => {
                let mut buf = [0; 6];
                stream.read_exact(&mut buf).await?;
                addr_data.extend_from_slice(&buf);
            }
            Self::ATYP_DOMAIN => {
                let len = stream.read_u8().await? as usize;
                let mut buf = vec![0; len + 2];
                stream.read_exact(&mut buf).await?;

                addr_data.push(len as u8);
                addr_data.extend_from_slice(&buf);
            }
            Self::ATYP_IPV6 => {
                let mut buf = [0; 18];
                stream.read_exact(&mut buf).await?;
                addr_data.extend_from_slice(&buf);
            }
            atyp => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    format!("Unsupported address type {0:#x}", atyp),
                ));
            }
        }
        Ok(addr_data)
    }

    pub fn from_data(data: &[u8]) -> Result<Self> {
        let mut rdr = Cursor::new(data);
        let atyp = ReadBytesExt::read_u8(&mut rdr)?;
        match atyp {
            Self::ATYP_IPV4 => {
                let addr = Ipv4Addr::new(
                    ReadBytesExt::read_u8(&mut rdr)?,
                    ReadBytesExt::read_u8(&mut rdr)?,
                    ReadBytesExt::read_u8(&mut rdr)?,
                    ReadBytesExt::read_u8(&mut rdr)?,
                );

                let port = ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?;

                Ok(Self::SocketAddress(SocketAddr::from((addr, port))))
            }
            Self::ATYP_DOMAIN => {
                let len = ReadBytesExt::read_u8(&mut rdr)? as usize;
                let mut buf = data[2..2 + len + 2].to_vec();

                let port = ReadBytesExt::read_u16::<BigEndian>(&mut &buf[len..])?;
                buf.truncate(len);

                let addr = match String::from_utf8(buf) {
                    Ok(addr) => addr,
                    Err(err) => {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            format!("Invalid address encoding: {err}"),
                        ))
                    }
                };

                Ok(Self::DomainAddress(addr, port))
            }
            Self::ATYP_IPV6 => {
                let addr = Ipv6Addr::new(
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                    ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?,
                );
                let port = ReadBytesExt::read_u16::<BigEndian>(&mut rdr)?;
                Ok(Self::SocketAddress(SocketAddr::from((addr, port))))
            }
            atyp => Err(Error::new(
                ErrorKind::Unsupported,
                format!("Unsupported address type {0:#x}", atyp),
            )),
        }
    }

    pub async fn from_stream<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Self> {
        let addr_data = Self::addr_data_from_stream(stream).await?;
        Self::from_data(&addr_data)
    }

    pub fn write_to_buf<B: BufMut>(&self, buf: &mut B) {
        match self {
            Self::SocketAddress(addr) => match addr {
                SocketAddr::V4(addr) => {
                    buf.put_u8(Self::ATYP_IPV4);
                    buf.put_slice(&addr.ip().octets());
                    buf.put_u16(addr.port());
                }
                SocketAddr::V6(addr) => {
                    buf.put_u8(Self::ATYP_IPV6);
                    for seg in addr.ip().segments() {
                        buf.put_u16(seg);
                    }
                    buf.put_u16(addr.port());
                }
            },
            Self::DomainAddress(addr, port) => {
                let addr = addr.as_bytes();
                buf.put_u8(Self::ATYP_DOMAIN);
                buf.put_u8(addr.len() as u8);
                buf.put_slice(addr);
                buf.put_u16(*port);
            }
        }
    }

    pub fn serialized_len(&self) -> usize {
        1 + match self {
            Address::SocketAddress(addr) => match addr {
                SocketAddr::V4(_) => 6,
                SocketAddr::V6(_) => 18,
            },
            Address::DomainAddress(addr, _) => 1 + addr.len() + 2,
        }
    }

    pub const fn max_serialized_len() -> usize {
        1 + 1 + u8::MAX as usize + 2
    }
}

impl Display for Address {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Address::DomainAddress(hostname, port) => write!(f, "{hostname}:{port}"),
            Address::SocketAddress(socket_addr) => write!(f, "{socket_addr}"),
        }
    }
}

impl TryFrom<Address> for SocketAddr {
    type Error = Error;

    fn try_from(address: Address) -> std::result::Result<Self, Self::Error> {
        match address {
            Address::SocketAddress(addr) => Ok(addr),
            Address::DomainAddress(addr, port) => {
                if let Ok(addr) = addr.parse::<Ipv4Addr>() {
                    Ok(SocketAddr::from((addr, port)))
                } else if let Ok(addr) = addr.parse::<Ipv6Addr>() {
                    Ok(SocketAddr::from((addr, port)))
                } else {
                    Err(Self::Error::new(
                        ErrorKind::Unsupported,
                        format!("domain address {addr} is not supported"),
                    ))
                }
            }
        }
    }
}

impl From<Address> for Vec<u8> {
    fn from(addr: Address) -> Self {
        let mut buf = Vec::with_capacity(addr.serialized_len());
        addr.write_to_buf(&mut buf);
        buf
    }
}

impl TryFrom<Vec<u8>> for Address {
    type Error = Error;

    fn try_from(data: Vec<u8>) -> std::result::Result<Self, Self::Error> {
        Self::from_data(&data)
    }
}

impl From<SocketAddr> for Address {
    fn from(addr: SocketAddr) -> Self {
        Address::SocketAddress(addr)
    }
}

impl From<(Ipv4Addr, u16)> for Address {
    fn from((addr, port): (Ipv4Addr, u16)) -> Self {
        Address::SocketAddress(SocketAddr::from((addr, port)))
    }
}

impl From<(Ipv6Addr, u16)> for Address {
    fn from((addr, port): (Ipv6Addr, u16)) -> Self {
        Address::SocketAddress(SocketAddr::from((addr, port)))
    }
}

impl From<(String, u16)> for Address {
    fn from((addr, port): (String, u16)) -> Self {
        Address::DomainAddress(addr, port)
    }
}

impl From<(&str, u16)> for Address {
    fn from((addr, port): (&str, u16)) -> Self {
        Address::DomainAddress(addr.to_owned(), port)
    }
}

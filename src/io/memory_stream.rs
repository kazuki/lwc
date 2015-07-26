use std;
use std::convert::AsRef;
use std::mem::{forget, size_of};
use std::io::{Read, Result, Write, Seek, SeekFrom};

/// Creates a stream that backing store is memory
pub struct MemoryStream {
    buf: Vec<u8>,
    pos: usize,
}

impl MemoryStream {
    pub fn new() -> MemoryStream {
        MemoryStream {
            buf: Vec::new(),
            pos: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> MemoryStream {
        MemoryStream {
            buf: Vec::with_capacity(capacity),
            pos: 0,
        }
    }

    pub fn with_buffer<T>(mut buf: Box<[T]>) -> MemoryStream {
        unsafe {
            let ptr = buf.as_mut_ptr() as *mut u8;
            let len = buf.len() * size_of::<T>();
            forget(buf);
            MemoryStream {
                buf: Vec::from_raw_parts(ptr, len, len),
                pos: 0,
            }
        }
    }

    pub fn as_vec(&self) -> &Vec<u8> {
        &self.buf
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }
}

impl Read for MemoryStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let size = std::cmp::max(0, std::cmp::min(buf.len(),
                                                  self.buf.len() - self.pos));
        if size > 0 {
            std::slice::bytes::copy_memory(
                &self.buf[self.pos..self.pos + size],
                buf);
            self.pos += size;
        }
        Ok(size)
    }
}

impl Write for MemoryStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let end = self.pos + buf.len();
        if end > self.buf.len() {
            let cur_len = self.buf.len();
            self.buf.reserve(end - cur_len);
            unsafe {
                self.buf.set_len(end);
            }
        }
        std::slice::bytes::copy_memory(
            buf,
            &mut self.buf[self.pos..]);
        self.pos = end;
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Seek for MemoryStream {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(x) => x,
            SeekFrom::End(x) => (self.buf.len() as i64 + x) as u64,
            SeekFrom::Current(x) => (self.pos as i64 + x) as u64,
        } as usize;
        if new_pos > self.buf.len() {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "out of range"))
        } else {
            self.pos = new_pos;
            Ok(new_pos as u64)
        }
    }
}

impl AsRef<[u8]> for MemoryStream {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

#[test]
fn write_test() {
    let mut ms = MemoryStream::new();
    ms.write(&[0u8, 1, 2, 3, 4, 5]).unwrap();
    assert_eq!(ms.as_vec().len(), 6);
    assert_eq!(ms.as_vec(), &[0u8, 1, 2, 3, 4, 5]);

    ms = MemoryStream::with_capacity(1);
    ms.write(&[0u8, 1, 2, 3, 4, 5]).unwrap();
    assert_eq!(ms.as_vec().len(), 6);
    assert_eq!(ms.as_vec(), &[0u8, 1, 2, 3, 4, 5]);
}

#[test]
fn seek_test() {
    let mut ms = MemoryStream::new();
    ms.write(&[0u8, 1, 2, 3, 4, 5]).unwrap();
    assert_eq!(ms.as_vec().len(), 6);
    assert_eq!(ms.as_vec(), &[0u8, 1, 2, 3, 4, 5]);

    assert_eq!(ms.seek(SeekFrom::Start(0)).unwrap(), 0);
    assert!(match ms.seek(SeekFrom::Current(-1)) {
        Err(_) => true,
        _ => false
    });
    ms.write(&[255u8]).unwrap();
    assert!(match ms.seek(SeekFrom::End(1)) {
        Err(_) => true,
        _ => false
    });
    assert_eq!(ms.seek(SeekFrom::Start(6)).unwrap(), 6);
    ms.write(&[6u8]).unwrap();
    assert_eq!(ms.as_vec(), &[255u8, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn read_test() {
    let mut ms = MemoryStream::new();
    ms.write(&[0u8, 1, 2, 3, 4, 5]).unwrap();
    ms.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    buf.resize(64, 0u8);
    assert_eq!(ms.read(&mut buf).unwrap(), 6);
    assert_eq!(&buf[0..6], &[0u8, 1, 2, 3, 4, 5]);

    ms = MemoryStream::with_buffer(Box::new([0u8, 1, 2, 3, 4, 5]));
    assert_eq!(ms.read(&mut buf).unwrap(), 6);
    assert_eq!(&buf[0..6], &[0u8, 1, 2, 3, 4, 5]);
    ms.seek(SeekFrom::End(0)).unwrap();
    ms.write(&[6, 7, 8, 9]).unwrap();
    assert_eq!(ms.as_vec(), &[0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

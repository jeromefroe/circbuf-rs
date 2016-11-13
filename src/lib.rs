// MIT License

// Copyright (c) 2016 Jerome Froelich

// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:

// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! An implementation of a growable circular buffer of bytes. The `CircBuf` struct
//! manages a buffer of bytes allocated on the heap. The buffer can be grown when needed
//! and can return slices into its internal buffer that can be used for both normal IO
//! (e.g. `read` and `write`) as well as vector IO (`readv` and `writev`).
//!
//! ## Example
//!
//! Below is a simple example of a server which makes use of a `CircBuf` to read messages
//! from a client. It uses the `vecio` crate to call `readv` and `writev` on the socket.
//! Messages are seperated by a vertical bar `|` and the server returns to the client
//! the number of bytes in each message it receives.
//!
//! ``` rust,no_run
//! extern crate vecio;
//! extern crate circbuf;
//!
//! use std::thread;
//! use std::net::{TcpListener, TcpStream};
//! use std::io::Write;
//! use vecio::Rawv;
//! use circbuf::CircBuf;
//!
//! fn handle_client(mut stream: TcpStream) {
//!     let mut buf = CircBuf::new();
//!     let mut num_messages = 0; // number of messages from the client
//!     let mut num_bytes = 0; // number of bytes read since last '|'
//!
//!     loop {
//!         // grow the buffer if it is less than half full
//!         if buf.len() > buf.avail() {
//!             buf.grow().unwrap();
//!         }
//!
//!         let n;
//!         {
//!             n = match stream.readv(&buf.get_avail()) {
//!                 Ok(n) => {
//!                     if n == 0 {
//!                         // EOF
//!                         println!("client closed connection");
//!                         break;
//!                     }
//!                     n
//!                 }
//!                 Err(e) => panic!("got an error reading from a connection: {}", e),
//!             };
//!         }
//!
//!         println!("read {} bytes from the client", n);
//!
//!         // update write cursor
//!         buf.advance_write(n);
//!
//!         // parse request from client for messages seperated by '|'
//!         loop {
//!             match buf.find_from_index(b'|', num_bytes) {
//!                 Some(i) => {
//!                     // update read cursor past '|' and reset num_bytes since last '|'
//!                     buf.advance_read(i + 1);
//!                     num_bytes = 0;
//!                     num_messages += 1;
//!
//!                     let response = format!("Message {} contained {} bytes\n", num_messages, i - 1); // don't inclue '|' in num_bytes
//!                     match stream.write(&response.as_bytes()) {
//!                         Ok(n) => {
//!                             println!("wrote {} bytes to the client", n);
//!                         }
//!                         Err(e) => panic!("got an error writing to connection: {}", e),
//!                     }
//!                 }
//!                 None => break,
//!             }
//!         }
//!     }
//! }
//!
//! fn main() {
//!     let listener = TcpListener::bind("127.0.0.1:8888").unwrap();
//!     for stream in listener.incoming() {
//!         match stream {
//!             Ok(stream) => {
//!                 thread::spawn(move || handle_client(stream));
//!             }
//!             Err(e) => panic!("got an error accepting connection: {}", e),
//!         }
//!     }
//! }
//! ```

// need to uncomment this to run benchmarks
// #![feature(test)]
// extern crate test;
extern crate vecio;

use std::io;
use std::error;
use std::fmt;
use std::boxed::Box;
use std::slice::from_raw_parts;
use std::slice::from_raw_parts_mut;
use std::ptr::copy_nonoverlapping;

/// Default size of the circular buffer is 1 page
pub const DEFAULT_CAPACITY: usize = 4096;

/// Double the size of the buffer by default when growing it
pub const DEFAULT_SIZE_MULTIPLIER: usize = 2;

/// Circular Buffer Error
///
/// The errors that are used with `CircBuf`. Namely, that the buffer is empty, full,
/// or recieved a `usize` argument which could cause an overflow.
#[derive(Debug)]
pub enum CircBufError {
    BufEmpty,
    BufFull,
    Overflow,
}

impl fmt::Display for CircBufError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CircBufError::BufEmpty => write!(f, "CircBuf is full"),
            CircBufError::BufFull => write!(f, "CircBuf is empty"),
            CircBufError::Overflow => write!(f, "Value would overflow uszie"),
        }
    }
}

impl error::Error for CircBufError {
    fn description(&self) -> &str {
        match *self {
            CircBufError::BufEmpty => "CircBuf is full",
            CircBufError::BufFull => "CircBuf is empty",
            CircBufError::Overflow => "Value would overflow usize",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

/// Circular Buffer
///
/// A growable circular buffer for use with bytes.
#[derive(Debug)]
pub struct CircBuf {
    buf: Box<[u8]>,
    write_cursor: usize,
    read_cursor: usize,
}

// TODO: make note that we cant let write_cursor ever equal read_cursor, but we can do the opposite

impl CircBuf {
    /// Create a new CircBuf. The default size of the buffer is `DEFAULT_CAPACITY` bytes.
    pub fn new() -> Self {
        CircBuf {
            buf: Box::new([0; DEFAULT_CAPACITY]),
            write_cursor: 0,
            read_cursor: 0,
        }
    }

    /// Create a new CircBuf with a size of `cap` bytes. The capacity will be rounded up
    /// to the nearest power of two greater than or equal to `cap`. If the nearest power
    /// of two overflows `usize`, return `CircBufError::Overflow`, else return the buffer.
    pub fn with_capacity(cap: usize) -> Result<Self, CircBufError> {
        let capacity = match cap.checked_next_power_of_two() {
            Some(capacity) => capacity,
            None => return Err(CircBufError::Overflow),
        };

        Ok(CircBuf {
            buf: vec![0; capacity].into_boxed_slice(),
            write_cursor: 0,
            read_cursor: 0,
        })
    }

    /// Get the capacity of the buffer. This value is equal to one less than the length
    /// of the underlying buffer because we cannot let the write cursor to ever circle
    /// back to being equal with the read cursor.
    pub fn cap(&self) -> usize {
        self.buf.len() - 1
    }

    /// Get the number of bytes stored in the buffer.
    pub fn len(&self) -> usize {
        if self.write_cursor < self.read_cursor {
            return self.buf.len() - self.read_cursor + self.write_cursor;
        }

        self.write_cursor - self.read_cursor
    }

    /// Get the number of bytes available in the buffer.
    pub fn avail(&self) -> usize {
        self.cap() - self.len()
    }

    /// Get a `bool` indicating whether the buffer is empty or not.
    pub fn is_empty(&self) -> bool {
        return self.len() == 0;
    }

    /// Get a `bool` indicating whether the buffer is full or not.
    pub fn is_full(&self) -> bool {
        return self.avail() == 0;
    }

    /// Find the first occurence of `val` in the buffer starting from `index`. If `val`
    /// exists in the buffer return the index of the first occurence of `val` else return
    /// `None`.
    pub fn find_from_index(&self, val: u8, index: usize) -> Option<usize> {
        if index >= self.len() {
            return None;
        }

        if self.write_cursor < self.read_cursor {
            if self.read_cursor + index < self.buf.len() {
                for (i, b) in self.buf[self.read_cursor + index..].iter().enumerate() {
                    if *b == val {
                        return Some(i + index);
                    }
                }
            }

            for (i, b) in self.buf[..self.write_cursor].iter().enumerate() {
                if *b == val {
                    return Some(i + self.buf.len() - self.read_cursor);
                }
            }

            None
        } else {
            for (i, b) in self.buf[self.read_cursor + index..self.write_cursor].iter().enumerate() {
                if *b == val {
                    return Some(i + index);
                }
            }

            None
        }
    }

    /// Find the first occurence of `val` in the buffer. If `val` exists in the buffer
    /// return the index of the first occurence of `val` else return `None`. A convenience
    /// method for `find_from_index` with 0 as the index.
    pub fn find(&self, val: u8) -> Option<usize> {
        self.find_from_index(val, 0)
    }

    /// Get the next byte to be read from the buffer without removing it from it the buffer.
    /// Returns the byte if the buffer is not empty, else returns a `BufEmpty` error.
    pub fn peek(&self) -> Result<u8, CircBufError> {
        if self.len() == 0 {
            return Err(CircBufError::BufEmpty);
        }

        Ok(self.buf[self.read_cursor])
    }

    /// Get the next byte to be read from the buffer and remove it from it the buffer.
    /// Returns the byte if the buffer is not empty, else returns a `BufEmpty` error.
    pub fn get(&mut self) -> Result<u8, CircBufError> {
        if self.len() == 0 {
            return Err(CircBufError::BufEmpty);
        }

        let val = self.buf[self.read_cursor];
        self.advance_read(1);

        Ok(val)
    }

    /// Put `val` into the buffer. Returns a `BufFull` error if the buffer is full, else
    /// returns an empty tuple `()`.
    pub fn put(&mut self, val: u8) -> Result<(), CircBufError> {
        if self.avail() == 0 {
            return Err(CircBufError::BufFull);
        }

        self.buf[self.write_cursor] = val;
        self.advance_write(1);

        Ok(())
    }

    /// Advance the buffer's read cursor `num` bytes.
    pub fn advance_read(&mut self, num: usize) {
        self.read_cursor = (self.read_cursor + num) % self.buf.len();
    }

    /// Advance the buffer's write cursor `num` bytes.
    pub fn advance_write(&mut self, num: usize) {
        self.write_cursor = (self.write_cursor + num) % self.buf.len();
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.write_cursor = 0;
        self.read_cursor = 0;
    }

    /// Grow the size of the buffer by `factor`. The size of the buffer will be rounded
    /// up to the nearest power of two that is greater than or equal to the the current
    /// size of the buffer multiplied by `factor`. If the size of the buffer will overflow
    /// `usize` then `CircBufError::Overflow` will be returned else an empty tuple `()` will
    /// be returned.
    pub fn grow_with_factor(&mut self, factor: usize) -> Result<(), CircBufError> {
        let cap = match self.buf.len().checked_mul(factor) {
            Some(cap) => cap,
            None => return Err(CircBufError::Overflow),
        };

        let cap_checked = match cap.checked_next_power_of_two() {
            Some(cap) => cap,
            None => return Err(CircBufError::Overflow),
        };

        let mut new_buf = vec![0; cap_checked].into_boxed_slice();
        let mut bytes_written = 0;

        // copy the readable bytes from the old buffer to the new buffer
        if self.write_cursor < self.read_cursor {
            let num_to_end = self.buf.len() - self.read_cursor;
            unsafe { copy_nonoverlapping(&self.buf[self.read_cursor], &mut new_buf[0], num_to_end) }

            bytes_written += num_to_end;

            unsafe {
                copy_nonoverlapping(&self.buf[0], &mut new_buf[bytes_written], self.write_cursor)
            }

            bytes_written += self.write_cursor;
        } else {
            let num_to_copy = self.write_cursor - self.read_cursor;
            unsafe {
                copy_nonoverlapping(&self.buf[self.read_cursor], &mut new_buf[0], num_to_copy)
            }
            bytes_written += num_to_copy;
        }

        self.buf = new_buf;
        self.write_cursor = bytes_written;
        self.read_cursor = 0;

        Ok(())
    }

    /// Grow the size of the buffer. The buffer will be expanded by a factor of
    /// `DEFAULT_SIZE_MULTIPLIER`. If the size of the buffer will overflow `usize`
    /// then `CircBufError::Overflow` will be returned else an empty tuple `()` will
    /// be returned.
    pub fn grow(&mut self) -> Result<(), CircBufError> {
        self.grow_with_factor(DEFAULT_SIZE_MULTIPLIER)
    }

    /// Return an array that contains two mutable slices which point to the available
    /// bytes in the buffer. The combined lengths of the slices will be the minimum
    /// of `size` and `self.avail()`. If the available bytes in the buffer are contiguous
    /// then the second slice will be of size zero. Otherwise, the first slice will point
    /// to the bytes available at the end of the buffer and the second slice will point
    /// to the bytes available at the start of the buffer. The array can be used for
    /// vector IO.
    pub fn get_avail_upto_size(&mut self, size: usize) -> [&mut [u8]; 2] {
        let first_buf;
        let second_buf;

        let min = if self.avail() < size {
            self.avail()
        } else {
            size
        };

        if self.write_cursor >= self.read_cursor && min > self.buf.len() - self.write_cursor {
            // the min available bytes wrap around the buffer, so we need to two slices to access
            // the available bytes at the end and the start of the buffer
            unsafe {
                first_buf = from_raw_parts_mut(&mut self.buf[self.write_cursor],
                                               self.buf.len() - self.write_cursor);
                second_buf = from_raw_parts_mut(&mut self.buf[0],
                                                min - (self.buf.len() - self.write_cursor));
            }
        } else {
            // the min available bytes are contiguous so our second buffer will have size zero
            unsafe {
                first_buf = from_raw_parts_mut(&mut self.buf[self.write_cursor], min);
                second_buf = from_raw_parts_mut(&mut self.buf[self.write_cursor], 0);
            }
        }

        [first_buf, second_buf]
    }

    /// Return an array that contains two slices which point to the bytes that have been
    /// written to the buffer. The combined lengths of the slices will be the minimum
    /// of `size` and `self.len()`. If the bytes are contiguous then the second slice will be
    /// of size zero. Otherwise, the first slice will point to the bytes at the end of the
    /// buffer and the second slice will point to the bytes available at the start of the
    /// buffer.
    pub fn get_bytes_upto_size(&mut self, size: usize) -> [&[u8]; 2] {
        let first_buf;
        let second_buf;


        let min = if self.len() < size { self.len() } else { size };

        if self.write_cursor < self.read_cursor && min > self.buf.len() - self.read_cursor {
            // the min bytes to be read wrap around the buffer so we need two slices
            unsafe {
                first_buf = from_raw_parts(&self.buf[self.read_cursor],
                                           self.buf.len() - self.read_cursor);
                second_buf = from_raw_parts(&self.buf[0],
                                            min - (self.buf.len() - self.read_cursor));
            }
        } else {
            // the min bytes to be read are contiguous so our second buffer will be of size zero
            unsafe {
                first_buf = from_raw_parts(&self.buf[self.read_cursor], min);
                second_buf = from_raw_parts(&self.buf[self.read_cursor], 0);
            }
        }

        [first_buf, second_buf]
    }

    /// Return an array that contains two slices which point to the bytes that are available
    /// in the buffer. A convenience method for `get_avail_upto_size` with `size` equal to
    /// `self.avail()` so all bytes written available in buffer will be returned.
    pub fn get_avail(&mut self) -> [&mut [u8]; 2] {
        let avail = self.avail();
        self.get_avail_upto_size(avail)
    }

    /// Return an array that contains two slices which point to the bytes that have been
    /// written to the buffer. A convenience method for `get_bytes_upto_size` with `size`
    /// equal to `self.len()` so all bytes written to the buffer will be returned.
    pub fn get_bytes(&mut self) -> [&[u8]; 2] {
        let len = self.len();
        self.get_bytes_upto_size(len)
    }
}

impl io::Read for CircBuf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.len();

        if len == 0 {
            return Ok(0);
        }

        let num_to_read = if len < buf.len() { len } else { buf.len() };

        if self.write_cursor < self.read_cursor {
            // check if we need to wrap around the buffer to read num_to_read bytes
            let num_to_end = self.buf.len() - self.read_cursor;
            let min = if num_to_read < num_to_end {
                num_to_read
            } else {
                num_to_end
            };

            unsafe { copy_nonoverlapping(&self.buf[self.read_cursor], &mut buf[0], min) };

            if min != num_to_read {
                unsafe { copy_nonoverlapping(&self.buf[0], &mut buf[min], num_to_read - min) };
            }
        } else {
            unsafe { copy_nonoverlapping(&self.buf[self.read_cursor], &mut buf[0], num_to_read) };
        }

        self.advance_read(num_to_read);

        Ok(num_to_read)
    }
}

impl io::Write for CircBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let avail = self.avail();

        if avail == 0 {
            return Ok(0);
        }

        let num_to_write = if avail < buf.len() { avail } else { buf.len() };

        if self.write_cursor < self.read_cursor {
            unsafe { copy_nonoverlapping(&buf[0], &mut self.buf[self.write_cursor], num_to_write) };
        } else {
            // check if we need to wrap around the buffer to write num_to_write bytes
            let num_to_end = self.buf.len() - self.write_cursor;
            let min = if num_to_write < num_to_end {
                num_to_write
            } else {
                num_to_end
            };

            unsafe { copy_nonoverlapping(&buf[0], &mut self.buf[self.write_cursor], min) };

            if min != num_to_write {
                unsafe { copy_nonoverlapping(&buf[min], &mut self.buf[0], num_to_write - min) };
            }
        }

        self.advance_write(num_to_write);

        Ok(num_to_write)
    }

    /// Clear the buffer by setting the read and write pointers to 0.
    fn flush(&mut self) -> io::Result<()> {
        self.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::error::Error;
    use std::io::{Read, Write, Seek, SeekFrom};
    use std::fs::OpenOptions;
    use vecio::Rawv;
    use super::{CircBuf, DEFAULT_CAPACITY, CircBufError};
    // uncomment below to run benchmarks
    // use test::Bencher;

    #[test]
    fn create_circbuf() {
        let c = CircBuf::new();

        assert_eq!(c.cap(), DEFAULT_CAPACITY - 1);
        assert_eq!(c.avail(), DEFAULT_CAPACITY - 1);
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());
    }

    #[test]
    fn create_circbuf_with_capacity() {
        let c = CircBuf::with_capacity(49).unwrap();

        assert_eq!(c.cap(), 64 - 1);
        assert_eq!(c.avail(), 64 - 1);
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());
    }

    #[test]
    fn put_peek_and_get_bytes() {
        let mut c = CircBuf::with_capacity(4).unwrap();

        c.put(1).unwrap();
        c.put(2).unwrap();
        c.put(3).unwrap();

        assert_eq!(c.put(4).unwrap_err().description(),
                   CircBufError::BufFull.description());
        assert_eq!(c.avail(), 0);
        assert_eq!(c.len(), 3);
        assert!(c.is_full());

        assert_eq!(c.peek().unwrap(), 1);
        assert_eq!(c.get().unwrap(), 1);
        assert_eq!(c.peek().unwrap(), 2);
        assert_eq!(c.get().unwrap(), 2);
        assert_eq!(c.peek().unwrap(), 3);
        assert_eq!(c.get().unwrap(), 3);
        assert_eq!(c.get().unwrap_err().description(),
                   CircBufError::BufEmpty.description());

        assert_eq!(c.avail(), 3);
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());

        // check that everything continues to work when we wrap around the buffer
        c.put(4).unwrap();
        c.put(5).unwrap();
        c.put(6).unwrap();

        assert_eq!(c.peek().unwrap(), 4);
        assert_eq!(c.get().unwrap(), 4);
        assert_eq!(c.peek().unwrap(), 5);
        assert_eq!(c.get().unwrap(), 5);
        assert_eq!(c.peek().unwrap(), 6);
        assert_eq!(c.get().unwrap(), 6);
    }

    #[test]
    fn find_bytes() {
        let mut c = CircBuf::with_capacity(8).unwrap();

        c.put(7).unwrap();
        c.put(6).unwrap();
        c.put(5).unwrap();
        c.put(4).unwrap();
        c.put(3).unwrap();
        c.put(2).unwrap();
        c.put(1).unwrap();

        assert_eq!(c.find(4).unwrap(), 3);
        assert_eq!(c.find_from_index(3, 2).unwrap(), 4);
        assert!(c.find_from_index(6, 2).is_none());

        c.advance_read(4);

        assert_eq!(c.find(1).unwrap(), 2);
        assert!(c.find(5).is_none());

        // wrap around the buffer
        c.put(10).unwrap();
        c.put(11).unwrap();
        c.put(12).unwrap();

        assert_eq!(c.find(12).unwrap(), 5);
        assert_eq!(c.find_from_index(12, 4).unwrap(), 5);
    }

    #[test]
    fn grow_buffer() {
        let mut c = CircBuf::with_capacity(4).unwrap();

        c.put(1).unwrap();
        c.put(2).unwrap();
        c.put(3).unwrap();

        c.grow().unwrap();

        assert!(c.cap() == 8 - 1);
        assert!(c.avail() == 4);
        assert!(c.len() == 3);

        assert_eq!(c.get().unwrap(), 1);
        assert_eq!(c.get().unwrap(), 2);
        assert_eq!(c.get().unwrap(), 3);

        // grow a buffer that wraps around
        c.advance_read(7);
        c.advance_write(7);

        c.put(1).unwrap();
        c.put(2).unwrap();
        c.put(3).unwrap();

        c.grow().unwrap();

        assert_eq!(c.get().unwrap(), 1);
        assert_eq!(c.get().unwrap(), 2);
        assert_eq!(c.get().unwrap(), 3);
    }

    #[test]
    fn read_and_write_bytes() {
        let mut c = CircBuf::with_capacity(8).unwrap();

        assert_eq!(c.write(b"foo").unwrap(), 3);
        assert_eq!(c.write(b"bar").unwrap(), 3);

        assert_eq!(c.cap(), 7);
        assert_eq!(c.avail(), 1);
        assert_eq!(c.len(), 6);

        let mut buf = [0; 6];

        assert_eq!(c.read(&mut buf).unwrap(), 6);
        assert!(b"foobar".iter().zip(buf.iter()).all(|(a, b)| a == b));

        assert_eq!(c.cap(), 7);
        assert_eq!(c.avail(), 7);
        assert_eq!(c.len(), 0);

        // test read and write on a buffer that wraps around
        c.advance_read(7);
        c.advance_write(7);

        assert_eq!(c.write(b"foo").unwrap(), 3);
        assert_eq!(c.write(b"bar").unwrap(), 3);

        let mut buf = [0; 6];

        assert_eq!(c.read(&mut buf).unwrap(), 6);
        assert!(b"foobar".iter().zip(buf.iter()).all(|(a, b)| a == b));
    }

    #[test]
    fn get_bytes_and_avail() {
        let mut c = CircBuf::with_capacity(16).unwrap();

        assert_eq!(c.write(b"funkytowns").unwrap(), 10);
        assert_eq!(c.len(), 10);
        assert_eq!(c.avail(), 5);

        {
            let bufs = c.get_avail_upto_size(4);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 4);
            assert_eq!(bufs[1].len(), 0);

        }

        {
            let bufs = c.get_bytes_upto_size(5);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 5);
            assert_eq!(bufs[1].len(), 0);
            assert!(b"funky".iter().zip(bufs[0].iter()).all(|(a, b)| a == b));
        }

        // test when size is greate than `c.avail()` and `c.len()` respectively
        {
            let bufs = c.get_avail_upto_size(10);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 5);
            assert_eq!(bufs[1].len(), 0);

        }

        {
            let bufs = c.get_bytes_upto_size(12);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 10);
            assert_eq!(bufs[1].len(), 0);
            assert!(b"funky".iter().zip(bufs[0].iter()).all(|(a, b)| a == b));
        }

        {
            let bufs = c.get_avail();
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 5);
            assert_eq!(bufs[1].len(), 0);
        }

        {
            let bufs = c.get_bytes();
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 10);
            assert_eq!(bufs[1].len(), 0);
            assert!(b"funkytowns".iter().zip(bufs[0].iter()).all(|(a, b)| a == b));
        }

        // test when the buffer wraps around
        c.advance_read(10);
        assert_eq!(c.write(b"brickhouse").unwrap(), 10);
        assert_eq!(c.len(), 10);
        assert_eq!(c.avail(), 5);

        {
            let bufs = c.get_avail_upto_size(4);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 4);
            assert_eq!(bufs[1].len(), 0);
        }

        {
            let bufs = c.get_bytes_upto_size(8);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 6);
            assert_eq!(bufs[1].len(), 2);
            assert!(b"brickh".iter().zip(bufs[0].iter()).all(|(a, b)| a == b));
            assert!(b"ou".iter().zip(bufs[1].iter()).all(|(a, b)| a == b));
        }

        // test when size is greate than `c.avail()` and `c.len()` respectively
        {
            let bufs = c.get_avail_upto_size(17);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 5);
            assert_eq!(bufs[1].len(), 0);
        }

        {
            let bufs = c.get_bytes_upto_size(12);
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 6);
            assert_eq!(bufs[1].len(), 4);
            assert!(b"brickh".iter().zip(bufs[0].iter()).all(|(a, b)| a == b));
            assert!(b"ouse".iter().zip(bufs[1].iter()).all(|(a, b)| a == b));
        }

        {
            let bufs = c.get_avail();
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 5);
            assert_eq!(bufs[1].len(), 0);
        }

        {
            let bufs = c.get_bytes();
            assert_eq!(bufs.len(), 2);
            assert_eq!(bufs[0].len(), 6);
            assert_eq!(bufs[1].len(), 4);
            assert!(b"brickh".iter().zip(bufs[0].iter()).all(|(a, b)| a == b));
            assert!(b"ouse".iter().zip(bufs[1].iter()).all(|(a, b)| a == b));
        }
    }

    #[test]
    fn vecio() {
        let mut c = CircBuf::with_capacity(16).unwrap();

        let mut path = env::temp_dir();
        path.push("circbuf-rs-test.txt");

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.to_str().unwrap())
            .unwrap();

        assert_eq!(file.write(b"foo\nbar\nbaz\n").unwrap(), 12);
        file.seek(SeekFrom::Current(-12)).unwrap();

        {
            let mut bufs = c.get_avail();
            assert_eq!(file.readv(&mut bufs).unwrap(), 12);
        }

        // advance the write cursor since we just wrote 12 bytes to the buffer
        c.advance_write(12);

        // wrap around the buffer
        c.advance_read(12);
        assert_eq!(c.write(b"fizzbuzz").unwrap(), 8);

        let mut s = String::new();
        assert_eq!(c.read_to_string(&mut s).unwrap(), 8);
        assert_eq!(s, "fizzbuzz");
    }

    // #[bench]
    // pub fn normal_read(b: &mut Bencher) {
    //     let mut c = CircBuf::with_capacity(16).unwrap();

    //     let mut path = env::temp_dir();
    //     path.push("circbuf-rs-test.txt");

    //     let mut file = OpenOptions::new()
    //         .read(true)
    //         .write(true)
    //         .create(true)
    //         .truncate(true)
    //         .open(path.to_str().unwrap())
    //         .unwrap();

    //     assert_eq!(file.write(b"foo\nbar\nbaz\n").unwrap(), 12);
    //     file.seek(SeekFrom::Current(-12)).unwrap();

    //     c.advance_read(8);
    //     c.advance_write(8);
    //     let mut bufs = c.get_avail();

    //     b.iter(|| {
    //         file.read(&mut bufs[0]).unwrap();
    //         file.read(&mut bufs[1]).unwrap();
    //         file.seek(SeekFrom::Current(-12)).unwrap();
    //     })
    // }

    // #[bench]
    // pub fn vector_read(b: &mut Bencher) {
    //     let mut c = CircBuf::with_capacity(16).unwrap();

    //     let mut path = env::temp_dir();
    //     path.push("circbuf-rs-test.txt");

    //     let mut file = OpenOptions::new()
    //         .read(true)
    //         .write(true)
    //         .create(true)
    //         .truncate(true)
    //         .open(path.to_str().unwrap())
    //         .unwrap();

    //     assert_eq!(file.write(b"foo\nbar\nbaz\n").unwrap(), 12);
    //     file.seek(SeekFrom::Current(-12)).unwrap();

    //     c.advance_read(8);
    //     c.advance_write(8);
    //     let mut bufs = c.get_avail();

    //     b.iter(|| {
    //         file.readv(&mut bufs).unwrap();
    //         file.seek(SeekFrom::Current(-12)).unwrap();
    //     })
    // }

    // #[bench]
    // // we call seek in iter in both benches above so lets get a base time for it
    // pub fn seek_base(b: &mut Bencher) {
    //     let mut path = env::temp_dir();
    //     path.push("circbuf-rs-test.txt");

    //     let mut file = OpenOptions::new()
    //         .read(true)
    //         .write(true)
    //         .create(true)
    //         .truncate(true)
    //         .open(path.to_str().unwrap())
    //         .unwrap();

    //     b.iter(|| {
    //         file.seek(SeekFrom::Current(1)).unwrap();
    //     })
    // }
}

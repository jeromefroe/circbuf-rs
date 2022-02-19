use std::io::IoSlice;

use crate::CircBuf;

use bytes::{buf::UninitSlice, Buf, BufMut};

impl Buf for CircBuf {
    fn advance(&mut self, count: usize) {
        assert!(count == 0 || count <= self.remaining());
        self.advance_read_raw(count);
    }

    fn chunk(&self) -> &[u8] {
        let [left, right] = self.get_bytes();
        match (left.is_empty(), right.is_empty()) {
            (true, true) => left,
            (true, false) => right,
            (false, true) => left,
            (false, false) => left,
        }
    }

    fn remaining(&self) -> usize {
        self.len()
    }

    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        let [left, right] = self.get_bytes();
        let mut count = 0;
        if let Some(slice) = dst.get_mut(0) {
            count += 1;
            *slice = IoSlice::new(left);
        }
        if let Some(slice) = dst.get_mut(1) {
            count += 1;
            *slice = IoSlice::new(right);
        }
        count
    }
}

unsafe impl BufMut for CircBuf {
    unsafe fn advance_mut(&mut self, count: usize) {
        assert!(count == 0 || count <= self.remaining_mut());
        self.advance_write_raw(count);
    }

    fn chunk_mut(&mut self) -> &mut UninitSlice {
        let [left, right] = self.get_avail();
        let slice = match (left.is_empty(), right.is_empty()) {
            (true, true) => left,
            (true, false) => right,
            (false, true) => left,
            (false, false) => left,
        };
        // https://docs.rs/bytes/latest/bytes/buf/struct.UninitSlice.html#method.from_raw_parts_mut
        unsafe { UninitSlice::from_raw_parts_mut(slice.as_mut_ptr(), slice.len()) }
    }

    fn remaining_mut(&self) -> usize {
        self.avail()
    }
}

#[cfg(test)]
mod tests {
    use crate::CircBuf;
    use bytes::{Buf, BufMut};
    use std::io::IoSlice;

    #[test]
    fn bytes_buf_and_bufmut() {
        let mut c = CircBuf::with_capacity(4).unwrap();

        assert_eq!(c.remaining(), 0);
        assert_eq!(c.remaining_mut(), 3);
        unsafe {
            c.advance_mut(2);
        }
        assert_eq!(c.remaining(), 2);
        assert_eq!(c.remaining_mut(), 1);
        c.advance(1);
        assert_eq!(c.remaining(), 1);
        assert_eq!(c.remaining_mut(), 2);
        unsafe {
            c.advance_mut(1);
        }
        assert_eq!(c.remaining(), 2);
        assert_eq!(c.remaining_mut(), 1);

        assert_eq!(<CircBuf as Buf>::chunk(&c).len(), 2);
        assert_eq!(c.chunk_mut().len(), 1);

        let mut dst = [IoSlice::new(&[]); 2];
        assert_eq!(c.chunks_vectored(&mut dst[..]), 2);

        assert_eq!(dst[0].len(), 2);
        assert_eq!(dst[1].len(), 0);
    }

    #[test]
    fn bytes_buf_remaining() {
        use bytes::{Buf, BufMut};

        let mut c = CircBuf::with_capacity(4).unwrap();

        assert_eq!(c.remaining(), 0);
        assert_eq!(c.remaining_mut(), 3);
        assert!(!c.has_remaining());
        assert!(c.has_remaining_mut());

        unsafe {
            c.advance_mut(3);
        }

        assert_eq!(c.remaining(), 3);
        assert_eq!(c.remaining_mut(), 0);
        assert!(c.has_remaining());
        assert!(!c.has_remaining_mut());

        c.advance(2);

        assert_eq!(c.remaining(), 1);
        assert_eq!(c.remaining_mut(), 2);
        assert!(c.has_remaining());
        assert!(c.has_remaining_mut());

        c.advance(1);

        assert_eq!(c.remaining(), 0);
        assert_eq!(c.remaining_mut(), 3);
        assert!(!c.has_remaining());
        assert!(c.has_remaining_mut());
    }

    #[cfg(feature = "bytes")]
    #[test]
    fn bytes_bufmut_hello() {
        use bytes::BufMut;

        let mut c = CircBuf::with_capacity(16).unwrap();

        unsafe {
            c.chunk_mut().write_byte(0, b'h');
            c.chunk_mut().write_byte(1, b'e');

            c.advance_mut(2);

            c.chunk_mut().write_byte(0, b'l');
            c.chunk_mut().write_byte(1, b'l');
            c.chunk_mut().write_byte(2, b'o');

            c.advance_mut(3);
        }

        assert_eq!(c.get_bytes()[0], b"hello");
    }
}

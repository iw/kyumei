//! Varints, zigzag integers and length-prefixed strings.
//!
//! The reader works on untrusted input: every read is bounds-checked, and a
//! count that cannot fit in the remaining bytes is refused before anything is
//! allocated for it.

/// Why a payload could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WireError(pub &'static str);

pub(crate) fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

pub(crate) fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[derive(Default)]
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn uvarint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.buf.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
    }

    pub fn ivarint(&mut self, v: i64) {
        self.uvarint(zigzag(v));
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn str(&mut self, s: &str) {
        self.uvarint(s.len() as u64);
        self.bytes(s.as_bytes());
    }
}

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn u8(&mut self) -> Result<u8, WireError> {
        let b = *self
            .buf
            .get(self.pos)
            .ok_or(WireError("the samples end early"))?;
        self.pos += 1;
        Ok(b)
    }

    pub fn uvarint(&mut self) -> Result<u64, WireError> {
        let mut v: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(WireError("a number in the samples is too long"))
    }

    pub fn ivarint(&mut self) -> Result<i64, WireError> {
        self.uvarint().map(unzigzag)
    }

    /// A count of items that each take at least one byte.
    pub fn count(&mut self) -> Result<usize, WireError> {
        let n = self.uvarint()?;
        if n > self.remaining() as u64 {
            return Err(WireError(
                "a count in the samples is larger than the samples",
            ));
        }
        Ok(n as usize)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if n > self.remaining() {
            return Err(WireError("the samples end early"));
        }
        let b = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }

    pub fn str(&mut self) -> Result<String, WireError> {
        let n = self.count()?;
        let b = self.bytes(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| WireError("a text field is not UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_across_the_range() {
        let values = [0, 1, -1, 63, -64, 64, 300, -300, i64::MAX, i64::MIN];
        let mut w = Writer::default();
        for v in values {
            w.ivarint(v);
        }
        w.uvarint(u64::MAX);
        let mut r = Reader::new(&w.buf);
        for v in values {
            assert_eq!(r.ivarint(), Ok(v));
        }
        assert_eq!(r.uvarint(), Ok(u64::MAX));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_count_larger_than_the_input_is_refused() {
        let mut w = Writer::default();
        w.uvarint(1_000_000);
        assert!(Reader::new(&w.buf).count().is_err());
    }

    #[test]
    fn an_overlong_varint_is_refused() {
        let bytes = [0xff; 11];
        assert!(Reader::new(&bytes).uvarint().is_err());
    }
}

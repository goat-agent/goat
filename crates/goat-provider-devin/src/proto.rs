pub const WIRE_VARINT: u32 = 0;
pub const WIRE_FIXED64: u32 = 1;
pub const WIRE_LEN: u32 = 2;
pub const WIRE_FIXED32: u32 = 5;

pub fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn tag(out: &mut Vec<u8>, field: u32, wire: u32) {
    varint(out, u64::from(field << 3 | wire));
}

pub fn field_varint(out: &mut Vec<u8>, field: u32, value: u64) {
    tag(out, field, WIRE_VARINT);
    varint(out, value);
}

pub fn field_bool(out: &mut Vec<u8>, field: u32, value: bool) {
    field_varint(out, field, u64::from(value));
}

pub fn field_double(out: &mut Vec<u8>, field: u32, value: f64) {
    tag(out, field, WIRE_FIXED64);
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn field_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    tag(out, field, WIRE_LEN);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

pub fn field_str(out: &mut Vec<u8>, field: u32, value: &str) {
    if value.is_empty() {
        return;
    }
    field_bytes(out, field, value.as_bytes());
}

pub fn field_msg(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    field_bytes(out, field, value);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn next(&mut self) -> Option<(u32, Value<'a>)> {
        let key = self.read_varint()?;
        let field = u32::try_from(key >> 3).ok()?;
        let wire = u32::try_from(key & 7).ok()?;
        let value = match wire {
            WIRE_VARINT => Value::Varint(self.read_varint()?),
            WIRE_FIXED64 => {
                let bytes = self.take(8)?;
                Value::Fixed64(u64::from_le_bytes(bytes.try_into().ok()?))
            }
            WIRE_LEN => {
                let len = usize::try_from(self.read_varint()?).ok()?;
                Value::Bytes(self.take(len)?)
            }
            WIRE_FIXED32 => {
                let bytes = self.take(4)?;
                Value::Fixed32(u32::from_le_bytes(bytes.try_into().ok()?))
            }
            _ => return None,
        };
        Some((field, value))
    }

    fn read_varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.buf.get(self.pos)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }
}

pub fn as_str(value: Value<'_>) -> Option<&str> {
    match value {
        Value::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
        _ => None,
    }
}

pub fn as_u64(value: Value<'_>) -> Option<u64> {
    match value {
        Value::Varint(v) => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trip() {
        let mut out = Vec::new();
        varint(&mut out, 300);
        assert_eq!(out, [0xac, 0x02]);
        let mut reader = Reader::new(&[0x08, 0xac, 0x02]);
        assert_eq!(reader.next(), Some((1, Value::Varint(300))));
        assert!(reader.next().is_none());
    }

    #[test]
    fn str_and_msg_round_trip() {
        let mut inner = Vec::new();
        field_str(&mut inner, 1, "chisel");
        field_varint(&mut inner, 2, 42);
        let mut out = Vec::new();
        field_msg(&mut out, 3, &inner);
        field_str(&mut out, 4, "en");
        field_double(&mut out, 5, 0.4);

        let mut reader = Reader::new(&out);
        let (f, v) = reader.next().unwrap();
        assert_eq!(f, 3);
        let Value::Bytes(msg) = v else { panic!() };
        let mut inner_reader = Reader::new(msg);
        assert_eq!(
            inner_reader.next(),
            Some((1, Value::Bytes(b"chisel".as_slice())))
        );
        assert_eq!(inner_reader.next(), Some((2, Value::Varint(42))));
        assert_eq!(reader.next().unwrap().0, 4);
        assert_eq!(reader.next().unwrap().0, 5);
    }

    #[test]
    fn empty_str_is_skipped() {
        let mut out = Vec::new();
        field_str(&mut out, 1, "");
        assert!(out.is_empty());
    }
}

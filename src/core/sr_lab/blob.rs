use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const MAGIC: &[u8; 8] = b"SSRLAB01";
const MAX_WEIGHT_BLOB_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct SrLabTensor {
    pub name: String,
    pub shape: Vec<u32>,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrLabWeights {
    pub tensors: Vec<SrLabTensor>,
}

impl SrLabWeights {
    pub fn tensor(&self, name: &str) -> Option<&SrLabTensor> {
        self.tensors.iter().find(|tensor| tensor.name == name)
    }
}

pub fn read_weights(path: &Path) -> Result<SrLabWeights, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_WEIGHT_BLOB_BYTES {
        return Err(format!(
            "SR Lab weight blob is too large: {} bytes",
            metadata.len()
        ));
    }
    parse_weights(&fs::read(path).map_err(|error| error.to_string())?)
}

pub fn parse_weights(bytes: &[u8]) -> Result<SrLabWeights, String> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(MAGIC.len())? != MAGIC {
        return Err("invalid SR Lab weight blob header".to_owned());
    }
    let tensor_count = cursor.u32()? as usize;
    let mut tensors = Vec::with_capacity(tensor_count);
    let mut names = BTreeSet::new();

    for _ in 0..tensor_count {
        let name_len = cursor.u16()? as usize;
        let name_bytes = cursor.take(name_len)?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| "SR Lab tensor name is not UTF-8".to_owned())?
            .to_owned();
        if name.is_empty() {
            return Err("SR Lab tensor name is empty".to_owned());
        }
        if !names.insert(name.clone()) {
            return Err(format!("duplicate SR Lab tensor name: {name}"));
        }

        let rank = cursor.u8()? as usize;
        if rank > 4 {
            return Err(format!("SR Lab tensor {name} has unsupported rank {rank}"));
        }
        let shape4 = [cursor.u32()?, cursor.u32()?, cursor.u32()?, cursor.u32()?];
        let shape = shape4[..rank].to_vec();
        if shape.iter().any(|dimension| *dimension == 0) {
            return Err(format!("SR Lab tensor {name} has a zero dimension"));
        }

        let byte_len_u64 = cursor.u64()?;
        let byte_len = usize::try_from(byte_len_u64)
            .map_err(|_| format!("SR Lab tensor {name} byte length does not fit usize"))?;
        if byte_len % std::mem::size_of::<f32>() != 0 {
            return Err(format!(
                "SR Lab tensor {name} byte length is not f32-aligned"
            ));
        }
        let expected_values = shape
            .iter()
            .try_fold(1usize, |total, dimension| {
                total.checked_mul(*dimension as usize)
            })
            .ok_or_else(|| format!("SR Lab tensor {name} shape is too large"))?;
        let actual_values = byte_len / std::mem::size_of::<f32>();
        if expected_values != actual_values {
            return Err(format!(
                "SR Lab tensor {name} shape expects {expected_values} values, blob has {actual_values}"
            ));
        }

        let raw = cursor.take(byte_len)?;
        let mut values = Vec::with_capacity(actual_values);
        for chunk in raw.chunks_exact(4) {
            values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        tensors.push(SrLabTensor {
            name,
            shape,
            values,
        });
    }

    if !cursor.is_finished() {
        return Err("SR Lab weight blob has trailing bytes".to_owned());
    }

    Ok(SrLabWeights { tensors })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(size)
            .ok_or_else(|| "SR Lab weight blob offset overflowed".to_owned())?;
        let chunk = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| "truncated SR Lab weight blob".to_owned())?;
        self.offset = end;
        Ok(chunk)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_weights;

    fn tiny_blob() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"SSRLAB01");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&6u16.to_le_bytes());
        bytes.extend_from_slice(b"weight");
        bytes.push(2);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&8u64.to_le_bytes());
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&(-2.0f32).to_le_bytes());
        bytes
    }

    #[test]
    fn parses_srlab01_tensor_blob() {
        let weights = parse_weights(&tiny_blob()).unwrap();

        let tensor = weights.tensor("weight").unwrap();
        assert_eq!(tensor.shape, vec![1, 2]);
        assert_eq!(tensor.values, vec![1.5, -2.0]);
    }

    #[test]
    fn rejects_duplicate_tensor_names() {
        let mut bytes = tiny_blob();
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        let second = tiny_blob().split_off(12);
        bytes.extend_from_slice(&second);

        let error = parse_weights(&bytes).unwrap_err();

        assert!(error.contains("duplicate"));
    }

    #[test]
    fn rejects_shape_byte_length_mismatch() {
        let mut bytes = tiny_blob();
        let byte_len_offset = 8 + 4 + 2 + 6 + 1 + 16;
        bytes[byte_len_offset..byte_len_offset + 8].copy_from_slice(&4u64.to_le_bytes());

        let error = parse_weights(&bytes).unwrap_err();

        assert!(error.contains("shape expects"));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = tiny_blob();
        bytes.push(0);

        let error = parse_weights(&bytes).unwrap_err();

        assert!(error.contains("trailing"));
    }
}

use anyhow::{Result, bail};
use base64::Engine;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum RawValue {
    Varint(u64),
    Fixed64(u64),
    LengthDelimited(Vec<u8>),
    Fixed32(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawField {
    pub number: u32,
    pub value: RawValue,
}

pub fn decode_message(mut input: &[u8]) -> Result<Vec<RawField>> {
    let mut fields = Vec::new();
    while !input.is_empty() {
        let key = read_varint(&mut input)?;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x07) as u8;
        if number == 0 {
            bail!("invalid protobuf field number 0");
        }

        let value = match wire_type {
            0 => RawValue::Varint(read_varint(&mut input)?),
            1 => {
                if input.len() < 8 {
                    bail!("truncated fixed64 field {number}");
                }
                let (head, tail) = input.split_at(8);
                input = tail;
                RawValue::Fixed64(u64::from_le_bytes(head.try_into().unwrap()))
            }
            2 => {
                let len = read_varint(&mut input)? as usize;
                if input.len() < len {
                    bail!("truncated length-delimited field {number}");
                }
                let (head, tail) = input.split_at(len);
                input = tail;
                RawValue::LengthDelimited(head.to_vec())
            }
            5 => {
                if input.len() < 4 {
                    bail!("truncated fixed32 field {number}");
                }
                let (head, tail) = input.split_at(4);
                input = tail;
                RawValue::Fixed32(u32::from_le_bytes(head.try_into().unwrap()))
            }
            3 | 4 => bail!("groups are not supported"),
            _ => bail!("invalid wire type {wire_type}"),
        };

        fields.push(RawField { number, value });
    }
    Ok(fields)
}

pub fn to_json(fields: &[RawField]) -> Value {
    let mut object = Map::new();
    for field in fields {
        object
            .entry(field.number.to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .unwrap()
            .push(raw_value_to_json(&field.value));
    }
    Value::Object(object)
}

pub fn raw_bytes_from_json(value: &Value) -> Option<Vec<u8>> {
    if let Some(bytes) = value.get("bytes_base64").and_then(Value::as_str) {
        return base64::engine::general_purpose::STANDARD
            .decode(bytes.as_bytes())
            .ok();
    }
    if let Some(array) = value.as_array() {
        let mut out = Vec::new();
        for item in array {
            out.extend(raw_bytes_from_json(item)?);
        }
        return Some(out);
    }
    None
}

fn raw_value_to_json(value: &RawValue) -> Value {
    match value {
        RawValue::Varint(n) => json!({
            "wire": "varint",
            "value": n,
            "zigzag_i64": ((*n >> 1) as i64) ^ (-((*n & 1) as i64)),
        }),
        RawValue::Fixed64(n) => json!({
            "wire": "fixed64",
            "value": n,
            "double_le": f64::from_bits(*n),
        }),
        RawValue::Fixed32(n) => json!({
            "wire": "fixed32",
            "value": n,
            "float_le": f32::from_bits(*n),
        }),
        RawValue::LengthDelimited(bytes) => {
            let mut object = Map::new();
            object.insert("wire".into(), Value::String("len".into()));
            object.insert(
                "bytes_base64".into(),
                Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
            );
            object.insert("len".into(), Value::from(bytes.len()));
            if let Ok(text) = std::str::from_utf8(bytes) {
                object.insert("utf8".into(), Value::String(text.to_owned()));
            }
            if let Ok(nested) = decode_message(bytes)
                && !nested.is_empty()
            {
                object.insert("message".into(), to_json(&nested));
            }
            Value::Object(object)
        }
    }
}

fn read_varint(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        if input.is_empty() {
            bail!("truncated varint");
        }
        let byte = input[0];
        *input = &input[1..];
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("varint is too long")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_unknown_wire_message() {
        let decoded = decode_message(&[0x08, 0x96, 0x01, 0x12, 0x03, b'f', b'o', b'o']).unwrap();
        let json = to_json(&decoded);
        assert_eq!(json["1"][0]["value"], 150);
        assert_eq!(json["2"][0]["utf8"], "foo");
    }
}

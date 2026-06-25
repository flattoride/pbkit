use anyhow::{Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Index(usize),
    Wildcard,
}

pub fn parse_path(input: &str) -> Result<Vec<Segment>> {
    let mut chars = input.trim().chars().peekable();
    if matches!(chars.peek(), Some('$')) {
        chars.next();
    }

    let mut segments = Vec::new();
    while let Some(ch) = chars.peek().copied() {
        match ch {
            '.' => {
                chars.next();
                if matches!(chars.peek(), Some('*')) {
                    chars.next();
                    segments.push(Segment::Wildcard);
                    continue;
                }
                let mut key = String::new();
                while let Some(next) = chars.peek().copied() {
                    if next == '.' || next == '[' {
                        break;
                    }
                    key.push(next);
                    chars.next();
                }
                if key.is_empty() {
                    bail!("empty path key in {input:?}");
                }
                segments.push(Segment::Key(key));
            }
            '[' => {
                chars.next();
                if matches!(chars.peek(), Some('*')) {
                    chars.next();
                    if chars.next() != Some(']') {
                        bail!("unterminated wildcard in {input:?}");
                    }
                    segments.push(Segment::Wildcard);
                } else if matches!(chars.peek(), Some('"') | Some('\'')) {
                    let quote = chars.next().unwrap();
                    let mut key = String::new();
                    for next in chars.by_ref() {
                        if next == quote {
                            break;
                        }
                        key.push(next);
                    }
                    if chars.next() != Some(']') {
                        bail!("unterminated bracket key in {input:?}");
                    }
                    segments.push(Segment::Key(key));
                } else {
                    let mut number = String::new();
                    while let Some(next) = chars.peek().copied() {
                        if next == ']' {
                            break;
                        }
                        number.push(next);
                        chars.next();
                    }
                    if chars.next() != Some(']') {
                        bail!("unterminated bracket index in {input:?}");
                    }
                    let index = number
                        .parse::<usize>()
                        .map_err(|_| anyhow::anyhow!("invalid array index {number:?}"))?;
                    segments.push(Segment::Index(index));
                }
            }
            _ => bail!("unexpected path token {ch:?} in {input:?}"),
        }
    }

    Ok(segments)
}

pub fn select<'a>(value: &'a Value, path: &[Segment]) -> Option<&'a Value> {
    select_many(value, path).into_iter().next()
}

pub fn select_many<'a>(value: &'a Value, path: &[Segment]) -> Vec<&'a Value> {
    let mut current = vec![value];
    for segment in path {
        let mut next = Vec::new();
        for value in current {
            match segment {
                Segment::Key(key) => select_key(value, key, &mut next),
                Segment::Index(index) => {
                    if let Some(item) = value.get(*index) {
                        next.push(item);
                    }
                }
                Segment::Wildcard => select_wildcard(value, &mut next),
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current
}

fn select_key<'a>(value: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(_) => {
            if let Some(item) = value.get(key) {
                out.push(item);
            }
        }
        Value::Array(items) => {
            for item in items {
                select_key(item, key, out);
            }
        }
        _ => {}
    }
}

fn select_wildcard<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Array(items) => out.extend(items),
        Value::Object(object) => out.extend(object.values()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dot_and_bracket_paths() {
        assert_eq!(
            parse_path("$.payload[0]['data']").unwrap(),
            vec![
                Segment::Key("payload".into()),
                Segment::Index(0),
                Segment::Key("data".into())
            ]
        );
    }

    #[test]
    fn parses_wildcards() {
        assert_eq!(
            parse_path("$.items[*].id").unwrap(),
            vec![
                Segment::Key("items".into()),
                Segment::Wildcard,
                Segment::Key("id".into())
            ]
        );
        assert_eq!(parse_path("$.*").unwrap(), vec![Segment::Wildcard]);
    }

    #[test]
    fn selects_many_with_wildcards_and_array_flattening() {
        let value = serde_json::json!({
            "items": [
                { "id": 1, "labels": { "name": "a" } },
                { "id": 2, "labels": { "name": "b" } }
            ]
        });
        let wildcard_path = parse_path("$.items[*].id").unwrap();
        let flattened_path = parse_path("$.items.id").unwrap();
        let wildcard = select_many(&value, &wildcard_path);
        let flattened = select_many(&value, &flattened_path);
        assert_eq!(wildcard.len(), 2);
        assert_eq!(wildcard[0], &serde_json::json!(1));
        assert_eq!(wildcard[1], &serde_json::json!(2));
        assert_eq!(flattened, wildcard);
    }
}

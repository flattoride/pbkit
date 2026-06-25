use anyhow::{Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Index(usize),
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
                if matches!(chars.peek(), Some('"') | Some('\'')) {
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
    let mut current = value;
    for segment in path {
        match segment {
            Segment::Key(key) => current = current.get(key)?,
            Segment::Index(index) => current = current.get(*index)?,
        }
    }
    Some(current)
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
}

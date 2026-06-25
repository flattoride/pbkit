use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Number,
}

pub fn sort_proto(source: &str, field_key: SortKey) -> Result<String> {
    let lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut sorted = sort_scope(&lines, field_key);
    if source.ends_with('\n') {
        sorted.push('\n');
    }
    Ok(sorted)
}

fn sort_scope(lines: &[String], field_key: SortKey) -> String {
    let mut output = Vec::new();
    let mut i = 0;
    let mut declaration_slots = Vec::<(usize, SortItem)>::new();

    while i < lines.len() {
        if let Some((kind, name)) = parse_declaration(&lines[i]) {
            let end = find_block_end(lines, i).unwrap_or(i);
            let mut chunk = lines[i..=end].to_vec();
            if end > i + 1 {
                let inner = sort_scope(&lines[i + 1..end], field_key);
                let inner_lines = inner.lines().map(str::to_owned).collect::<Vec<_>>();
                chunk.splice(1..chunk.len() - 1, inner_lines);
            }
            let slot = output.len();
            output.extend(chunk.clone());
            declaration_slots.push((
                slot,
                SortItem {
                    key: format!("{kind}:{name}"),
                    lines: chunk,
                },
            ));
            i = end + 1;
            continue;
        }

        output.push(lines[i].clone());
        i += 1;
    }

    replace_sorted_slots(&mut output, declaration_slots);
    sort_field_runs(&mut output, field_key);
    output.join("\n")
}

#[derive(Debug, Clone)]
struct SortItem {
    key: String,
    lines: Vec<String>,
}

fn replace_sorted_slots(lines: &mut Vec<String>, mut slots: Vec<(usize, SortItem)>) {
    if slots.len() < 2 {
        return;
    }
    let mut sorted = slots
        .iter()
        .map(|(_, item)| item.clone())
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));

    slots.sort_by_key(|(index, _)| *index);
    for ((index, old), new) in slots.into_iter().zip(sorted.into_iter()) {
        lines.splice(index..index + old.lines.len(), new.lines);
    }
}

fn sort_field_runs(lines: &mut Vec<String>, field_key: SortKey) {
    let mut i = 0;
    while i < lines.len() {
        let Some(first) = parse_field(&lines[i]) else {
            i += 1;
            continue;
        };

        let start = i;
        let mut items = vec![(field_sort_key(&first, field_key), lines[i].clone())];
        i += 1;
        while i < lines.len() {
            let Some(field) = parse_field(&lines[i]) else {
                break;
            };
            items.push((field_sort_key(&field, field_key), lines[i].clone()));
            i += 1;
        }

        if items.len() > 1 {
            items.sort_by(|a, b| a.0.cmp(&b.0));
            lines.splice(start..i, items.into_iter().map(|(_, line)| line));
        }
    }
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    number: u32,
}

fn field_sort_key(field: &Field, key: SortKey) -> String {
    match key {
        SortKey::Name => field.name.clone(),
        SortKey::Number => format!("{:010}:{}", field.number, field.name),
    }
}

fn parse_declaration(line: &str) -> Option<(&'static str, String)> {
    let trimmed = line.trim_start();
    for kind in ["message", "enum"] {
        let Some(rest) = trimmed.strip_prefix(kind) else {
            continue;
        };
        if !rest.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let name = rest
            .trim_start()
            .split(|ch: char| ch.is_whitespace() || ch == '{')
            .next()?;
        if !name.is_empty() && trimmed.contains('{') {
            return Some((kind, name.to_owned()));
        }
    }
    None
}

fn parse_field(line: &str) -> Option<Field> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with("option ")
        || trimmed.starts_with("reserved ")
        || trimmed.starts_with("extensions ")
        || trimmed.starts_with("oneof ")
        || trimmed.starts_with("map<")
    {
        return None;
    }
    let eq = trimmed.find('=')?;
    let left = trimmed[..eq].trim();
    let right = trimmed[eq + 1..].trim();
    let number = right
        .split(|ch: char| !ch.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    let name = left.split_whitespace().last()?.to_owned();
    if name.is_empty() {
        return None;
    }
    Some(Field { name, number })
}

fn find_block_end(lines: &[String], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, line) in lines[start..].iter().enumerate() {
        let code = line.split("//").next().unwrap_or(line);
        for ch in code.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(start + offset);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_messages_enums_and_fields() {
        let input = r#"message Z {
  string b = 2;
  int32 a = 1;
}
enum A {
  B = 1;
  A = 0;
}
"#;
        let out = sort_proto(input, SortKey::Number).unwrap();
        assert!(out.find("enum A").unwrap() < out.find("message Z").unwrap());
        assert!(out.find("int32 a = 1").unwrap() < out.find("string b = 2").unwrap());
    }
}

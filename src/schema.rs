use anyhow::{Context, Result};
use prost_reflect::{DescriptorPool, FieldDescriptor, Kind, MessageDescriptor};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub detail: String,
}

pub fn message_candidates(pool: &DescriptorPool, prefix: &str) -> Vec<Candidate> {
    sorted_candidates(pool.all_messages().filter_map(|message| {
        match_name(message.full_name(), prefix).map(|value| Candidate {
            value,
            detail: "message".into(),
        })
    }))
}

pub fn enum_candidates(pool: &DescriptorPool, prefix: &str) -> Vec<Candidate> {
    sorted_candidates(pool.all_enums().filter_map(|enm| {
        match_name(enm.full_name(), prefix).map(|value| Candidate {
            value,
            detail: "enum".into(),
        })
    }))
}

pub fn field_candidates(
    pool: &DescriptorPool,
    message: &str,
    prefix: &str,
) -> Result<Vec<Candidate>> {
    let descriptor = pool
        .get_message_by_name(message)
        .with_context(|| format!("message type {message:?} not found in descriptors"))?;
    Ok(field_candidates_for_message(
        &descriptor,
        prefix,
        "",
        CandidateMode::Field,
    ))
}

pub fn query_path_candidates(
    pool: &DescriptorPool,
    message: &str,
    prefix: &str,
) -> Result<Vec<Candidate>> {
    let root = pool
        .get_message_by_name(message)
        .with_context(|| format!("message type {message:?} not found in descriptors"))?;
    let normalized = if prefix.is_empty() { "$." } else { prefix };
    let (parent_path, field_prefix) = split_query_prefix(normalized);
    let descriptor = resolve_query_parent(root, parent_path)?;
    Ok(field_candidates_for_message(
        &descriptor,
        field_prefix,
        parent_path,
        CandidateMode::QueryPath,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateMode {
    Field,
    QueryPath,
}

fn field_candidates_for_message(
    descriptor: &MessageDescriptor,
    prefix: &str,
    path_prefix: &str,
    mode: CandidateMode,
) -> Vec<Candidate> {
    sorted_candidates(descriptor.fields().flat_map(|field| {
        let json_name = field.json_name();
        let proto_name = field.name();
        let name = if json_name.starts_with(prefix) {
            json_name
        } else if proto_name.starts_with(prefix) {
            proto_name
        } else {
            ""
        };

        let mut candidates = Vec::new();
        if !name.is_empty() {
            candidates.push(Candidate {
                value: join_path(path_prefix, name),
                detail: field_detail(&field),
            });
        }

        if mode == CandidateMode::QueryPath {
            for local in query_access_candidates(&field) {
                if local.starts_with(prefix) {
                    candidates.push(Candidate {
                        value: join_path(path_prefix, &local),
                        detail: field_detail(&field),
                    });
                }
            }
        }

        candidates
    }))
}

fn join_path(path_prefix: &str, name: &str) -> String {
    if path_prefix.is_empty() {
        name.to_owned()
    } else if path_prefix.ends_with('.') {
        format!("{path_prefix}{name}")
    } else {
        format!("{path_prefix}.{name}")
    }
}

fn query_access_candidates(field: &FieldDescriptor) -> Vec<String> {
    let name = field.json_name();
    if field.is_map() {
        vec![format!("{name}[\"<key>\"]")]
    } else if field.is_list() {
        vec![format!("{name}[*]"), format!("{name}[0]")]
    } else {
        Vec::new()
    }
}

fn field_detail(field: &FieldDescriptor) -> String {
    if field.is_map() {
        let (key, value) = map_key_value_labels(field);
        format!("map<{key}, {value}>")
    } else if field.is_list() {
        format!("repeated {}", field_kind_label(&field.kind()))
    } else {
        field_kind_label(&field.kind()).into()
    }
}

fn map_key_value_labels(field: &FieldDescriptor) -> (&'static str, &'static str) {
    let Kind::Message(entry) = field.kind() else {
        return ("unknown", "unknown");
    };
    let key = entry
        .get_field_by_name("key")
        .map(|field| field_kind_label(&field.kind()))
        .unwrap_or("unknown");
    let value = entry
        .get_field_by_name("value")
        .map(|field| field_kind_label(&field.kind()))
        .unwrap_or("unknown");
    (key, value)
}

fn resolve_query_parent(root: MessageDescriptor, parent_path: &str) -> Result<MessageDescriptor> {
    let mut descriptor = root;
    let path = parent_path.trim_start_matches('$').trim_start_matches('.');
    if path.is_empty() {
        return Ok(descriptor);
    }

    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        let field_name = segment_field_name(segment);
        if field_name == "*" {
            continue;
        }
        let field = descriptor
            .get_field_by_json_name(field_name)
            .or_else(|| descriptor.get_field_by_name(field_name))
            .with_context(|| {
                format!("field {field_name:?} not found while resolving {parent_path:?}")
            })?;
        descriptor = match field.kind() {
            Kind::Message(message) if field.is_map() => {
                map_value_message(&message).with_context(|| {
                    format!("map field {field_name:?} does not contain message values")
                })?
            }
            Kind::Message(message) => message,
            other => {
                anyhow::bail!(
                    "field {field_name:?} is {}, not a message",
                    field_kind_label(&other)
                )
            }
        };
    }

    Ok(descriptor)
}

fn segment_field_name(segment: &str) -> &str {
    segment.split_once('[').map_or(segment, |(name, _)| name)
}

fn map_value_message(entry: &MessageDescriptor) -> Option<MessageDescriptor> {
    let value = entry.get_field_by_name("value")?;
    match value.kind() {
        Kind::Message(message) => Some(message),
        _ => None,
    }
}

fn split_query_prefix(prefix: &str) -> (&str, &str) {
    let Some((parent, field)) = prefix.rsplit_once('.') else {
        return ("$.", prefix.trim_start_matches('$'));
    };
    let parent = if parent.is_empty() { "$" } else { parent };
    (parent, field)
}

fn match_name(name: &str, prefix: &str) -> Option<String> {
    if name.starts_with(prefix) {
        return Some(name.to_owned());
    }
    let short = name.rsplit('.').next().unwrap_or(name);
    short.starts_with(prefix).then(|| name.to_owned())
}

fn sorted_candidates(candidates: impl IntoIterator<Item = Candidate>) -> Vec<Candidate> {
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| a.value.cmp(&b.value).then(a.detail.cmp(&b.detail)));
    candidates.dedup_by(|a, b| a.value == b.value && a.detail == b.detail);
    candidates
}

fn field_kind_label(kind: &Kind) -> &'static str {
    match kind {
        Kind::Double => "double",
        Kind::Float => "float",
        Kind::Int32 => "int32",
        Kind::Int64 => "int64",
        Kind::Uint32 => "uint32",
        Kind::Uint64 => "uint64",
        Kind::Sint32 => "sint32",
        Kind::Sint64 => "sint64",
        Kind::Fixed32 => "fixed32",
        Kind::Fixed64 => "fixed64",
        Kind::Sfixed32 => "sfixed32",
        Kind::Sfixed64 => "sfixed64",
        Kind::Bool => "bool",
        Kind::String => "string",
        Kind::Bytes => "bytes",
        Kind::Message(_) => "message",
        Kind::Enum(_) => "enum",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use prost_reflect::DescriptorPool;
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MessageOptions,
        field_descriptor_proto::{Label, Type},
    };

    #[test]
    fn completes_nested_query_paths() {
        let pool = test_pool();
        let candidates = query_path_candidates(&pool, "demo.User", "$.profile.n").unwrap();
        assert_eq!(candidates[0].value, "$.profile.name");
    }

    #[test]
    fn completes_repeated_query_paths() {
        let pool = test_pool();
        let candidates = query_path_candidates(&pool, "demo.User", "$.profiles[").unwrap();
        let values = candidates
            .iter()
            .map(|candidate| candidate.value.as_str())
            .collect::<Vec<_>>();
        assert!(values.contains(&"$.profiles[*]"));
        assert!(values.contains(&"$.profiles[0]"));

        let nested = query_path_candidates(&pool, "demo.User", "$.profiles[*].n").unwrap();
        assert_eq!(nested[0].value, "$.profiles[*].name");
    }

    #[test]
    fn completes_map_query_paths() {
        let pool = test_pool();
        let candidates = query_path_candidates(&pool, "demo.User", "$.labels[").unwrap();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.value == "$.labels[\"<key>\"]")
        );

        let nested = query_path_candidates(&pool, "demo.User", "$.labels[\"foo\"].n").unwrap();
        assert_eq!(nested[0].value, "$.labels[\"foo\"].name");
    }

    fn test_pool() -> DescriptorPool {
        let profile = DescriptorProto {
            name: Some("Profile".into()),
            field: vec![FieldDescriptorProto {
                name: Some("name".into()),
                json_name: Some("name".into()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let labels_entry = DescriptorProto {
            name: Some("LabelsEntry".into()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("key".into()),
                    json_name: Some("key".into()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::String as i32),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("value".into()),
                    json_name: Some("value".into()),
                    number: Some(2),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some(".demo.Profile".into()),
                    ..Default::default()
                },
            ],
            options: Some(MessageOptions {
                map_entry: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let user = DescriptorProto {
            name: Some("User".into()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("profile".into()),
                    json_name: Some("profile".into()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some(".demo.Profile".into()),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("profiles".into()),
                    json_name: Some("profiles".into()),
                    number: Some(2),
                    label: Some(Label::Repeated as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some(".demo.Profile".into()),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("labels".into()),
                    json_name: Some("labels".into()),
                    number: Some(3),
                    label: Some(Label::Repeated as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some(".demo.User.LabelsEntry".into()),
                    ..Default::default()
                },
            ],
            nested_type: vec![labels_entry],
            ..Default::default()
        };
        let set = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("demo.proto".into()),
                package: Some("demo".into()),
                syntax: Some("proto3".into()),
                message_type: vec![profile, user],
                ..Default::default()
            }],
        };
        DescriptorPool::decode(set.encode_to_vec().as_slice()).unwrap()
    }
}

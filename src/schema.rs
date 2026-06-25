use anyhow::{Context, Result};
use prost_reflect::{DescriptorPool, Kind, MessageDescriptor};

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
    Ok(field_candidates_for_message(&descriptor, prefix, ""))
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
    ))
}

fn field_candidates_for_message(
    descriptor: &MessageDescriptor,
    prefix: &str,
    path_prefix: &str,
) -> Vec<Candidate> {
    sorted_candidates(descriptor.fields().filter_map(|field| {
        let json_name = field.json_name();
        let proto_name = field.name();
        let name = if json_name.starts_with(prefix) {
            json_name
        } else if proto_name.starts_with(prefix) {
            proto_name
        } else {
            return None;
        };
        let value = if path_prefix.is_empty() {
            name.to_owned()
        } else if path_prefix.ends_with('.') {
            format!("{path_prefix}{name}")
        } else {
            format!("{path_prefix}.{name}")
        };
        Some(Candidate {
            value,
            detail: field_kind_label(&field.kind()).into(),
        })
    }))
}

fn resolve_query_parent(root: MessageDescriptor, parent_path: &str) -> Result<MessageDescriptor> {
    let mut descriptor = root;
    let path = parent_path.trim_start_matches('$').trim_start_matches('.');
    if path.is_empty() {
        return Ok(descriptor);
    }

    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        let field = descriptor
            .get_field_by_json_name(segment)
            .or_else(|| descriptor.get_field_by_name(segment))
            .with_context(|| {
                format!("field {segment:?} not found while resolving {parent_path:?}")
            })?;
        descriptor = match field.kind() {
            Kind::Message(message) => message,
            other => {
                anyhow::bail!(
                    "field {segment:?} is {}, not a message",
                    field_kind_label(&other)
                )
            }
        };
    }

    Ok(descriptor)
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
        field_descriptor_proto::{Label, Type},
    };

    #[test]
    fn completes_nested_query_paths() {
        let pool = test_pool();
        let candidates = query_path_candidates(&pool, "demo.User", "$.profile.n").unwrap();
        assert_eq!(candidates[0].value, "$.profile.name");
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
        let user = DescriptorProto {
            name: Some("User".into()),
            field: vec![FieldDescriptorProto {
                name: Some("profile".into()),
                json_name: Some("profile".into()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".demo.Profile".into()),
                ..Default::default()
            }],
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

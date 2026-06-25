use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage};
use prost_types::FileDescriptorSet;
use serde_json::Value;

pub fn load_pool(
    descriptor_set: Option<&Path>,
    proto_files: &[PathBuf],
    includes: &[PathBuf],
) -> Result<DescriptorPool> {
    if let Some(path) = descriptor_set {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read descriptor set {}", path.display()))?;
        let fds = FileDescriptorSet::decode(bytes.as_slice())
            .with_context(|| format!("failed to decode descriptor set {}", path.display()))?;
        return DescriptorPool::from_file_descriptor_set(fds)
            .context("failed to load descriptor pool");
    }

    let mut include_paths = includes.to_vec();
    if include_paths.is_empty() {
        include_paths.push(PathBuf::from("."));
    }
    let fds =
        protox::compile(proto_files, include_paths).context("failed to compile proto files")?;
    DescriptorPool::from_file_descriptor_set(fds).context("failed to load descriptor pool")
}

pub fn decode_to_json(pool: &DescriptorPool, message: &str, bytes: &[u8]) -> Result<Value> {
    let descriptor = pool
        .get_message_by_name(message)
        .with_context(|| format!("message type {message:?} not found in descriptors"))?;
    let dynamic = DynamicMessage::decode(descriptor, bytes)
        .with_context(|| format!("failed to decode message {message}"))?;
    serde_json::to_value(&dynamic).context("failed to convert dynamic protobuf message to JSON")
}

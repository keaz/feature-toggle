use feature_toggle_backend::rest::ApiDoc;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use utoipa::OpenApi;

const OPENAPI_FILE: &str = "openapi.json";
const PROTO_DIR: &str = "proto";
const PROTO_FILE: &str = "evaluation.proto";
const PROTO_DESCRIPTOR_FILE: &str = "evaluation_descriptor.pb";
const MANIFEST_FILE: &str = "contract-hashes.json";

#[derive(Debug, Serialize)]
struct ArtifactHash {
    file: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct ProtoHashes {
    proto_file: String,
    proto_sha256: String,
    descriptor_file: String,
    descriptor_sha256: String,
}

#[derive(Debug, Serialize)]
struct ContractHashes {
    manifest_version: u8,
    openapi: ArtifactHash,
    protobuf: ProtoHashes,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = parse_output_dir(std::env::args().skip(1))?;
    export_contracts(&output_dir)?;
    println!("Contracts exported to {}", output_dir.display());
    Ok(())
}

fn parse_output_dir(
    mut args: impl Iterator<Item = String>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut output = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --output".to_string())?;
                output = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                println!(
                    "Usage: cargo run -p feature-toggle-backend --bin export-contracts -- [--output <dir>]"
                );
                std::process::exit(0);
            }
            _ => {
                return Err(format!("unknown argument: {arg}").into());
            }
        }
    }

    Ok(output.unwrap_or_else(default_output_dir))
}

fn default_output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contracts/generated")
}

fn export_contracts(output_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let proto_output_dir = output_dir.join(PROTO_DIR);
    fs::create_dir_all(&proto_output_dir)?;

    let openapi_json = canonicalize_json(serde_json::to_value(ApiDoc::openapi())?);
    let openapi_path = output_dir.join(OPENAPI_FILE);
    fs::write(&openapi_path, format_json(&openapi_json)?)?;

    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let proto_source_dir = crate_root.join("proto");
    let proto_source_path = proto_source_dir.join(PROTO_FILE);
    let proto_copy_path = proto_output_dir.join(PROTO_FILE);
    fs::copy(&proto_source_path, &proto_copy_path)?;

    let descriptor_path = proto_output_dir.join(PROTO_DESCRIPTOR_FILE);
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let status = Command::new(protoc)
        .arg(format!("--proto_path={}", proto_source_dir.display()))
        .arg(format!(
            "--descriptor_set_out={}",
            descriptor_path.display()
        ))
        .arg("--include_imports")
        .arg("--include_source_info")
        .arg(&proto_source_path)
        .status()?;

    if !status.success() {
        return Err("failed to generate protobuf descriptor set".into());
    }

    let hashes = ContractHashes {
        manifest_version: 1,
        openapi: ArtifactHash {
            file: OPENAPI_FILE.to_string(),
            sha256: sha256_hex(&openapi_path)?,
        },
        protobuf: ProtoHashes {
            proto_file: format!("{PROTO_DIR}/{PROTO_FILE}"),
            proto_sha256: sha256_hex(&proto_copy_path)?,
            descriptor_file: format!("{PROTO_DIR}/{PROTO_DESCRIPTOR_FILE}"),
            descriptor_sha256: sha256_hex(&descriptor_path)?,
        },
    };

    let manifest_path = output_dir.join(MANIFEST_FILE);
    fs::write(manifest_path, serde_json::to_string_pretty(&hashes)? + "\n")?;

    Ok(())
}

fn format_json(value: &Value) -> Result<String, Box<dyn std::error::Error>> {
    Ok(serde_json::to_string_pretty(value)? + "\n")
}

fn sha256_hex(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                sorted.insert(key, canonicalize_json(value));
            }

            let mut canonical_map = serde_json::Map::new();
            for (key, value) in sorted {
                canonical_map.insert(key, value);
            }
            Value::Object(canonical_map)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_json).collect()),
        other => other,
    }
}

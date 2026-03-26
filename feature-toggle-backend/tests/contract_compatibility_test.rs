use feature_toggle_backend::rest::ApiDoc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use utoipa::OpenApi;

const OPENAPI_FILE: &str = "openapi.json";
const PROTO_DIR: &str = "proto";
const PROTO_FILE: &str = "evaluation.proto";
const PROTO_DESCRIPTOR_FILE: &str = "evaluation_descriptor.pb";
const MANIFEST_FILE: &str = "contract-hashes.json";

#[test]
fn contract_hashes_match_baseline() {
    let temp_dir = create_temp_contract_dir().expect("failed to create temp contract dir");
    export_contracts(&temp_dir).expect("failed to export contracts for compatibility check");

    let baseline_path = crate_root().join("contracts/baseline").join(MANIFEST_FILE);
    assert!(
        baseline_path.exists(),
        "missing baseline manifest at {}",
        baseline_path.display()
    );

    let baseline = read_json(&baseline_path).expect("failed to read baseline contract hashes");
    let current = read_json(&temp_dir.join(MANIFEST_FILE))
        .expect("failed to read generated contract hashes for compatibility check");

    assert_eq!(
        baseline, current,
        "contract compatibility check failed.\n\
         If this change is intentional, update baseline:\n\
         1) ./scripts/export-contracts.sh\n\
         2) cp feature-toggle-backend/contracts/generated/contract-hashes.json feature-toggle-backend/contracts/baseline/contract-hashes.json"
    );

    let _ = fs::remove_dir_all(temp_dir);
}

fn create_temp_contract_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "feature-toggle-contract-compat-{}-{now}",
        std::process::id()
    ));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn export_contracts(output_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let proto_output_dir = output_dir.join(PROTO_DIR);
    fs::create_dir_all(&proto_output_dir)?;

    let openapi_json = canonicalize_json(serde_json::to_value(ApiDoc::openapi())?);
    let openapi_path = output_dir.join(OPENAPI_FILE);
    fs::write(
        &openapi_path,
        serde_json::to_string_pretty(&openapi_json)? + "\n",
    )?;

    let root = crate_root();
    let proto_source_dir = root.join("proto");
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
        return Err("failed to generate protobuf descriptor set during compatibility check".into());
    }

    let manifest = serde_json::json!({
        "manifest_version": 1,
        "openapi": {
            "file": OPENAPI_FILE,
            "sha256": sha256_hex(&openapi_path)?,
        },
        "protobuf": {
            "proto_file": format!("{PROTO_DIR}/{PROTO_FILE}"),
            "proto_sha256": sha256_hex(&proto_copy_path)?,
            "descriptor_file": format!("{PROTO_DIR}/{PROTO_DESCRIPTOR_FILE}"),
            "descriptor_sha256": sha256_hex(&descriptor_path)?,
        }
    });

    fs::write(
        output_dir.join(MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;

    Ok(())
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
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

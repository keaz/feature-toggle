fn main() {
    // Use vendored protoc to avoid external dependency
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("protoc not found");
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    tonic_build::configure()
        .build_server(true)
        .compile_protos(&["proto/evaluation.proto"], &["proto"]) // include path
        .expect("Failed to compile protos");
}

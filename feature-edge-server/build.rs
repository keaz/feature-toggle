fn main() {
    let proto_dir = "../feature-toggle-backend/proto";
    let proto = format!("{}/evaluation.proto", proto_dir);

    // Use vendored protoc for portability
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("protoc not found");
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&[proto.as_str()], &[proto_dir])
        .expect("failed to compile protos");
}

fn main() {
    unsafe {
        std::env::set_var(
            "PROTOC",
            "E:/DELL/Documents/Codes/vcpkg/installed/x64-windows/tools/protobuf/protoc.exe",
        );
    }
    println!("cargo:rerun-if-changed=../proto/analysis.proto");

    let mut config = prost_build::Config::new();
    config
        .compile_protos(&["../proto/analysis.proto"], &["../proto"])
        .expect("failed to compile proto/analysis.proto");
}

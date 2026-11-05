fn main() {
    // PROTOC 优先级：外部环境变量（含 PATH 查找）> 本机 vcpkg 兜底。
    // 不能无条件覆盖：CI 的 protoc 由系统包/下载安装，硬编码路径会让其失效。
    if std::env::var_os("PROTOC").is_none() {
        let vcpkg_protoc =
            "E:/DELL/Documents/Codes/vcpkg/installed/x64-windows/tools/protobuf/protoc.exe";
        if std::path::Path::new(vcpkg_protoc).is_file() {
            unsafe {
                std::env::set_var("PROTOC", vcpkg_protoc);
            }
        }
    }
    println!("cargo:rerun-if-changed=../proto/analysis.proto");
    println!("cargo:rerun-if-env-changed=PROTOC");

    let mut config = prost_build::Config::new();
    config
        .compile_protos(&["../proto/analysis.proto"], &["../proto"])
        .expect("failed to compile proto/analysis.proto");
}

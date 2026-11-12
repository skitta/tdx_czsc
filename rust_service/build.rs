fn main() {
    // PROTOC 优先级：外部环境变量（含 PATH 查找）> 本机 vcpkg 兜底。
    // 不能无条件覆盖：CI 的 protoc 由系统包/下载安装，硬编码路径会让其失效。
    if std::env::var_os("PROTOC").is_none() {
        let vcpkg_protoc =
            "E:/DELL/Documents/Codes/vcpkg/installed/x64-windows/tools/protobuf/protoc.exe";
        if std::path::Path::new(vcpkg_protoc).is_file() {
            // 唯一允许的 unsafe：edition 2024 起 `set_var` 因线程安全原因变成
            // unsafe，构建脚本是单线程且在任何子进程 spawn 之前执行，此处安全。
            #[allow(unsafe_code)]
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

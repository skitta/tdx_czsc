fn main() {
    // 各系统本机专用配置放在仓库根的 .build/<os>.env（已 gitignore），格式为 KEY=VALUE。
    // 环境变量优先于文件，方便 CI 覆盖。
    load_local_env();

    // 受限环境（如 Zed 的沙箱）下系统临时目录不可写，prost-build 会创建临时目录失败；
    // 探测失败时改用 cargo 已创建好的 OUT_DIR 作为临时目录。
    if !temp_dir_writable() {
        if let Some(out_dir) = std::env::var_os("OUT_DIR") {
            println!(
                "cargo:warning=系统临时目录不可写，构建临时文件改用 {}（随 cargo clean 清理）",
                std::path::Path::new(&out_dir).display()
            );
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("TMP", &out_dir);
                std::env::set_var("TEMP", &out_dir);
                std::env::set_var("TMPDIR", &out_dir);
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

fn temp_dir_writable() -> bool {
    let probe = std::env::temp_dir().join(format!("tdx-czsc-probe-{}", std::process::id()));
    let writable = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    writable
}

fn load_local_env() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.build")
        .join(format!("{}.env", std::env::consts::OS));
    println!("cargo:rerun-if-changed={}", path.display());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        // 构建脚本单线程且在任何子进程 spawn 之前执行，set_var 此处安全。
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

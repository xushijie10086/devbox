fn main() {
    let msvc_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");

    if !msvc_windows {
        tauri_build::build();
        return;
    }

    // Tauri 默认只把「依赖 Common Controls v6」的清单嵌进正式程序的资源里，
    // 单元测试的可执行文件没有，tao / muda 等窗口库要用的入口在 v6 才有，
    // 测试程序一加载就报 STATUS_ENTRYPOINT_NOT_FOUND。
    // 这里不再让 tauri-build 放清单，改由链接器给所有目标（含测试程序）嵌入同一份。
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("tauri-build 失败");
}

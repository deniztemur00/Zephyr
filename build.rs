// Links the app icon (assets/zephyr.res, made by scripts/make_assets.py) into zephyr.exe.
fn main() {
    let icon = "assets/zephyr.res";
    println!("cargo:rerun-if-changed={icon}");
    let target = |key| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        let dir = target("CARGO_MANIFEST_DIR");
        println!("cargo:rustc-link-arg-bins={dir}/{icon}");
    }
}

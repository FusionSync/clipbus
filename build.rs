fn main() {
    println!("cargo:rerun-if-env-changed=CLIPBUS_SONAME_MAJOR");
    if cfg!(target_os = "linux") {
        let soname_major = std::env::var("CLIPBUS_SONAME_MAJOR").unwrap_or_else(|_| "0".to_owned());
        if soname_major.parse::<u32>().is_err() {
            panic!("CLIPBUS_SONAME_MAJOR must be a non-negative integer");
        }
        println!("cargo:rustc-link-arg-cdylib=-Wl,-soname,libclipbus.so.{soname_major}");
    }
}

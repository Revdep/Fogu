//! Validate the capability independently of runtime opt-outs.
fn main() {
    println!("cargo:rerun-if-env-changed=FOGU_UPDATER");
    if let Ok(value) = std::env::var("FOGU_UPDATER") {
        assert!(
            matches!(value.as_str(), "manual" | "automatic"),
            "invalid FOGU_UPDATER"
        );
    }
}

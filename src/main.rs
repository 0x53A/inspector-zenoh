#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    inspector_zenoh::run()
}

#[cfg(target_arch = "wasm32")]
fn main() {}

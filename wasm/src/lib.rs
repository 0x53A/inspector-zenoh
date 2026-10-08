use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    inspector_zenoh::start(canvas, "./pkg/inspector_zenoh_wasm.js").await
}

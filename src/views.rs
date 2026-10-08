use egui::{Color32, RichText, Ui};
use hiroz::dynamic::{DynamicMessage, DynamicValue, FieldType};

pub fn type_label(ty: &FieldType) -> String {
    match ty {
        FieldType::Message(schema) => schema.type_name.clone(),
        FieldType::Array(inner, n) => format!("{}[{n}]", type_label(inner)),
        FieldType::Sequence(inner) => format!("{}[]", type_label(inner)),
        FieldType::BoundedSequence(inner, n) => format!("{}[<={n}]", type_label(inner)),
        other => format!("{other:?}"),
    }
}

pub fn fields(ui: &mut Ui, message: &DynamicMessage) {
    for (name, value) in message.iter() {
        value_ui(ui, name, value);
    }
}

fn scalar(ui: &mut Ui, name: &str, value: impl ToString) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).color(Color32::from_rgb(160, 175, 191)));
        ui.label(
            RichText::new(value.to_string())
                .monospace()
                .color(Color32::from_rgb(90, 226, 172)),
        );
    });
}

fn value_ui(ui: &mut Ui, name: &str, value: &DynamicValue) {
    match value {
        DynamicValue::Message(message) => {
            egui::CollapsingHeader::new(name)
                .id_salt(name)
                .default_open(true)
                .show(ui, |ui| fields(ui, message));
        }
        DynamicValue::Array(values) => {
            egui::CollapsingHeader::new(format!("{name} [{}]", values.len()))
                .id_salt(name)
                .show(ui, |ui| {
                    // Chunked branches bound widget creation even for large sensor arrays.
                    if values.len() <= 64 {
                        for (i, value) in values.iter().enumerate() {
                            value_ui(ui, &i.to_string(), value);
                        }
                    } else {
                        for (chunk, values) in values.chunks(64).enumerate() {
                            egui::CollapsingHeader::new(format!(
                                "{}–{}",
                                chunk * 64,
                                chunk * 64 + values.len() - 1
                            ))
                            .show(ui, |ui| {
                                for (i, value) in values.iter().enumerate() {
                                    value_ui(ui, &(chunk * 64 + i).to_string(), value);
                                }
                            });
                        }
                    }
                });
        }
        DynamicValue::Bytes(bytes) => {
            egui::CollapsingHeader::new(format!("{name} [{} bytes]", bytes.len()))
                .id_salt(name)
                .show(ui, |ui| hex_view(ui, bytes));
        }
        DynamicValue::Bool(v) => scalar(ui, name, v),
        DynamicValue::Int8(v) => scalar(ui, name, v),
        DynamicValue::Uint8(v) => scalar(ui, name, v),
        DynamicValue::Int16(v) => scalar(ui, name, v),
        DynamicValue::Uint16(v) => scalar(ui, name, v),
        DynamicValue::Int32(v) => scalar(ui, name, v),
        DynamicValue::Uint32(v) => scalar(ui, name, v),
        DynamicValue::Int64(v) => scalar(ui, name, v),
        DynamicValue::Uint64(v) => scalar(ui, name, v),
        DynamicValue::Float32(v) => scalar(ui, name, v),
        DynamicValue::Float64(v) => scalar(ui, name, v),
        DynamicValue::String(v) => {
            let end = v.char_indices().nth(1024).map_or(v.len(), |(i, _)| i);
            scalar(
                ui,
                name,
                format!("{:?}{}", &v[..end], if end < v.len() { "…" } else { "" }),
            );
        }
    }
}

pub fn hex_view(ui: &mut Ui, bytes: &[u8]) {
    for (i, chunk) in bytes[..bytes.len().min(1024)].chunks(16).enumerate() {
        let hex = chunk
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        ui.monospace(format!("{:04x}  {hex}", i * 16));
    }
    if bytes.len() > 1024 {
        ui.weak(format!(
            "Preview: 1024 of {} bytes. Full payload retained.",
            bytes.len()
        ));
    }
}

/// Renderer overrides are independent of subscription/decoding. The full field tree stays available.
pub fn custom(ui: &mut Ui, message: &DynamicMessage) -> bool {
    match message.schema().type_name.as_str() {
        "geometry_msgs/msg/Twist" => {
            ui.heading("Velocity");
            for (label, path, unit) in
                [("Linear", "linear", "m/s"), ("Angular", "angular", "rad/s")]
            {
                ui.strong(format!("{label} · {unit}"));
                ui.horizontal(|ui| {
                    for axis in ["x", "y", "z"] {
                        if let Ok(value) = message.get::<f64>(&format!("{path}.{axis}")) {
                            ui.monospace(format!("{axis} {value:8.3}   "));
                        }
                    }
                });
            }
            true
        }
        "sensor_msgs/msg/PointCloud2" => {
            ui.heading("Point cloud");
            for name in ["width", "height", "point_step", "row_step"] {
                if let Ok(v) = message.get::<u32>(name) {
                    scalar(ui, name, v);
                }
            }
            ui.weak("Expand Fields for point layout and data bytes.");
            true
        }
        _ => false,
    }
}

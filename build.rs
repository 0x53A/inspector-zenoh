use std::{collections::HashSet, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    let mut packages = hiroz_codegen::discover_bundled_packages(false)?;
    // The bundled test corpora include intentionally incomplete cross-package
    // definitions. Only ship actual interface packages in the inspector.
    packages.retain(|p| {
        !p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("test_")
    });
    let manifest = out.join("schemas.json");
    let generator = hiroz_codegen::MessageGenerator::new(hiroz_codegen::GeneratorConfig {
        generate_cdr: true,
        generate_protobuf: false,
        generate_type_info: true,
        is_humble: false,
        output_dir: out.clone(),
        external_crate: None,
        local_packages: HashSet::new(),
        json_out: Some(manifest.clone()),
    });
    generator.generate_from_msg_files(&packages.iter().map(|p| p.as_path()).collect::<Vec<_>>())?;

    // Generate registration alongside bindings: adding a .msg needs no handwritten dispatch arm.
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
    let mut registry =
        String::from("pub fn embedded_registry() -> Registry { let mut r = Registry::default();\n");
    for msg in json["messages"]
        .as_array()
        .expect("codegen message manifest")
    {
        let package = msg["package"].as_str().unwrap();
        let name = msg["name"].as_str().unwrap();
        registry.push_str(&format!(
            "r.register::<crate::messages::ros::{package}::{name}>();\n"
        ));
    }
    // Action feedback is a normal published topic, wrapped in goal_id + feedback.
    for action in json["actions"].as_array().expect("codegen actions") {
        if let Some(feedback) = action["feedback"].as_object() {
            let package = action["package"].as_str().unwrap();
            let name = action["name"].as_str().unwrap();
            let feedback_name = feedback["name"].as_str().unwrap();
            registry.push_str(&format!(
                "r.register_action_feedback::<crate::messages::ros::{package}::{feedback_name}>(\"{package}::action::dds_::{name}_FeedbackMessage_\");\n"
            ));
        }
    }
    registry.push_str("r }\n");
    std::fs::write(out.join("registry.rs"), registry)?;

    // Upstream emits repeated defaults for fixed arrays; nested messages aren't Copy.
    let generated = out.join("generated.rs");
    let code = std::fs::read_to_string(&generated)?;
    // HIRoZ currently marks some padded generated results as CdrPlain/Pod.
    // This inspector uses field-wise CDR, so disable these optional zero-copy
    // promises for all generated types rather than accepting invalid layouts.
    let code = code.replace(
        "derive(Copy, ::bytemuck::Pod, ::bytemuck::Zeroable)",
        "derive(Copy)",
    );
    // Select the generator's portable field-wise sequence implementations too.
    // These cfgs only guard CdrPlain and the matching bulk-copy fast paths.
    let code = code
        .replace("#[cfg(target_endian = \"little\")]", "#[cfg(any())]")
        .replace("#[cfg(not(target_endian = \"little\"))]", "#[cfg(all())]");
    let fixed = code
        .lines()
        .map(|line| {
            if let Some(count) = line
                .trim()
                .strip_prefix("let mut __arr = [Default::default(); ")
                .and_then(|s| s.strip_suffix("];"))
            {
                format!(
                    "let mut __arr = core::array::from_fn::<_, {count}, _>(|_| Default::default());"
                )
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(generated, fixed)?;
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}

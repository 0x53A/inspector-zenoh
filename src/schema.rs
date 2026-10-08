use hiroz::{
    MessageTypeInfo,
    dynamic::{DynamicMessage, FieldType, MessageSchema},
};
use std::{collections::BTreeMap, sync::Arc};

/// Message schemas available to the inspector, keyed by the ROS type name
/// advertised through rmw_zenoh discovery.
#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<String, Arc<MessageSchema>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a generated HIRoZ message type.
    pub fn register<T: MessageTypeInfo>(&mut self) {
        if let Some(schema) = T::message_schema() {
            let mut schema = (*schema).clone();
            schema.type_hash = Some(T::type_hash().to_string());
            self.entries.insert(T::type_name().into(), Arc::new(schema));
        }
    }
    /// Register the DDS feedback envelope used by a ROS action.
    pub fn register_action_feedback<T: MessageTypeInfo>(&mut self, name: &str) {
        use crate::messages::ros::unique_identifier_msgs::UUID;
        // HIRoZ's builder currently only accepts /msg/ names; action topics
        // use the same CDR field schema with an /action/ identity.
        let (package, message) = name
            .split_once("::action::dds_::")
            .expect("generated action name");
        let message = message.strip_suffix('_').expect("DDS action suffix");
        let mut feedback = (*T::message_schema().expect("action feedback schema")).clone();
        feedback.name = message
            .strip_suffix("Message")
            .expect("feedback envelope name")
            .into();
        feedback.type_name = format!("{package}/action/{}", feedback.name);
        let mut schema = MessageSchema {
            type_name: format!("{package}/action/{message}"),
            package: package.into(),
            name: message.into(),
            fields: vec![
                hiroz::dynamic::FieldSchema::new(
                    "goal_id",
                    FieldType::Message(UUID::message_schema().expect("UUID schema")),
                ),
                hiroz::dynamic::FieldSchema::new(
                    "feedback",
                    FieldType::Message(Arc::new(feedback)),
                ),
            ],
            type_hash: None,
        };
        // Upstream's action hash omits transitive feedback dependencies. Compute
        // RIHS01 from the complete schema, including the ROS /action/ names.
        use hiroz::dynamic::MessageSchemaTypeDescription;
        schema.type_hash = Some(
            schema
                .compute_type_hash()
                .expect("action type hash")
                .to_string(),
        );
        self.entries.insert(name.to_owned(), Arc::new(schema));
    }
    /// Merge another registry. Entries from `other` replace entries with the
    /// same advertised type name, allowing deployment-specific definitions to
    /// override the built-in set.
    pub fn extend(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn get(&self, name: &str, hash: &str) -> Result<Arc<MessageSchema>, String> {
        let schema = self
            .entries
            .get(name)
            .ok_or_else(|| format!("No embedded schema for {name}"))?;
        if schema.type_hash.as_deref() != Some(hash) {
            return Err(format!(
                "Schema mismatch: embedded {}, advertised {hash}",
                schema.type_hash.as_deref().unwrap_or("unknown")
            ));
        }
        Ok(Arc::clone(schema))
    }
    pub fn decode(&self, name: &str, hash: &str, payload: &[u8]) -> Result<DynamicMessage, String> {
        let schema = self.get(name, hash)?;
        Self::decode_with_schema(&schema, payload)
    }

    /// Decode a payload with a schema obtained at runtime through REP-2016.
    pub fn decode_with_schema(
        schema: &Arc<MessageSchema>,
        payload: &[u8],
    ) -> Result<DynamicMessage, String> {
        validate_payload(payload, schema)?;
        DynamicMessage::from_cdr(payload, schema).map_err(|e| format!("CDR decode failed: {e}"))
    }
}
include!(concat!(env!("OUT_DIR"), "/registry.rs"));

// Validate without allocating before upstream builds a DynamicValue tree. A small
// corrupt packet can otherwise advertise a huge sequence and exhaust WASM memory.
fn validate_payload(payload: &[u8], schema: &MessageSchema) -> Result<(), String> {
    if payload.len() > crate::transport::MAX_PAYLOAD {
        return Err("Payload exceeds inspection limit".into());
    }
    if payload.get(..2) != Some(&[0, 1]) || payload.len() < 4 {
        return Err("Expected little-endian CDR encapsulation".into());
    }
    let mut reader = hiroz_cdr::CdrReader::<hiroz_cdr::LittleEndian>::new(&payload[4..]);
    let mut budget = 100_000usize;
    for field in &schema.fields {
        validate_field(&mut reader, &field.field_type, &mut budget, 0)?;
    }
    Ok(())
}

fn validate_field(
    reader: &mut hiroz_cdr::CdrReader<hiroz_cdr::LittleEndian>,
    ty: &FieldType,
    budget: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 || *budget == 0 {
        return Err(
            "Message exceeds the 100,000-value / 64-level inspection limit; use Raw".into(),
        );
    }
    *budget -= 1;
    let err = |e: hiroz_cdr::Error| format!("Invalid CDR: {e}");
    match ty {
        FieldType::Message(schema) => {
            for field in &schema.fields {
                validate_field(reader, &field.field_type, budget, depth + 1)?;
            }
        }
        FieldType::String | FieldType::BoundedString(_) => {
            let text = reader.read_str().map_err(err)?;
            if let FieldType::BoundedString(max) = ty
                && text.len() > *max
            {
                return Err("String exceeds schema bound".into());
            }
        }
        FieldType::Array(inner, _)
        | FieldType::Sequence(inner)
        | FieldType::BoundedSequence(inner, _) => {
            let len = if let FieldType::Array(_, len) = ty {
                *len
            } else {
                reader.read_sequence_length().map_err(err)?
            };
            if let FieldType::BoundedSequence(_, max) = ty
                && len > *max
            {
                return Err("Sequence exceeds schema bound".into());
            }
            // Unbounded byte and uint8 sequences use the compact Bytes variant upstream.
            if matches!(ty, FieldType::Sequence(_))
                && matches!(**inner, FieldType::Byte | FieldType::Uint8)
            {
                reader.read_bytes(len).map_err(err)?;
            } else {
                if len > *budget {
                    return Err(
                        "Message exceeds the 100,000-value inspection limit; use Raw".into(),
                    );
                }
                for _ in 0..len {
                    validate_field(reader, inner, budget, depth + 1)?;
                }
            }
        }
        _ => {
            reader.align(ty.alignment()).map_err(err)?;
            reader
                .read_bytes(ty.fixed_size().expect("primitive size"))
                .map_err(err)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::ros::geometry_msgs::Twist;

    #[test]
    fn rejects_truncated_cdr_and_keeps_numeric_structure() {
        let registry = embedded_registry();
        let schema = registry
            .get(Twist::type_name(), &Twist::type_hash().to_string())
            .unwrap();
        let mut msg = DynamicMessage::new(&schema);
        msg.set("linear.x", 3.5f64).unwrap();
        let bytes = msg.to_cdr().unwrap();
        let decoded = registry
            .decode(Twist::type_name(), &Twist::type_hash().to_string(), &bytes)
            .unwrap();
        assert_eq!(decoded.get::<f64>("linear.x").unwrap(), 3.5);
        assert!(
            registry
                .decode(
                    Twist::type_name(),
                    &Twist::type_hash().to_string(),
                    &bytes[..8]
                )
                .is_err()
        );
    }

    #[test]
    fn rejects_large_sequence_before_dynamic_allocation() {
        let schema = MessageSchema::builder("test/msg/Sequence")
            .field("values", FieldType::Sequence(Box::new(FieldType::Float64)))
            .build()
            .unwrap();
        let mut bytes = vec![0, 1, 0, 0];
        bytes.extend_from_slice(&50_000_000u32.to_le_bytes());
        assert!(
            validate_payload(&bytes, &schema)
                .unwrap_err()
                .contains("inspection limit")
        );
    }
}

//! Isolated local router + traffic for browser development; never connects to the rover.
use hiroz_native::{MessageTypeInfo, dynamic::DynamicMessage};
use hiroz_protocol_native::{EndpointEntity, EndpointKind, KeyExprFormat, NodeEntity, TypeInfo};
use inspector_zenoh::{
    messages::ros::{geometry_msgs::Twist, sensor_msgs::JointState, std_msgs::String as StringMsg},
    schema::embedded_registry,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"router\"")?;
    config.insert_json5(
        "listen/endpoints",
        "[\"tcp/127.0.0.1:17447\",\"ws/127.0.0.1:17448\"]",
    )?;
    config.insert_json5("scouting/multicast/enabled", "false")?;
    config.insert_json5("scouting/gossip/enabled", "false")?;
    let session = zenoh::open(config).await?;
    let format = KeyExprFormat::RmwZenoh;
    let node = NodeEntity::new(
        0,
        session.zid(),
        0,
        "inspector_fixture".into(),
        "/demo".into(),
        String::new(),
    );
    let _node = session
        .liveliness()
        .declare_token(format.node_liveliness_key_expr(&node)?.to_string())
        .await?;
    let registry = embedded_registry();
    let definitions = [
        ("/demo/velocity", Twist::type_name(), Twist::type_hash()),
        (
            "/demo/joints",
            JointState::type_name(),
            JointState::type_hash(),
        ),
        (
            "/demo/status",
            StringMsg::type_name(),
            StringMsg::type_hash(),
        ),
    ];
    let mut tokens = Vec::new();
    let mut publishers = Vec::new();
    let mut messages = Vec::new();
    for (i, (topic, name, hash)) in definitions.iter().enumerate() {
        let endpoint = EndpointEntity {
            id: i + 1,
            node: Some(NodeEntity {
                domain_id: [0, 17, 42][i],
                ..node.clone()
            }),
            kind: EndpointKind::Publisher,
            topic: topic.to_string(),
            type_info: Some(TypeInfo::new(name, hash.clone())),
            qos: Default::default(),
        };
        tokens.push(
            session
                .liveliness()
                .declare_token(
                    format
                        .liveliness_key_expr(&endpoint, &session.zid())?
                        .to_string(),
                )
                .await?,
        );
        publishers.push(
            session
                .declare_publisher(format.topic_key_expr(&endpoint)?.to_string())
                .await?,
        );
        messages.push(DynamicMessage::new(&registry.get(name, &hash.to_string())?));
    }
    messages[1].set("name", vec!["shoulder".to_owned(), "elbow".to_owned()])?;
    messages[1].set("position", vec![0.25f64, -0.5])?;
    messages[2].set("data", "inspector fixture".to_owned())?;
    println!(
        "Local fixture: ws/127.0.0.1:17448, domains 0, 17, 42. Three topics at 10 Hz. Ctrl-C to stop."
    );
    let mut tick = 0u64;
    loop {
        messages[0].set("linear.x", (tick as f64 / 30.0).sin())?;
        messages[0].set("angular.z", 0.125f64)?;
        for (publisher, message) in publishers.iter().zip(&messages) {
            publisher.put(message.to_cdr()?).await?;
        }
        tick += 1;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

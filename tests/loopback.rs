#![recursion_limit = "256"]

use hiroz_native::{
    Builder, MessageTypeInfo,
    dynamic::{DynamicMessage, FieldType, MessageSchema},
};
use hiroz_protocol_native::{
    EndpointEntity, EndpointKind, Entity, KeyExprFormat, NodeEntity, TypeInfo,
};
use inspector_zenoh::{
    messages::ros::geometry_msgs::Twist, schema::embedded_registry, transport::Connection,
};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn discovers_decodes_switches_and_disconnects_over_websocket() {
    check_session("ws/127.0.0.1:0").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn discovers_decodes_switches_and_disconnects_over_tcp() {
    check_session("tcp/127.0.0.1:0").await;
}

async fn check_session(listener: &str) {
    let listener = local_listener(listener);
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"router\"").unwrap();
    config
        .insert_json5(
            "listen/endpoints",
            &serde_json::to_string(&[&listener]).unwrap(),
        )
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    config
        .insert_json5("scouting/gossip/enabled", "false")
        .unwrap();
    let router = zenoh::open(config).await.unwrap();
    let address = listener;
    let node = NodeEntity::new(
        17,
        router.zid(),
        0,
        "fixture".into(),
        "/".into(),
        String::new(),
    );
    let e = EndpointEntity {
        id: 1,
        node: Some(node),
        kind: EndpointKind::Publisher,
        topic: "/test/twist".into(),
        type_info: Some(TypeInfo::new(Twist::type_name(), Twist::type_hash())),
        qos: Default::default(),
    };
    let format = KeyExprFormat::RmwZenoh;
    let token = router
        .liveliness()
        .declare_token(
            format
                .liveliness_key_expr(&e, &router.zid())
                .unwrap()
                .to_string(),
        )
        .await
        .unwrap();
    let publisher = router
        .declare_publisher(format.topic_key_expr(&e).unwrap().to_string())
        .await
        .unwrap();
    let mut other = e.clone();
    other.node.as_mut().unwrap().domain_id = 42;
    let other_token = router
        .liveliness()
        .declare_token(
            format
                .liveliness_key_expr(&other, &router.zid())
                .unwrap()
                .to_string(),
        )
        .await
        .unwrap();
    let other_publisher = router
        .declare_publisher(format.topic_key_expr(&other).unwrap().to_string())
        .await
        .unwrap();
    let quiet_node = NodeEntity::new(
        99,
        router.zid(),
        5,
        "quiet".into(),
        "/".into(),
        String::new(),
    );
    let quiet_token = router
        .liveliness()
        .declare_token(
            format
                .node_liveliness_key_expr(&quiet_node)
                .unwrap()
                .to_string(),
        )
        .await
        .unwrap();
    let connection = Connection::open(address);
    let deadline = Instant::now() + Duration::from_secs(10);
    let topic = loop {
        if let Some(topic) = connection
            .snapshot
            .lock()
            .unwrap()
            .topics
            .iter()
            .find(|t| t.domain == 17)
            .cloned()
        {
            break topic;
        }
        assert!(
            Instant::now() < deadline,
            "Discovery timed out: {}",
            connection.snapshot.lock().unwrap().status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(topic.key.starts_with("17/"));
    connection.select(Some(topic.clone()));
    let registry = embedded_registry();
    let mut message = DynamicMessage::new(
        &registry
            .get(Twist::type_name(), &Twist::type_hash().to_string())
            .unwrap(),
    );
    message.set("linear.x", 1.25f64).unwrap();
    let bytes = message.to_cdr().unwrap();
    let sample = loop {
        publisher.put(bytes.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        if let Some(sample) = connection.snapshot.lock().unwrap().latest.clone() {
            break sample;
        }
        assert!(Instant::now() < deadline, "Sample timed out");
    };
    let decoded = registry
        .decode(&topic.type_name, &topic.hash, &sample.bytes)
        .unwrap();
    assert_eq!(decoded.get::<f64>("linear.x").unwrap(), 1.25);
    while connection.snapshot.lock().unwrap().domains != vec![17, 42, 99] {
        assert!(
            Instant::now() < deadline,
            "Cross-domain discovery timed out"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let other_topic = connection
        .snapshot
        .lock()
        .unwrap()
        .topics
        .iter()
        .find(|t| t.domain == 42)
        .unwrap()
        .clone();
    assert_eq!(topic.name, other_topic.name);
    assert_ne!(topic.key, other_topic.key);
    let announcements = router
        .liveliness()
        .declare_subscriber("@ros2_lv/42/**")
        .history(true)
        .await
        .unwrap();
    connection.select(Some(other_topic.clone()));
    message.set("linear.x", 42.0f64).unwrap();
    let other_bytes = message.to_cdr().unwrap();
    loop {
        publisher.put(bytes.clone()).await.unwrap();
        other_publisher.put(other_bytes.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let snapshot = connection.snapshot.lock().unwrap();
        if snapshot.selected.as_deref() == Some(&other_topic.key)
            && let Some(sample) = &snapshot.latest
        {
            let decoded = registry
                .decode(&other_topic.type_name, &other_topic.hash, &sample.bytes)
                .unwrap();
            assert_eq!(decoded.get::<f64>("linear.x").unwrap(), 42.0);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cross-domain selection timed out"
        );
    }
    loop {
        let announcement = tokio::time::timeout(Duration::from_secs(3), announcements.recv_async())
            .await
            .unwrap()
            .unwrap();
        if let Ok(Entity::Endpoint(endpoint)) = format.parse_liveliness(announcement.key_expr())
            && endpoint.kind == EndpointKind::Subscription
            && let Some(node) = endpoint.node
            && node.name == "inspector_zenoh"
        {
            assert_eq!(node.domain_id, 42);
            assert_eq!(endpoint.topic, other_topic.name);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Subscriber announcement timed out"
        );
    }
    drop(announcements);
    connection.select(None);
    while connection.snapshot.lock().unwrap().selected.is_some() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    publisher.put(bytes).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(connection.snapshot.lock().unwrap().latest.is_none());
    token.undeclare().await.unwrap();
    other_token.undeclare().await.unwrap();
    quiet_token.undeclare().await.unwrap();
    while !connection.snapshot.lock().unwrap().domains.is_empty() {
        assert!(Instant::now() < deadline, "Endpoint removal timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let state = connection.snapshot.clone();
    drop(connection);
    while state.lock().unwrap().connected {
        assert!(Instant::now() < deadline, "Disconnect timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    router.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reflects_and_decodes_an_uncompiled_message_schema() {
    let address = local_listener("tcp/127.0.0.1:0");
    let mut router_config = zenoh::Config::default();
    router_config.insert_json5("mode", "\"router\"").unwrap();
    router_config
        .insert_json5(
            "listen/endpoints",
            &serde_json::to_string(&[&address]).unwrap(),
        )
        .unwrap();
    router_config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    router_config
        .insert_json5("scouting/gossip/enabled", "false")
        .unwrap();
    let router = zenoh::open(router_config).await.unwrap();

    let publisher_context = hiroz_native::context::ZContextBuilder::default()
        .with_domain_id(88)
        .with_mode("client")
        .with_connect_endpoints([address.clone()])
        .with_listen_endpoints(std::iter::empty::<String>())
        .disable_multicast_scouting()
        .build()
        .unwrap();
    let publisher_node = publisher_context
        .create_node("runtime_schema_source")
        .with_type_description_service()
        .without_parameters()
        .build()
        .unwrap();
    let schema = MessageSchema::builder("inspector_fixture/msg/Telemetry")
        .field("label", FieldType::String)
        .field("voltage", FieldType::Float64)
        .build()
        .unwrap();
    let publisher = publisher_node
        .create_dyn_pub("/runtime_only", schema.clone())
        .build()
        .unwrap();

    let connection = Connection::open(address);
    let deadline = Instant::now() + Duration::from_secs(10);
    let topic = loop {
        if let Some(topic) = connection.snapshot.lock().unwrap().topics.first().cloned() {
            break topic;
        }
        assert!(Instant::now() < deadline, "topic discovery timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(topic.domain, 88);
    assert_eq!(topic.name, "/runtime_only");
    assert!(
        embedded_registry()
            .get(&topic.type_name, &topic.hash)
            .is_err()
    );

    let advertised_hash = topic.hash.clone();
    connection.select_with_reflection(Some(topic), true);
    let reflected = loop {
        let (schema, error) = {
            let snapshot = connection.snapshot.lock().unwrap();
            (
                snapshot.reflected_schema.clone(),
                snapshot.reflection_error.clone(),
            )
        };
        if let Some(schema) = schema {
            break schema;
        }
        assert!(error.is_none(), "reflection failed: {:?}", error);
        assert!(Instant::now() < deadline, "schema reflection timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(reflected.type_name, "inspector_fixture/msg/Telemetry");
    assert_eq!(
        reflected.type_hash.as_deref(),
        Some(advertised_hash.as_str())
    );

    let mut message = DynamicMessage::new(&schema);
    message.set("label", "drive rail".to_owned()).unwrap();
    message.set("voltage", 47.5f64).unwrap();
    publisher.async_publish(&message).await.unwrap();
    let sample = loop {
        if let Some(sample) = connection.snapshot.lock().unwrap().latest.clone() {
            break sample;
        }
        assert!(Instant::now() < deadline, "sample delivery timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let decoded =
        inspector_zenoh::schema::Registry::decode_with_schema(&reflected, &sample.bytes).unwrap();
    assert_eq!(decoded.get::<String>("label").unwrap(), "drive rail");
    assert_eq!(decoded.get::<f64>("voltage").unwrap(), 47.5);

    let state = connection.snapshot.clone();
    drop(connection);
    while state.lock().unwrap().connected {
        assert!(Instant::now() < deadline, "disconnect timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(publisher);
    drop(publisher_node);
    publisher_context.shutdown().unwrap();
    router.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn retrieves_retained_sample_without_waiting_for_a_new_publication() {
    check_retained_sample(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn retrieves_retained_sample_when_publisher_is_discovered_after_selection() {
    check_retained_sample(true).await;
}

async fn check_retained_sample(late_announcement: bool) {
    use hiroz_protocol_native::qos::QosDurability;
    use zenoh_ext::{AdvancedPublisherBuilderExt, CacheConfig};
    for listener in ["tcp/127.0.0.1:0", "ws/127.0.0.1:0"] {
        let listener = local_listener(listener);
        let mut config = zenoh::Config::default();
        config.insert_json5("mode", "\"router\"").unwrap();
        config
            .insert_json5(
                "listen/endpoints",
                &serde_json::to_string(&[&listener]).unwrap(),
            )
            .unwrap();
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        config
            .insert_json5("scouting/gossip/enabled", "false")
            .unwrap();
        let router = zenoh::open(config).await.unwrap();
        let address = listener;
        let endpoint = EndpointEntity {
            id: 1,
            node: Some(NodeEntity::new(
                123,
                router.zid(),
                0,
                "latched".into(),
                "/".into(),
                String::new(),
            )),
            kind: EndpointKind::Publisher,
            topic: "/retained".into(),
            type_info: Some(TypeInfo::new(Twist::type_name(), Twist::type_hash())),
            qos: hiroz_protocol_native::qos::QosProfile {
                durability: QosDurability::TransientLocal,
                ..Default::default()
            },
        };
        let format = KeyExprFormat::RmwZenoh;
        let mut advertised = endpoint.clone();
        if late_announcement {
            advertised.kind = EndpointKind::Subscription;
            advertised.qos.durability = QosDurability::Volatile;
        }
        let _token = router
            .liveliness()
            .declare_token(
                format
                    .liveliness_key_expr(&advertised, &router.zid())
                    .unwrap()
                    .to_string(),
            )
            .await
            .unwrap();
        let publisher = router
            .declare_publisher(format.topic_key_expr(&endpoint).unwrap().to_string())
            .advanced()
            .cache(CacheConfig::default().max_samples(1))
            .publisher_detection()
            .await
            .unwrap();
        // Publish exactly once, before the inspector even connects.
        publisher.put(vec![10u8, 20, 30]).await.unwrap();
        let connection = Connection::open(address);
        let deadline = Instant::now() + Duration::from_secs(5);
        let topic = loop {
            if let Some(topic) = connection.snapshot.lock().unwrap().topics.first().cloned() {
                break topic;
            }
            assert!(Instant::now() < deadline, "Discovery timeout");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(topic.transient_local, !late_announcement);
        let mut late_token = None;
        for attempt in 0..2 {
            // Deliberately reuse the original topic snapshot on reselect, too.
            connection.select(Some(topic.clone()));
            if late_announcement && attempt == 0 {
                loop {
                    let ready = connection.snapshot.lock().unwrap().subscription_ready;
                    if ready {
                        break;
                    }
                    assert!(Instant::now() < deadline, "Subscription setup timeout");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert!(connection.snapshot.lock().unwrap().latest.is_none());
                // The publisher already cached its only sample, but its ROS
                // announcement arrives after we selected a subscriber-only topic.
                late_token = Some(
                    router
                        .liveliness()
                        .declare_token(
                            format
                                .liveliness_key_expr(&endpoint, &router.zid())
                                .unwrap()
                                .to_string(),
                        )
                        .await
                        .unwrap(),
                );
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(sample) = connection.snapshot.lock().unwrap().latest.clone() {
                    assert_eq!(&*sample.bytes, &[10, 20, 30]);
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Retained sample was never fetched"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            connection.select(None);
            while connection.snapshot.lock().unwrap().selected.is_some() {
                assert!(Instant::now() < deadline, "Unsubscribe timeout");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let state = connection.snapshot.clone();
        drop(connection);
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.lock().unwrap().connected {
            assert!(Instant::now() < deadline, "Disconnect timeout");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(late_token);
        router.close().await.unwrap();
    }
}

// SessionInfo::locators excludes loopback WebSocket addresses in Zenoh 1.10.1.
// Choose the local endpoint before opening the router instead of discovering it.
fn local_listener(endpoint: &str) -> String {
    let (transport, address) = endpoint.split_once('/').unwrap();
    let socket = std::net::TcpListener::bind(address).unwrap();
    format!("{transport}/{}", socket.local_addr().unwrap())
}

//! Router connection and bounded latest-sample delivery, independent of the UI.
use crate::{
    discovery::{Graph, Topic},
    protocol::{EndpointKind, KeyExprFormat, NodeEntity},
};
use futures::{
    FutureExt,
    future::{BoxFuture, Fuse, pending},
    select_biased,
};
use hiroz::{Builder, dynamic::MessageSchema};
use std::time::Duration;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use web_time::Instant;
use zenoh_ext::{AdvancedSubscriberBuilderExt, HistoryConfig};

pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct Sample {
    pub sequence: u64,
    pub received: Instant,
    pub bytes: Arc<[u8]>,
    pub size: usize,
}

#[derive(Default)]
pub struct Snapshot {
    pub status: String,
    pub connected: bool,
    pub topics: Vec<Topic>,
    pub domains: Vec<usize>,
    pub selected: Option<String>,
    pub latest: Option<Sample>,
    pub samples: u64,
    pub bytes: u64,
    pub subscription_error: Option<String>,
    pub subscription_ready: bool,
    pub reflected_schema: Option<Arc<MessageSchema>>,
    pub reflection_pending: bool,
    pub reflection_error: Option<String>,
    generation: usize,
}

enum Command {
    Select {
        topic: Option<Box<Topic>>,
        reflect: bool,
    },
    Disconnect,
}

struct ReflectionResult {
    selected_key: String,
    domain: usize,
    cache_key: (String, String),
    node: Option<Arc<hiroz::node::ZNode>>,
    schema: Result<Arc<MessageSchema>, String>,
}

pub struct Connection {
    pub snapshot: Arc<Mutex<Snapshot>>,
    commands: flume::Sender<Command>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Disconnect);
    }
}

impl Connection {
    pub fn open(endpoint: String) -> Self {
        let snapshot = Arc::new(Mutex::new(Snapshot {
            status: format!("Connecting to {endpoint}…"),
            ..Default::default()
        }));
        let (commands, rx) = flume::unbounded();
        let state = snapshot.clone();
        let task = async move {
            let result = worker(endpoint, state.clone(), rx).await;
            let mut s = state.lock().unwrap();
            s.connected = false;
            s.status = match result {
                Ok(()) => "Disconnected".into(),
                Err(e) => e,
            };
        };
        #[cfg(target_arch = "wasm32")]
        zenoh_runtime::ZRuntime::Application.spawn(async move {
            zenoh_runtime::spawn_on_current(task);
        });
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap()
                .block_on(task);
        });
        Self { snapshot, commands }
    }
    pub fn select(&self, topic: Option<Topic>) {
        self.select_with_reflection(topic, false);
    }

    pub fn select_with_reflection(&self, topic: Option<Topic>, reflect: bool) {
        let _ = self.commands.send(Command::Select {
            topic: topic.map(Box::new),
            reflect,
        });
    }
}

async fn worker(
    endpoint: String,
    state: Arc<Mutex<Snapshot>>,
    commands: flume::Receiver<Command>,
) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    if !endpoint.starts_with("ws/") && !endpoint.starts_with("wss/") {
        return Err("Browser connections need ws/host:port or wss/host:port".into());
    }
    let mut config = zenoh::Config::default();
    for (key, value) in [
        ("mode", "\"client\"".into()),
        (
            "connect/endpoints",
            serde_json::to_string(&[&endpoint]).unwrap(),
        ),
        ("listen/endpoints", "[]".into()),
        ("scouting/multicast/enabled", "false".into()),
        ("connect/exit_on_failure", "true".into()),
        ("connect/timeout_ms", "5000".into()),
    ] {
        config
            .insert_json5(key, &value)
            .map_err(|e| e.to_string())?;
    }
    let session = zenoh::open(config)
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;
    let format = KeyExprFormat::RmwZenoh;
    // History plus live events avoids a get/subscribe discovery gap.
    let discovery = session
        .liveliness()
        .declare_subscriber("@ros2_lv/**")
        .history(true)
        .await
        .map_err(|e| e.to_string())?;
    {
        let mut s = state.lock().unwrap();
        s.status = format!("Session open · {endpoint} · discovering all domains");
        s.connected = true;
    }
    let mut graph = Graph::default();
    let mut subscription = None;
    let mut token = None;
    let mut node_token = None;
    let mut generation = 0usize;
    let mut history_enabled = false;
    let mut reflection_nodes = BTreeMap::new();
    let mut schema_cache = BTreeMap::new();
    let mut reflection: Fuse<BoxFuture<'static, ReflectionResult>> = pending().boxed().fuse();
    loop {
        select_biased! {
            command = commands.recv_async().fuse() => {
                let Ok(Command::Select { topic: mut selected, mut reflect }) = command else { break; };
                // A rapid series of clicks only needs the most recent subscription.
                let mut disconnect = false;
                while let Ok(command) = commands.try_recv() {
                    match command {
                        Command::Select { topic, reflect: next_reflect } => {
                            selected = topic;
                            reflect = next_reflect;
                        }
                        Command::Disconnect => { disconnect = true; break; }
                    }
                }
                if disconnect { break; }
                // Clear selection before dropping old handles; late callbacks are ignored.
                {
                    let mut s = state.lock().unwrap();
                    s.selected = None; s.latest = None; s.samples = 0; s.bytes = 0; s.subscription_error = None; s.subscription_ready = false;
                    s.reflected_schema = None; s.reflection_pending = false; s.reflection_error = None;
                    s.generation = generation + 1;
                }
                drop(subscription.take()); drop(token.take()); drop(node_token.take());
                reflection = pending().boxed().fuse();
                generation += 1;
                if let Some(topic) = selected {
                    let node = NodeEntity::new(topic.domain, session.zid(), 0,
                        "inspector_zenoh".into(), "/".into(), String::new());
                    let node_key = format.node_liveliness_key_expr(&node).map_err(|e| e.to_string())?;
                    node_token = Some(session.liveliness().declare_token(node_key.to_string())
                        .await.map_err(|e| e.to_string())?);
                    let key = topic.key.clone();
                    state.lock().unwrap().selected = Some(key.clone());
                    // Use the current graph even if the UI selected an older snapshot.
                    history_enabled = graph.topics().iter().find(|t| t.key == key)
                        .map_or(topic.transient_local, |t| t.transient_local);
                    match subscribe(&session, key.clone(), history_enabled, state.clone(), generation).await {
                        Ok(sub) => {
                            subscription = Some(sub);
                            // Announce a ROS subscriber too: some publishers wait for one.
                            let mut subscriber = topic.endpoint;
                            subscriber.node = Some(node.clone()); subscriber.id = generation;
                            subscriber.kind = EndpointKind::Subscription;
                            subscriber.qos.reliability = crate::protocol::qos::QosReliability::BestEffort;
                            // Keep the announcement compatible with volatile publishers
                            // even when another publisher on this topic offers history.
                            subscriber.qos.durability = crate::protocol::qos::QosDurability::Volatile;
                            let key = format.liveliness_key_expr(&subscriber, &session.zid()).map_err(|e| e.to_string())?;
                            match session.liveliness().declare_token(key.to_string()).await {
                                Ok(t) => { token = Some(t); state.lock().unwrap().subscription_ready = true; },
                                Err(e) => state.lock().unwrap().subscription_error = Some(format!("ROS subscription announcement failed: {e}")),
                            }
                        }
                        Err(e) => state.lock().unwrap().subscription_error = Some(format!("Subscription failed: {e}")),
                    }
                    if reflect {
                        let cache_key = (topic.type_name.clone(), topic.hash.clone());
                        if let Some(schema) = schema_cache.get(&cache_key) {
                            state.lock().unwrap().reflected_schema = Some(Arc::clone(schema));
                        } else {
                            state.lock().unwrap().reflection_pending = true;
                            let existing_node = reflection_nodes.get(&topic.domain).cloned();
                            reflection = reflect_schema(
                                endpoint.clone(),
                                key,
                                topic.domain,
                                topic.name,
                                cache_key,
                                existing_node,
                            ).boxed().fuse();
                        }
                    }
                }
            },
            result = reflection => {
                reflection = pending().boxed().fuse();
                if let Some(node) = result.node {
                    reflection_nodes.insert(result.domain, node);
                }
                let mut s = state.lock().unwrap();
                if s.selected.as_deref() == Some(&result.selected_key) {
                    s.reflection_pending = false;
                    match result.schema {
                        Ok(schema) => {
                            schema_cache.insert(result.cache_key, Arc::clone(&schema));
                            s.reflected_schema = Some(schema);
                        }
                        Err(e) => s.reflection_error = Some(format!("Runtime schema unavailable: {e}")),
                    }
                }
            },
            sample = discovery.recv_async().fuse() => {
                let sample = sample.map_err(|e| format!("Discovery stopped: {e}"))?;
                // Our selected-topic announcement must not create phantom domains.
                let is_own = sample.key_expr().as_str().split('/').nth(2)
                    .is_some_and(|zid| zid == session.zid().to_string());
                if !is_own && graph.update(sample.key_expr().as_str(), sample.kind() == zenoh::sample::SampleKind::Put) {
                    let refresh = {
                        let mut s = state.lock().unwrap();
                        s.topics = graph.topics();
                        s.domains = graph.domains();
                        s.selected.as_ref().filter(|key| !history_enabled && subscription.is_some()
                            && s.topics.iter().any(|t| &t.key == *key && t.transient_local)).cloned()
                    };
                    if let Some(key) = refresh {
                        // Upgrade once per selection. Keep history detection enabled even
                        // if this publisher disappears, so later publishers are handled too.
                        generation += 1;
                        {
                            let mut s = state.lock().unwrap();
                            s.generation = generation;
                            s.subscription_ready = false;
                        }
                        drop(subscription.take());
                        match subscribe(&session, key, true, state.clone(), generation).await {
                            Ok(sub) => {
                                subscription = Some(sub);
                                history_enabled = true;
                                state.lock().unwrap().subscription_ready = token.is_some();
                            }
                            Err(e) => state.lock().unwrap().subscription_error = Some(format!("Subscription failed: {e}")),
                        }
                    }
                }
            },
        }
    }
    drop(subscription);
    drop(token);
    drop(discovery);
    drop(node_token);
    session.close().await.map_err(|e| e.to_string())
}

async fn reflect_schema(
    endpoint: String,
    selected_key: String,
    domain: usize,
    topic_name: String,
    cache_key: (String, String),
    existing_node: Option<Arc<hiroz::node::ZNode>>,
) -> ReflectionResult {
    let node = match existing_node {
        Some(node) => Ok(node),
        None => reflection_node(&endpoint, domain).await.map(Arc::new),
    };
    let (node, schema) = match node {
        Ok(node) => {
            let schema = match node
                .discover_topic_schema(&topic_name, Duration::from_secs(3))
                .await
            {
                Ok(discovered) if discovered.type_hash == cache_key.1 => {
                    let mut schema = (*discovered.schema).clone();
                    schema.type_hash = Some(discovered.type_hash);
                    Ok(Arc::new(schema))
                }
                Ok(discovered) => Err(format!(
                    "Type description hash mismatch: publisher advertised {}, service returned {}",
                    cache_key.1, discovered.type_hash
                )),
                Err(e) => Err(e.to_string()),
            };
            (Some(node), schema)
        }
        Err(e) => (None, Err(e)),
    };
    ReflectionResult {
        selected_key,
        domain,
        cache_key,
        node,
        schema,
    }
}

async fn reflection_node(endpoint: &str, domain: usize) -> Result<hiroz::node::ZNode, String> {
    let builder = hiroz::context::ZContextBuilder::default()
        .with_domain_id(domain)
        .with_mode("client")
        .with_connect_endpoints([endpoint])
        .with_listen_endpoints(std::iter::empty::<&str>())
        .disable_multicast_scouting()
        .with_json("connect/exit_on_failure", true)
        .with_json("connect/timeout_ms", 5000);
    #[cfg(not(target_arch = "wasm32"))]
    let context = builder.build();
    #[cfg(target_arch = "wasm32")]
    let context = builder.build_async().await;
    let context = context.map_err(|e| format!("Reflection connection failed: {e}"))?;
    context
        .create_node("inspector_zenoh_reflection")
        .without_parameters()
        .build()
        .map_err(|e| format!("Reflection node failed: {e}"))
}

/// Replacing the transport subscription preserves the selected topic's counters
/// and latest sample; generation checks exclude callbacks from the old handle.
async fn subscribe(
    session: &zenoh::Session,
    key: String,
    history: bool,
    state: Arc<Mutex<Snapshot>>,
    generation: usize,
) -> Result<zenoh_ext::AdvancedSubscriber<()>, String> {
    let mut builder = session.declare_subscriber(key.clone()).advanced();
    if history {
        builder = builder
            .history(
                HistoryConfig::default()
                    .max_samples(1)
                    .detect_late_publishers(),
            )
            .query_timeout(Duration::from_secs(1));
    }
    builder
        .callback(move |sample: zenoh::sample::Sample| {
            let size = sample.payload().len();
            let bytes: Arc<[u8]> = if size <= MAX_PAYLOAD {
                sample.payload().to_bytes().as_ref().into()
            } else {
                Arc::from([])
            };
            let mut s = state.lock().unwrap();
            if s.selected.as_deref() != Some(&key) || s.generation != generation {
                return;
            }
            s.samples += 1;
            s.bytes += size as u64;
            s.latest = Some(Sample {
                sequence: s.samples,
                received: Instant::now(),
                bytes,
                size,
            });
        })
        .await
        .map_err(|e| e.to_string())
}

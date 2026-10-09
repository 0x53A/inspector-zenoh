//! One graph entry per ROS endpoint; aggregate by full data key, including type hash.
use crate::protocol::{
    EndpointEntity, EndpointKind, Entity, KeyExprFormat, NodeEntity, qos::QosProfile,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Topic {
    pub key: String,
    pub domain: usize,
    pub name: String,
    pub type_name: String,
    pub hash: String,
    pub publishers: usize,
    /// Distinct advertised publisher profiles and their endpoint counts.
    pub publisher_qos: Vec<(QosProfile, usize)>,
    pub transient_local: bool,
    pub subscribers: usize,
    pub nodes: Vec<String>,
    pub endpoint: EndpointEntity,
}

#[derive(Default)]
pub struct Graph {
    endpoints: BTreeMap<String, EndpointEntity>,
    nodes: BTreeMap<String, NodeEntity>,
}

impl Graph {
    pub fn update(&mut self, token: &str, alive: bool) -> bool {
        if !alive {
            let endpoint = self.endpoints.remove(token).is_some();
            return self.nodes.remove(token).is_some() || endpoint;
        }
        let Ok(key) = zenoh::key_expr::KeyExpr::try_from(token) else {
            return false;
        };
        match KeyExprFormat::RmwZenoh.parse_liveliness(&key) {
            Ok(Entity::Node(node)) => {
                self.nodes.insert(token.into(), node);
            }
            Ok(Entity::Endpoint(endpoint)) if endpoint.node.is_some() => {
                self.endpoints.insert(token.into(), endpoint);
            }
            _ => return false,
        }
        true
    }

    /// Includes nodes without topics and service-only domains.
    pub fn domains(&self) -> Vec<usize> {
        self.nodes
            .values()
            .map(|n| n.domain_id)
            .chain(
                self.endpoints
                    .values()
                    .filter_map(|e| e.node.as_ref().map(|n| n.domain_id)),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn topics(&self) -> Vec<Topic> {
        let mut topics: BTreeMap<String, Topic> = BTreeMap::new();
        for endpoint in self.endpoints.values() {
            if !matches!(
                endpoint.kind,
                EndpointKind::Publisher | EndpointKind::Subscription
            ) {
                continue;
            }
            let Some(info) = &endpoint.type_info else {
                continue;
            };
            let Ok(key) = KeyExprFormat::RmwZenoh.topic_key_expr(endpoint) else {
                continue;
            };
            let topic = topics.entry(key.to_string()).or_insert_with(|| Topic {
                key: key.to_string(),
                domain: endpoint.node.as_ref().unwrap().domain_id,
                name: endpoint.topic.clone(),
                type_name: info.name.clone(),
                hash: info.hash.to_string(),
                publishers: 0,
                publisher_qos: vec![],
                transient_local: false,
                subscribers: 0,
                nodes: vec![],
                endpoint: endpoint.clone(),
            });
            if endpoint.kind == EndpointKind::Publisher {
                topic.publishers += 1;
                if let Some((_, count)) = topic
                    .publisher_qos
                    .iter_mut()
                    .find(|(qos, _)| *qos == endpoint.qos)
                {
                    *count += 1;
                } else {
                    topic.publisher_qos.push((endpoint.qos, 1));
                }
                topic.transient_local |=
                    endpoint.qos.durability == crate::protocol::qos::QosDurability::TransientLocal;
                topic.endpoint = endpoint.clone();
            } else {
                topic.subscribers += 1;
            }
            if let Some(node) = &endpoint.node {
                let name = format!("{}/{}", node.namespace.trim_end_matches('/'), node.name);
                if !topic.nodes.contains(&name) {
                    topic.nodes.push(name);
                }
            }
        }
        let mut topics: Vec<_> = topics.into_values().collect();
        topics.sort_by(|a, b| (&a.name, &a.key).cmp(&(&b.name, &b.key)));
        topics
    }
}

pub fn display_type(name: &str) -> String {
    name.replace("::msg::dds_::", "/msg/")
        .replace("::action::dds_::", "/action/")
        .trim_end_matches('_')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{NodeEntity, TypeHash, TypeInfo};
    #[test]
    fn graph_handles_multiple_publishers_removals_and_domains() {
        let zid = "1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(7, zid, 1, "talker".into(), "/rover".into(), String::new());
        let mut e = EndpointEntity {
            id: 2,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "/rover/state".into(),
            type_info: Some(TypeInfo::new(
                "std_msgs::msg::dds_::String_",
                TypeHash::from_rihs_string(&format!("RIHS01_{}", "00".repeat(32))).unwrap(),
            )),
            qos: Default::default(),
        };
        let fmt = KeyExprFormat::RmwZenoh;
        let a = fmt.liveliness_key_expr(&e, &zid).unwrap().to_string();
        e.id = 3;
        e.qos.durability = crate::protocol::qos::QosDurability::TransientLocal;
        let b = fmt.liveliness_key_expr(&e, &zid).unwrap().to_string();
        let mut g = Graph::default();
        assert!(!g.update("not/a/ros/token", true));
        g.update(&a, true);
        g.update(&a, true);
        g.update(&b, true);
        assert_eq!(g.topics()[0].publishers, 2);
        assert_eq!(g.topics()[0].publisher_qos.len(), 2);
        assert!(
            g.topics()[0]
                .publisher_qos
                .iter()
                .all(|(_, count)| *count == 1)
        );
        assert!(g.topics()[0].transient_local);
        g.update(&b, false);
        assert!(!g.topics()[0].transient_local);
        assert_eq!(g.topics()[0].publisher_qos, vec![(Default::default(), 1)]);
        // Subscriber QoS must not appear among publisher offerings.
        let mut subscriber = e.clone();
        subscriber.kind = EndpointKind::Subscription;
        subscriber.id = 4;
        let subscriber_key = fmt
            .liveliness_key_expr(&subscriber, &zid)
            .unwrap()
            .to_string();
        g.update(&subscriber_key, true);
        assert_eq!(g.topics()[0].publisher_qos, vec![(Default::default(), 1)]);
        g.update(&subscriber_key, false);
        g.update(&b, true);
        e.node.as_mut().unwrap().domain_id = 19;
        let other = fmt.liveliness_key_expr(&e, &zid).unwrap().to_string();
        g.update(&other, true);
        assert_eq!(g.domains(), vec![7, 19]);
        assert_eq!(g.topics().len(), 2);
        g.update(&other, false);
        let node_key = fmt
            .node_liveliness_key_expr(e.node.as_ref().unwrap())
            .unwrap()
            .to_string();
        g.update(&node_key, true);
        assert_eq!(g.domains(), vec![7, 19]);
        g.update(&node_key, false);
        assert_eq!(g.domains(), vec![7]);
        assert!(g.topics()[0].key.starts_with("7/rover/state/"));
        g.update(&a, false);
        assert_eq!(g.topics()[0].publishers, 1);
        g.update(&b, false);
        assert!(g.topics().is_empty());
        assert!(g.domains().is_empty());
        e.kind = EndpointKind::Service;
        let service_key = fmt.liveliness_key_expr(&e, &zid).unwrap().to_string();
        g.update(&service_key, true);
        assert_eq!(g.domains(), vec![19]);
        assert!(g.topics().is_empty());
        g.update(&service_key, false);
        assert!(g.domains().is_empty());
    }
}

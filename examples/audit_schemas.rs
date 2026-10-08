//! Read-only live graph audit: checks embedded type hashes without subscribing to data.
use clap::Parser;
use inspector_zenoh::{schema::embedded_registry, transport::Connection};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    endpoint: String,
    /// Time to collect existing and live graph announcements.
    #[arg(long, default_value_t = 5)]
    seconds: u64,
    /// Restrict the audit to a ROS domain.
    #[arg(long)]
    domain_id: Option<usize>,
}

fn main() {
    let args = Args::parse();
    let connection = Connection::open(args.endpoint);
    std::thread::sleep(std::time::Duration::from_secs(args.seconds));
    let state = connection.snapshot.lock().unwrap();
    let topics: Vec<_> = state
        .topics
        .iter()
        .filter(|t| args.domain_id.is_none_or(|d| t.domain == d))
        .collect();
    if !state.connected || topics.is_empty() {
        eprintln!("No live topics: {}", state.status);
        std::process::exit(1);
    }
    let registry = embedded_registry();
    let mut missing = 0;
    for topic in &topics {
        match registry.get(&topic.type_name, &topic.hash) {
            Ok(_) => println!("OK\t{}\t{}\t{}", topic.domain, topic.name, topic.type_name),
            Err(error) => {
                missing += 1;
                println!(
                    "MISSING\t{}\t{}\t{}\t{}",
                    topic.domain, topic.name, topic.type_name, error
                );
            }
        }
    }
    eprintln!(
        "{} topics, {} domains, {} embedded schemas, {missing} missing/mismatched",
        topics.len(),
        topics
            .iter()
            .map(|t| t.domain)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        registry.len()
    );
    drop(state);
    let snapshot = connection.snapshot.clone();
    drop(connection);
    for _ in 0..100 {
        if !snapshot.lock().unwrap().connected {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if missing != 0 {
        std::process::exit(1);
    }
}

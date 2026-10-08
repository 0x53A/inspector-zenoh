//! Measure discovery and selection-to-first-sample latency without publishing data.
use clap::Parser;
use inspector_zenoh::transport::Connection;
use std::time::{Duration, Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    endpoint: String,
    #[arg(long, default_value_t = 123)]
    domain_id: usize,
    #[arg(long, required = true)]
    topic: Vec<String>,
    #[arg(long, default_value_t = 5)]
    timeout: u64,
}
fn main() {
    let args = Args::parse();
    let connection = Connection::open(args.endpoint);
    let discovery_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = connection.snapshot.lock().unwrap();
        if args.topic.iter().all(|name| {
            state
                .topics
                .iter()
                .any(|t| t.domain == args.domain_id && &t.name == name)
        }) {
            break;
        }
        if Instant::now() >= discovery_deadline {
            eprintln!("Discovery deadline: {}", state.status);
            break;
        }
        drop(state);
        std::thread::sleep(Duration::from_millis(20));
    }
    for name in args.topic {
        let topic = connection
            .snapshot
            .lock()
            .unwrap()
            .topics
            .iter()
            .find(|t| t.domain == args.domain_id && t.name == name)
            .cloned();
        let Some(topic) = topic else {
            println!("{name}: not advertised");
            continue;
        };
        println!(
            "{name}: {} publishers, {:?}",
            topic.publishers, topic.endpoint.qos
        );
        let start = Instant::now();
        connection.select(Some(topic.clone()));
        loop {
            let state = connection.snapshot.lock().unwrap();
            if state.selected.as_deref() == Some(&topic.key)
                && let Some(sample) = &state.latest
            {
                println!(
                    "  first sample: {} ms, {} bytes",
                    start.elapsed().as_millis(),
                    sample.size
                );
                break;
            }
            if start.elapsed() >= Duration::from_secs(args.timeout) {
                println!(
                    "  no sample after {} s; {:?}",
                    args.timeout, state.subscription_error
                );
                break;
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(10));
        }
        connection.select(None);
        while connection.snapshot.lock().unwrap().selected.is_some() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

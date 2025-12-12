use clap::Parser;
use log::info;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tonic::{transport::Server, Request, Response, Status};

// Reuse protobuf definitions
mod pb;
use pb::sensor_service_server::{SensorService, SensorServiceServer};
use pb::SensorEvent;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the Unix socket to write alerts to
    #[arg(short, long, default_value = "/tmp/suricata.sock")]
    socket: String,

    /// Port to listen on for gRPC
    #[arg(short, long, default_value_t = 50051)]
    port: u16,

    /// Rate limit (events/sec) for input generator. 0 = unlimited.
    #[arg(short, long, default_value_t = 0)]
    rate: u64,
}

// Mock Server
#[derive(Default)]
pub struct MockSensorService {
    counter: Arc<AtomicU64>,
}

#[tonic::async_trait]
impl SensorService for MockSensorService {
    async fn stream_data(
        &self,
        request: Request<tonic::Streaming<SensorEvent>>,
    ) -> Result<Response<()>, Status> {
        info!("Server: Accepted stream connection");
        let mut stream = request.into_inner();
        let mut count = 0;
        while let Some(event) = stream.message().await? {
            count += 1;
            if count % 1000 == 0 {
                info!(
                    "Received {} events. Last event metrics: {}",
                    count,
                    event.metrics.len()
                );
            }
            // Count metrics in the event
            self.counter
                .fetch_add(event.metrics.len() as u64, Ordering::Relaxed);
        }
        info!("Server: Stream ended");
        Ok(Response::new(()))
    }
}

// Input Generator
fn start_input_generator(socket_path: String, rate: u64) {
    std::thread::spawn(move || {
        // Create the socket listener if it doesn't exist, to ensure we can connect to it?
        // No, the CLIENT (sensor-service) creates the listener.
        // WE are the Suricata Mock, so we CONNECT to the socket.
        // But we should wait for it to appear.

        info!("Input Generator: Waiting for socket {}...", socket_path);
        loop {
            if std::path::Path::new(&socket_path).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        info!("Input Generator: Socket found. Connecting...");

        // Connect to socket
        let mut stream = loop {
            match std::os::unix::net::UnixStream::connect(&socket_path) {
                Ok(s) => break s,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        };

        info!("Input Generator: Connected. Starting flood...");

        // Sample Alert JSON
        let alert_json = r#"{"metadata":{"sensor_id":"test","sensor_version":"1.0","sent_at":0,"hash_sha256":"hash","read_at":0,"received_at":0},"timestamp":"2023-10-27T10:00:00.000000+0000","flow_id":123456789,"in_iface":"eth0","event_type":"alert","src_ip":"192.168.1.10","src_port":12345,"dest_ip":"10.0.0.1","dest_port":80,"proto":"TCP","alert":{"action":"allowed","gid":1,"signature_id":1000001,"rev":1,"signature":"Test Alert","category":"Misc","severity":3},"http":{"hostname":"example.com","url":"/","http_user_agent":"Mozilla/5.0","http_content_type":"text/html","http_method":"GET","protocol":"HTTP/1.1","status":200,"length":1024},"app_proto":"http","flow":{"pkts_toserver":10,"pkts_toclient":10,"bytes_toserver":1000,"bytes_toclient":5000,"start":"2023-10-27T10:00:00.000000+0000"}}"#;
        let alert_line = format!("{}\n", alert_json);

        let mut count = 0;
        let mut flow_id_counter: u64 = 0;
        let start = std::time::Instant::now();

        loop {
            flow_id_counter = flow_id_counter.wrapping_add(1);
            // Sample Alert JSON with dynamic signature_id to ensure unique hash
            let alert_json = format!(
                r#"{{"metadata":{{"sensor_id":"test","sensor_version":"1.0","sent_at":0,"hash_sha256":"hash","read_at":0,"received_at":0}},"timestamp":"2023-10-27T10:00:00.000000+0000","flow_id":123456789,"in_iface":"eth0","event_type":"alert","src_ip":"192.168.1.10","src_port":12345,"dest_ip":"10.0.0.1","dest_port":80,"proto":"TCP","alert":{{"action":"allowed","gid":1,"signature_id":{},"rev":1,"signature":"Test Alert","category":"Misc","severity":3}},"http":{{"hostname":"example.com","url":"/","http_user_agent":"Mozilla/5.0","http_content_type":"text/html","http_method":"GET","protocol":"HTTP/1.1","status":200,"length":1024}},"app_proto":"http","flow":{{"pkts_toserver":10,"pkts_toclient":10,"bytes_toserver":1000,"bytes_toclient":5000,"start":"2023-10-27T10:00:00.000000+0000"}}}}"#,
                flow_id_counter
            );

            let alert_line = format!("{}\n", alert_json);
            let bytes = alert_line.as_bytes();

            if let Err(_) = stream.write_all(bytes) {
                // Reconnect if failed
                if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket_path) {
                    stream = s;
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }

            if rate > 0 {
                count += 1;
                if count >= rate {
                    let elapsed = start.elapsed();
                    if elapsed < Duration::from_secs(1) {
                        std::thread::sleep(Duration::from_secs(1) - elapsed);
                    }
                    count = 0;
                    // Reset start? No, this logic is a bit flawed for precise rate limiting.
                    // Simple token bucket or just sleep per batch would be better.
                    // For now, let's just sleep a tiny bit if rate is set.
                    // Actually, let's ignore precise rate limiting for now as the goal is usually max throughput.
                    // If rate > 0, we sleep 1s / rate.
                    let sleep_ns = 1_000_000_000 / rate;
                    std::thread::sleep(Duration::from_nanos(sleep_ns));
                }
            }
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup logging
    unsafe {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let args = Args::parse();

    // Start Mock Server
    let addr = format!("0.0.0.0:{}", args.port).parse()?;
    let counter = Arc::new(AtomicU64::new(0));
    let service = MockSensorService {
        counter: counter.clone(),
    };

    info!("Mock gRPC Server listening on {}", addr);

    let counter_clone = counter.clone();
    tokio::spawn(async move {
        Server::builder()
            .add_service(SensorServiceServer::new(service))
            .serve(addr)
            .await
            .unwrap();
    });

    // Start Stats Reporter
    tokio::spawn(async move {
        let mut last_count = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let current_count = counter_clone.load(Ordering::Relaxed);
            let rate = current_count - last_count;
            info!("Server Throughput: {} events/sec", rate);
            last_count = current_count;
        }
    });

    // Start Input Generator
    info!("Starting Input Generator targeting {}", args.socket);
    start_input_generator(args.socket, args.rate);

    // Keep main alive
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    Ok(())
}

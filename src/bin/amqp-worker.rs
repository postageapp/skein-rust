use std::env;
use std::error::Error;
use std::num::ParseIntError;

use async_trait::async_trait;
use clap::Clap;
use dotenv::dotenv;
use serde_json::json;
use serde_json::Value;
use tokio::time::sleep;
use tokio::time::Duration;

use skein_rpc::amqp::Worker;
use skein_rpc::logging;
use skein_rpc::Responder;
use skein_rpc::rpc;

#[derive(Clap)]
#[clap(version = env!("CARGO_PKG_VERSION"))]
struct Program {
    #[clap(short, long, parse(from_occurrences))]
    verbose: usize,
    #[clap(short,long)]
    env_file : Option<String>,
    #[clap(short,long)]
    amqp_url : Option<String>,
    #[clap(long,parse(try_from_str=Self::try_into_duration))]
    timeout_warning : Option<Duration>,
    #[clap(long,parse(try_from_str=Self::try_into_duration))]
    timeout_terminate : Option<Duration>,
    #[clap(short,long)]
    queue : Option<String>
}

impl Program {
    fn try_into_duration(s: &str) -> Result<Duration, ParseIntError> {
        s.parse().map(|seconds| Duration::from_secs(seconds))
    }
}

struct WorkerContext {
    handler_count: usize
}

impl WorkerContext {
    fn new() -> Self {
        Self {
            handler_count: 0
        }
    }
}

#[async_trait]
impl Responder for WorkerContext {
    async fn respond(&mut self, request: &rpc::Request) -> Result<Value,Box<dyn Error>> {
        match request.method().as_str() {
            "echo" => {
                self.handler_count += 1;

                Ok(request.params().cloned().unwrap_or(json!(null)))
            },
            "stall" => {
                sleep(Duration::from_secs(10)).await;

                Ok(json!(false))
            },
            _ => {
                Err(Box::new(rpc::ErrorResponse::new(-32601, "Method not found", None)))
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let program = Program::parse();

    match program.env_file {
        Some(ref path) => {
            dotenv::from_filename(path).ok();
        },
        None => {
            dotenv().ok();
        }
    }

    logging::setup(program.verbose);

    let amqp_url = program.amqp_url.unwrap_or_else(|| env::var("AMQP_URL").unwrap_or_else(|_| "amqp://localhost:5672/%2f".to_string()));
    let queue = program.queue.unwrap_or_else(|| env::var("AMQP_QUEUE").unwrap_or_else(|_| "skein_test".to_string()));

    let context = WorkerContext::new();

    let (worker, terminator) = Worker::new(
        context,
        amqp_url,
        queue,
        program.timeout_warning,
        program.timeout_terminate
    )?;

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Couldn't bind to CTRL-C handler.");

        terminator.send(()).await.expect("Couldn't send interrupt signal.");

        log::info!("Interrupted.");
    });

    let worker = worker.run().await??;

    log::info!("Handled {} message(s)", worker.context().handler_count);

    Ok(())
}

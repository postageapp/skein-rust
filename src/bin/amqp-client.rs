use std::env;
use std::fs::File;
use std::io::Write;
use std::num::ParseFloatError;
use std::time::Duration;
use std::time::Instant;

use clap::Parser;
use dotenv::dotenv;
use serde_json::json;
use tokio::time::sleep;

use skein_rpc::AsyncResult;
use skein_rpc::Client;
use skein_rpc::amqp::Client as AMQPClient;
use skein_rpc::amqp::ClientOptions as AMQPClientOptions;
use skein_rpc::logging;

#[derive(Parser)]
#[clap(version = env!("CARGO_PKG_VERSION"))]
struct Program {
    #[clap(short, long, parse(from_occurrences))]
    verbose: usize,
    #[clap(long)]
    receipt_log : Option<String>,
    #[clap(short,long)]
    env_file : Option<String>,
    #[clap(short,long)]
    amqp_url : Option<String>,
    #[clap(short,long)]
    queue : Option<String>,
    #[clap(short,long)]
    silent: bool,
    #[clap(short='t',long)]
    report: bool,
    #[clap(short,long,default_value="1")]
    repeat: usize,
    #[clap(long)]
    sequencer: bool,
    #[clap(short,long,default_value="0",parse(try_from_str=Self::try_into_duration))]
    repeat_delay: Duration,
    #[clap(long)]
    ident: Option<String>,
    #[clap(short,long,default_value="30",parse(try_from_str=Self::try_into_duration))]
    timeout : Duration,
    #[clap(long)]
    noreply : bool,
    method : String,
    #[clap(multiple=true)]
    args : Vec<String>
}

impl Program {
    fn try_into_duration(s: &str) -> Result<Duration, ParseFloatError> {
        s.parse().map(Duration::from_secs_f32)
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> AsyncResult<()> {
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

    let options = AMQPClientOptions::new(
        program.amqp_url.unwrap_or_else(|| env::var("AMQP_URL").unwrap_or_else(|_| "amqp://localhost:5672/%2f".to_string())),
        program.queue.unwrap_or_else(|| env::var("AMQP_QUEUE").unwrap_or_else(|_| "skein_test".to_string())),
        program.ident.unwrap_or_else(|| "amqp-client".to_string())
    ).with_timeout(program.timeout);

    // skein_test

    let client = AMQPClient::new(options).await?;
    let method = program.method;
    let params = Some(json!(program.args));

    let now = Instant::now();

    let repeat = program.repeat;

    let mut completed = 0;

    let mut receipt_log = program.receipt_log.map(|path| File::create(path.as_str()).unwrap());

    if program.noreply {
        for i in 0..repeat {
            let params = if program.sequencer {
                Some(json!(i + 1))
            }
            else {
                params.clone()
            };

            match client.rpc_request_inject(method.as_str(), params.clone()).await {
                Ok(id) => {
                    completed += 1;

                    if let Some(ref mut log) = &mut receipt_log {
                        log.write_all(format!("{}\n", id).as_bytes()).unwrap();
                    }

                    log::debug!("Sent {}/{}", completed, repeat);
                },
                Err(err) => {
                    log::error!("Error with request: {}", err);
                }
            }

            sleep(program.repeat_delay).await;
        }
    }
    else {
        for i in 0..repeat {
            let params = if program.sequencer {
                Some(json!(i + 1))
            }
            else {
                params.clone()
            };

            match client.rpc_request(method.as_str(), params.clone()).await {
                Ok(response) => {
                    if !program.silent {
                        println!("{}", response.to_string());
                    }

                    completed += 1;

                    log::debug!("Sent {}/{}", completed, repeat);
                },
                Err(err) => {
                    log::error!("Error sending request: {}", err);
                }
            }

            sleep(program.repeat_delay).await;
        }
    }

    log::debug!("Run complete, cleaning up client.");

    client.close();

    let report = client.into_handle().await.unwrap();

    if program.report {
        let elapsed = now.elapsed().as_secs_f64();

        log::info!("Completed {} request(s) in {:.2}s ({:.1}RPS)", completed, elapsed, completed as f64/elapsed);
    }

    log::info!(
        "Client report: connections={}, confirmations={}, retried={}, pending={}",
        report.connections,
        report.confirmations,
        report.retried,
        report.pending
    );

    Ok(())
}

extern crate async_trait;
extern crate clap;
extern crate tokio;

pub mod amqp;

mod client;
pub use client::Client;

pub mod logging;

pub mod rpc;

mod responder;
pub use responder::Responder;

pub type AsyncResult<T,E=Box<dyn std::error::Error + Sync + Send>> = std::result::Result<T, E>;

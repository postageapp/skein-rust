use futures::future::FutureExt;
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::convert::TryFrom;

use amq_protocol_types::ShortString;
use gethostname::gethostname;
use lapin::{
    BasicProperties,
    options::*,
    types::FieldTable,
    message::Delivery,
    Channel,
    Connection,
    ConnectionProperties,
    Result as LapinResult
};
use serde::Serialize;
use tokio::sync::mpsc::{channel,Sender,unbounded_channel,UnboundedSender};
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::rpc;

#[derive(Debug)]
pub struct Client {
    rpc: UnboundedSender<(rpc::Request, Sender<rpc::Response>)>,
    queue_name: String,
    ident: String,
    handle: JoinHandle<()>
}

impl Client {
    pub async fn new(amqp_addr: impl ToString, queue_name: impl ToString, client_name: impl ToString) -> LapinResult<Client> {
        let connection = Connection::connect(
            amqp_addr.to_string().as_str(),
            ConnectionProperties::default().with_default_executor(8),
        ).await?;

        let channel = connection.create_channel().await?;
        let queue_name = queue_name.to_string();

        channel.queue_declare(
            queue_name.as_str(),
            QueueDeclareOptions {
                passive: false,
                durable: true,
                exclusive: false,
                auto_delete: false,
                nowait: true
            },
            FieldTable::default()
        ).await?;

        let ident = format!(
            "{}-{}@{}",
            client_name.to_string(),
            Uuid::new_v4().to_string(),
            gethostname().into_string().unwrap()
        );

        channel.queue_declare(
            ident.as_str(),
            QueueDeclareOptions {
                passive: false,
                durable: false,
                exclusive: false,
                auto_delete: true,
                nowait: true
            },
            FieldTable::default()
        ).await?;

        let mut consumer = channel.basic_consume(
            ident.as_str(),
            "",
            BasicConsumeOptions::default(),
            FieldTable::default()
        ).await?;

        let (tx, mut rx) = unbounded_channel::<(rpc::Request, Sender<rpc::Response>)>();

        let rpc_queue_name = queue_name.clone();

        let handle = tokio::spawn(async move {
            let rpc_queue_name = rpc_queue_name.as_str();
            let properties = BasicProperties::default().with_content_type(ShortString::from("application/json"));
            let mut requests = HashMap::new();

            loop {
                tokio::select!(
                    r = rx.recv() => {
                        match r {
                            Some((request,reply)) => {
                                match serde_json::to_string(&request) {
                                    Ok(str) => {
                                        let payload = str.as_bytes().to_vec();

                                        match channel.basic_publish(
                                            "", // FUTURE: Allow specifying exchange
                                            rpc_queue_name,
                                            Default::default(),
                                            payload,
                                            properties.clone()
                                        ).await {
                                            Ok(_) => {
                                                requests.insert(request.id().clone(), (request,reply));
                                            },
                                            Err(err) => {
                                                reply.send(
                                                    rpc::Response::error_for(
                                                        &request,
                                                        -32603,
                                                        format!("Could not send request: {}", err),
                                                        None
                                                    )
                                                ).await.ok();
                                            }
                                        }
                                    },
                                    Err(err) => {
                                        log::error!("Error serializing request: {}", err);
                                    }
                                }
                            },
                            None => break
                        }
                    },
                    incoming = consumer.next() => {
                        match incoming {
                            Some(Ok((_channel, delivery))) => {
                                match rpc::Response::try_from(&delivery) {
                                    Ok(response) => {
                                        match response.id() {
                                            Some(id) => {
                                                match requests.remove(id) {
                                                    Some((_,reply)) => {
                                                        reply.send(response).await.ok();
                                                    },
                                                    None => {
                                                        // Unknown request.
                                                        log::warn!("Warning: Received response for unknown request {}", id);
                                                    }
                                                }
                                            },
                                            None => {
                                                log::error!("Error: {:?}", response);
                                            }
                                        }
                                    },
                                    Err(err) => {
                                        log::error!("Error: {:?}", err);
                                    }
                                }
                                // let response = handle_rpc_delivery(&delivery).await;

                                // self.try_reply_to(&channel, &delivery, &response).await;

                                // channel.basic_ack(
                                //     delivery.delivery_tag,
                                //     BasicAckOptions::default()
                                // ).map(|_| ()).await;
                            },
                            Some(Err(err)) => {
                                log::error!("Error: {:?}", err);
                            },
                            None => {
                                // break;
                            }
                        }
                    }
                )
            }
        });

        Ok(
            Client {
                rpc: tx,
                queue_name,
                ident,
                handle
            }
        )
    }

    pub fn abort(self) {
        self.handle.abort()
    }

    pub fn rpc_request(method: impl ToString, params: Option<impl Serialize>) {

    }
}

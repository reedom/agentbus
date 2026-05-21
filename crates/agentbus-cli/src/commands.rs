use clap::{Parser, Subcommand};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use std::io::Read;

#[derive(Parser)]
#[command(name = "agentbus", about = "agentbus CLI")]
pub struct Cli {
    #[arg(long, env = "AGENTBUS_URL", default_value = "http://127.0.0.1:8765")]
    pub url: String,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    Ls,
    Send {
        to: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    Ask {
        to: String,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(short, long)]
        file: Option<String>,
    },
    Tail {
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        since: Option<String>,
    },
    Reply {
        request_id: String,
        instance: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    Rm {
        id: String,
        #[arg(long)]
        owner: String,
    },
}

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    match cli.cmd {
        Cmd::Ls => {
            let r = client
                .get(format!("{}/v1/instances", cli.url))
                .send()
                .await?;
            let body = read_success_body(r).await?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            println!("{}", serde_json::to_string_pretty(&parsed)?);
        }
        Cmd::Send { to, file } => {
            let payload = read_payload(file)?;
            let r = client
                .post(format!("{}/v1/instances/{}/messages", cli.url, to))
                .json(&serde_json::json!({"payload": payload}))
                .send()
                .await?;
            println!("{}", read_success_body(r).await?);
        }
        Cmd::Ask {
            to,
            timeout_ms,
            file,
        } => {
            let payload = read_payload(file)?;
            let r = client
                .post(format!(
                    "{}/v1/instances/{}/ask?timeout_ms={}",
                    cli.url, to, timeout_ms
                ))
                .json(&serde_json::json!({"payload": payload}))
                .send()
                .await?;
            println!("{}", read_success_body(r).await?);
        }
        Cmd::Tail { instance, since } => {
            let mut url = format!("{}/v1/events", cli.url);
            let mut qs = Vec::new();
            if let Some(i) = instance {
                qs.push(format!("instance={}", urlencoding::encode(&i)));
            }
            if let Some(s) = since {
                qs.push(format!("since={}", urlencoding::encode(&s)));
            }
            if !qs.is_empty() {
                url.push('?');
                url.push_str(&qs.join("&"));
            }
            let resp = client.get(url).send().await?;
            let mut stream = resp.bytes_stream().eventsource();
            while let Some(ev) = stream.next().await {
                let ev = ev?;
                println!("{}", ev.data);
            }
        }
        Cmd::Reply {
            request_id,
            instance,
            file,
        } => {
            let payload = read_payload(file)?;
            let r = client
                .post(format!("{}/v1/instances/{}/replies", cli.url, instance))
                .json(&serde_json::json!({"request_id": request_id, "payload": payload}))
                .send()
                .await?;
            println!("{}", read_success_body(r).await?);
        }
        Cmd::Rm { id, owner } => {
            let r = client
                .delete(format!("{}/v1/instances/{}", cli.url, id))
                .header("x-agentbus-owner", owner)
                .send()
                .await?;
            println!("{}", read_success_body(r).await?);
        }
    }
    Ok(())
}

async fn read_success_body(resp: reqwest::Response) -> anyhow::Result<String> {
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("HTTP {}: {}", status, body);
    }
    Ok(body)
}

fn read_payload(file: Option<String>) -> anyhow::Result<serde_json::Value> {
    let raw = match file.as_deref() {
        Some("-") | None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
        Some(path) => std::fs::read_to_string(path)?,
    };
    Ok(serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw)))
}

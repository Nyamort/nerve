use std::{fmt::Display, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use duration_string::DurationString;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{state::SharedState, Invocation};

#[cfg(feature = "fireworks")]
mod fireworks;
#[cfg(feature = "groq")]
mod groq;
#[cfg(feature = "hf")]
mod huggingface;
#[cfg(feature = "ollama")]
mod ollama;
#[cfg(feature = "openai")]
mod openai;

mod options;

pub use options::*;

lazy_static! {
    static ref RETRY_TIME_PARSER: Regex =
        Regex::new(r"(?m)^.+try again in (.+)\. Visit.*").unwrap();
    static ref CONN_RESET_PARSER: Regex = Regex::new(r"(?m)^.+onnection reset by peer.*").unwrap();
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatOptions {
    pub system_prompt: String,
    pub prompt: String,
    pub history: Vec<Message>,
}

impl ChatOptions {
    pub fn new(system_prompt: String, prompt: String, history: Vec<Message>) -> Self {
        Self {
            system_prompt,
            prompt,
            history,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    Agent(String, Option<Invocation>),
    Feedback(String, Option<Invocation>),
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Message::Agent(data, _) => format!("[agent]\n\n{}\n", data),
                Message::Feedback(data, _) => format!("[feedback]\n\n{}\n", data),
            }
        )
    }
}

#[async_trait]
pub trait Client: mini_rag::Embedder + Send + Sync {
    fn new(url: &str, port: u16, model_name: &str, context_window: u32) -> Result<Self>
    where
        Self: Sized;

    async fn chat(
        &self,
        state: SharedState,
        options: &ChatOptions,
    ) -> Result<(String, Vec<Invocation>)>;

    async fn check_tools_support(&self) -> Result<bool> {
        Ok(false)
    }

    async fn check_rate_limit(&self, error: &str) -> bool {
        // if rate limit exceeded, parse the retry time and retry
        if let Some(caps) = RETRY_TIME_PARSER.captures_iter(error).next() {
            if caps.len() == 2 {
                let mut retry_time_str = "".to_string();

                caps.get(1)
                    .unwrap()
                    .as_str()
                    .clone_into(&mut retry_time_str);

                // DurationString can't handle decimals like Xm3.838383s
                if retry_time_str.contains('.') {
                    let (val, _) = retry_time_str.split_once('.').unwrap();
                    retry_time_str = format!("{}s", val);
                }

                if let Ok(retry_time) = retry_time_str.parse::<DurationString>() {
                    log::warn!(
                        "rate limit reached for this model, retrying in {} ...",
                        retry_time,
                    );

                    tokio::time::sleep(
                        retry_time.checked_add(Duration::from_millis(1000)).unwrap(),
                    )
                    .await;

                    return true;
                } else {
                    log::error!("can't parse '{}'", &retry_time_str);
                }
            } else {
                log::error!("cap len wrong");
            }
        } else if CONN_RESET_PARSER.captures_iter(error).next().is_some() {
            let retry_time = Duration::from_secs(5);
            log::warn!(
                "connection reset by peer, retrying in {:?} ...",
                &retry_time,
            );

            tokio::time::sleep(retry_time).await;

            return true;
        }

        return false;
    }
}

// ugly workaround because rust doesn't support trait upcasting coercion yet

macro_rules! factory_body {
    ($name:expr, $url:expr, $port:expr, $model_name:expr, $context_window:expr) => {
        match $name {
            #[cfg(feature = "ollama")]
            "ollama" => Ok(Box::new(ollama::OllamaClient::new(
                $url,
                $port,
                $model_name,
                $context_window,
            )?)),
            #[cfg(feature = "openai")]
            "openai" => Ok(Box::new(openai::OpenAIClient::new(
                $url,
                $port,
                $model_name,
                $context_window,
            )?)),
            #[cfg(feature = "fireworks")]
            "fireworks" => Ok(Box::new(fireworks::FireworksClient::new(
                $url,
                $port,
                $model_name,
                $context_window,
            )?)),
            "hf" => Ok(Box::new(huggingface::HuggingfaceMessageClient::new(
                $url,
                $port,
                $model_name,
                $context_window,
            )?)),
            #[cfg(feature = "groq")]
            "groq" => Ok(Box::new(groq::GroqClient::new(
                $url,
                $port,
                $model_name,
                $context_window,
            )?)),
            _ => Err(anyhow!("generator '{}' not supported yet", $name)),
        }
    };
}

pub fn factory(
    name: &str,
    url: &str,
    port: u16,
    model_name: &str,
    context_window: u32,
) -> Result<Box<dyn Client>> {
    factory_body!(name, url, port, model_name, context_window)
}

pub fn factory_embedder(
    name: &str,
    url: &str,
    port: u16,
    model_name: &str,
    context_window: u32,
) -> Result<Box<dyn mini_rag::Embedder>> {
    factory_body!(name, url, port, model_name, context_window)
}

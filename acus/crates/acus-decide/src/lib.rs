// SPDX-License-Identifier: MPL-2.0
//! Typed judgements through the Jev "System One" API (`POST …/v1/systemone`), which
//! TypeSafe's Jev, Cloudflare's Clef (Workers AI) and Ollama's Clef serve.

use capsa_core::hint::Hint;
use serde_json::{Map, Value, json};
use std::borrow::Cow;

/// Reports why a judgement failed: the request, the response, or the answer it carried.
#[derive(Debug, thiserror::Error)]
pub enum DecideError {
    /// The endpoint answers with an error status.
    #[error("HTTP {code} from {url}")]
    Status { code: u16, url: String },
    /// The request does not complete.
    #[error("request to {url} failed")]
    Request {
        url: String,
        #[source]
        source: ureq::Error,
    },
    /// The response body is not JSON.
    #[error("response is not JSON")]
    NotJson(#[source] ureq::Error),
    /// The response carries no answer; the text is its error, cut short.
    #[error("unexpected response: {0}")]
    Unexpected(String),
    /// The answer lacks the field the question needs.
    #[error("answer without `{field}`: {answer}")]
    MissingField { field: &'static str, answer: Value },
}

impl Hint for DecideError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Status { .. } => Some(Cow::Borrowed(
                "check the API key variable and the URL in the acus config",
            )),
            _ => None,
        }
    }
}

pub enum Question {
    /// Yes or no, answered as the probability of yes (Jev `noul`).
    YesNo,
    /// One of the named options, given as name and description (Jev `choice`).
    Choice(Vec<(String, String)>),
    /// One of the ordered levels, lowest first (Jev `score`).
    Score(Vec<String>),
}

#[derive(Debug)]
pub struct Answer {
    pub value: String,
    /// The probability of `value`.
    pub confidence: f64,
    /// Set for yes/no questions.
    pub yes: Option<bool>,
}

pub struct Endpoint {
    pub url: String,
    pub model: String,
    pub api_key: Option<String>,
}

/// Builds the request body; the single question is named `q`.
pub fn body(
    model: &str,
    state: &str,
    instructions: &str,
    q: &Question,
    images: &[String],
) -> Value {
    let mut question = Map::new();
    let (kind, criteria) = match q {
        Question::YesNo => ("noul", None),
        Question::Choice(opts) => (
            "choice",
            Some(json!(
                opts.iter()
                    .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                    .collect::<Map<_, _>>()
            )),
        ),
        Question::Score(levels) => ("score", Some(json!(levels))),
    };
    question.insert("type".into(), kind.into());
    question.insert("instructions".into(), instructions.into());
    if let Some(c) = criteria {
        question.insert("criteria".into(), c);
    }
    let mut b = json!({ "model": model, "state": state, "questions": { "q": question } });
    if !images.is_empty() {
        // ponytail: Clef-only extension; base64 encoding assumed (Cloudflare docs do not say).
        b["images"] = json!(images);
    }
    b
}

pub fn parse_answer(resp: &Value, q: &Question) -> Result<Answer, DecideError> {
    // Workers AI wraps the model output in `{ "result": …, "success": … }`.
    let resp = resp.get("result").filter(|r| r.is_object()).unwrap_or(resp);
    let Some(a) = resp.pointer("/answers/q") else {
        let msg = resp
            .get("error")
            .or_else(|| resp.get("errors"))
            .map_or_else(|| resp.to_string(), Value::to_string);
        return Err(DecideError::Unexpected(msg.chars().take(300).collect()));
    };
    let num = |k: &str| a.get(k).and_then(Value::as_f64);
    Ok(match q {
        Question::YesNo => {
            let p = num("noul").ok_or_else(|| DecideError::MissingField {
                field: "noul",
                answer: a.clone(),
            })?;
            let yes = p >= 0.5;
            Answer {
                value: if yes { "yes" } else { "no" }.into(),
                confidence: if yes { p } else { 1.0 - p },
                yes: Some(yes),
            }
        }
        Question::Choice(_) => Answer {
            value: a
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| DecideError::MissingField {
                    field: "choice",
                    answer: a.clone(),
                })?
                .into(),
            confidence: num("confidence").unwrap_or(f64::NAN),
            yes: None,
        },
        Question::Score(levels) => {
            let s = a.get("score").ok_or_else(|| DecideError::MissingField {
                field: "score",
                answer: a.clone(),
            })?;
            // Jev returns the expected level index as a float (`1.99`); a label is accepted too.
            let value = match s.as_f64() {
                Some(f) => levels
                    .get(f.round() as usize)
                    .cloned()
                    .unwrap_or_else(|| f.to_string()),
                None => s.as_str().map_or_else(|| s.to_string(), str::to_owned),
            };
            Answer {
                value,
                confidence: num("confidence").unwrap_or(f64::NAN),
                yes: None,
            }
        }
    })
}

pub fn decide(
    ep: &Endpoint,
    state: &str,
    instructions: &str,
    q: &Question,
    images: &[String],
) -> Result<Answer, DecideError> {
    let mut req = ureq::post(&ep.url);
    if let Some(k) = &ep.api_key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let resp: Value = match req.send_json(body(&ep.model, state, instructions, q, images)) {
        Ok(mut r) => r.body_mut().read_json().map_err(DecideError::NotJson)?,
        Err(ureq::Error::StatusCode(code)) => {
            return Err(DecideError::Status {
                code,
                url: ep.url.clone(),
            });
        }
        Err(source) => {
            return Err(DecideError::Request {
                url: ep.url.clone(),
                source,
            });
        }
    };
    parse_answer(&resp, q)
}

/// Standard base64 with padding (RFC 4648), for image payloads.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() {
                T[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

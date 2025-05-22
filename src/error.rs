use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, OllanaError>;

#[derive(Error, Debug)]
pub enum OllanaError {
    #[error("HTTP error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    IO(#[from] io::Error),
    #[error("Url parse error")]
    UrlParse(#[from] url::ParseError),
    #[error("{0}")]
    Other(String),
}

use std::fmt::{Display, Formatter};
use std::io;

use serde::de;

#[derive(Debug)]
pub struct Error {
    err: Box<dyn std::error::Error>
}

pub type Result<T> = std::result::Result<T, Error>;

impl de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error {
            err: io::Error::other(msg),
        }
    }
}

impl de::StdError for Error {

}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "serde_le error")
    }
}
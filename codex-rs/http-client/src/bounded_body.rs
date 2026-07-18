use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use std::fmt::Display;
use std::io;

/// Collects a byte stream up to a hard limit and stops polling as soon as it is exceeded.
pub async fn collect_bytes_bounded<S, E>(stream: S, max_bytes: usize) -> io::Result<Vec<u8>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Display,
{
    futures::pin_mut!(stream);
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| io::Error::other(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("response body exceeds the {max_bytes}-byte limit"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "bounded_body_tests.rs"]
mod tests;

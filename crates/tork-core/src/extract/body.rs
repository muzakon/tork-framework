//! Request-body extractors.

use bytes::{BufMut, Bytes, BytesMut};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;

use crate::body::ReqBody;
use crate::constants::MAX_BODY_BYTES;
use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};
use crate::response::Json;

/// Deserializes the request body as JSON.
///
/// The body is buffered with a size cap of [`MAX_BODY_BYTES`] to guard against
/// memory-exhaustion attacks, then parsed into `T`.
///
/// # Errors
///
/// - `400 Bad Request` if the body was already consumed, exceeds the size cap,
///   or could not be read.
/// - `422 Unprocessable Entity` if the body is not valid JSON for `T`.
impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send,
{
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let taken = ctx.take_body();
        async move {
            let body = taken?;
            let bytes = read_body_capped(body).await?;
            let value = serde_json::from_slice::<T>(&bytes)
                .map_err(|_| Error::unprocessable("request body is not valid JSON"))?;
            Ok(Json(value))
        }
    }
}

/// Buffers a request body, rejecting payloads larger than [`MAX_BODY_BYTES`].
///
/// The cap is enforced incrementally as frames arrive, so an oversized payload
/// is rejected without buffering all of it first.
pub(crate) async fn read_body_capped(mut body: ReqBody) -> Result<Bytes> {
    let mut buffer = BytesMut::new();

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(map_body_error)?;

        if let Ok(data) = frame.into_data() {
            if buffer.len() + data.len() > MAX_BODY_BYTES {
                return Err(Error::bad_request("request body is too large"));
            }
            buffer.put(data);
        }
    }

    Ok(buffer.freeze())
}

fn map_body_error(error: crate::body::BoxError) -> Error {
    match error.downcast::<Error>() {
        Ok(error) => *error,
        Err(_) => Error::bad_request("request body could not be read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::extract::PathParams;
    use crate::state::StateMap;
    use http_body_util::Full;
    use serde::Deserialize;
    use std::sync::Arc;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Payload {
        name: String,
    }

    fn context(body: Bytes) -> RequestContext {
        let head = http::Request::new(()).into_parts().0;
        RequestContext::new(
            head,
            PathParams::new(),
            Arc::new(StateMap::new()),
            box_body(Full::new(body)),
        )
    }

    #[tokio::test]
    async fn reads_body_within_limit() {
        let body = box_body(Full::new(Bytes::from_static(b"hello")));

        let bytes = read_body_capped(body).await.unwrap();
        assert_eq!(bytes, Bytes::from_static(b"hello"));
    }

    #[tokio::test]
    async fn rejects_body_over_limit() {
        let oversized = vec![b'x'; MAX_BODY_BYTES + 1];
        let body = box_body(Full::new(Bytes::from(oversized)));

        let error = read_body_capped(body).await.unwrap_err();
        assert_eq!(error.kind(), crate::error::ErrorKind::BadRequest);
        assert_eq!(error.message(), "request body is too large");
    }

    #[tokio::test]
    async fn preserves_payload_too_large_errors_from_the_body() {
        let body = crate::body::box_body(http_body_util::StreamBody::new(futures_util::stream::iter(vec![
            Ok::<_, crate::body::BoxError>(http_body::Frame::data(Bytes::from_static(b"hello"))),
            Err::<http_body::Frame<Bytes>, _>(Box::new(Error::payload_too_large("request body too large")) as crate::body::BoxError),
        ])));

        let error = read_body_capped(body).await.unwrap_err();
        assert_eq!(error.kind(), crate::error::ErrorKind::PayloadTooLarge);
        assert_eq!(error.message(), "request body too large");
    }

    #[tokio::test]
    async fn json_extractor_accepts_valid_json() {
        let ctx = context(Bytes::from_static(br#"{"name":"tork"}"#));

        let Json(payload) = <Json<Payload> as FromRequest>::from_request(&ctx)
            .await
            .unwrap();
        assert_eq!(
            payload,
            Payload {
                name: "tork".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn json_extractor_rejects_invalid_json_shape() {
        let ctx = context(Bytes::from_static(br#"{"name":1}"#));

        let error = match <Json<Payload> as FromRequest>::from_request(&ctx).await {
            Ok(_) => panic!("expected invalid JSON shape to fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), crate::error::ErrorKind::Unprocessable);
        assert_eq!(error.message(), "request body is not valid JSON");
    }

    #[tokio::test]
    async fn json_extractor_rejects_consumed_body() {
        let ctx = context(Bytes::from_static(br#"{"name":"tork"}"#));
        let _ = ctx.take_body().unwrap();

        let error = match <Json<Payload> as FromRequest>::from_request(&ctx).await {
            Ok(_) => panic!("expected consumed body to fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), crate::error::ErrorKind::BadRequest);
        assert_eq!(error.message(), "request body has already been consumed");
    }
}

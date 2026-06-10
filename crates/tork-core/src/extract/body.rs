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
async fn read_body_capped(mut body: ReqBody) -> Result<Bytes> {
    let mut buffer = BytesMut::new();

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| Error::bad_request("request body could not be read"))?;

        if let Ok(data) = frame.into_data() {
            if buffer.len() + data.len() > MAX_BODY_BYTES {
                return Err(Error::bad_request("request body is too large"));
            }
            buffer.put(data);
        }
    }

    Ok(buffer.freeze())
}

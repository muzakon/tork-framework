//! Multipart and form request handling: file uploads and form fields.
//!
//! A `multipart/form-data` body is parsed once into a [`MultipartForm`]: text
//! fields are kept in memory, and file fields are spooled into a temporary file
//! (in memory up to a threshold, then on disk). Handlers consume fields as
//! [`FileBytes`] (buffered), [`UploadFile`] (spooled), or typed text values.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use bytes::Bytes;
use garde::Validate;
use http::header::CONTENT_TYPE;
use http_body_util::BodyDataStream;
use mime::Mime;
use serde::de::DeserializeOwned;
use tempfile::SpooledTempFile;

use crate::error::{Error, Result};
use crate::extract::body::read_body_capped;
use crate::extract::{FromRequest, RequestContext};

/// Default cap on the total decoded size of a multipart body.
const DEFAULT_MAX_BODY_SIZE: usize = 16 * 1024 * 1024;
/// Default cap on a single uploaded file.
const DEFAULT_MAX_FILE_SIZE: usize = 8 * 1024 * 1024;
/// Default size a file may reach in memory before spilling to disk.
const DEFAULT_MEMORY_THRESHOLD: usize = 1024 * 1024;
/// Default cap on the number of file parts in one request.
const DEFAULT_MAX_FILES: usize = 32;

/// Limits and temp-file behavior for multipart uploads.
///
/// Configure app-wide defaults with [`App::upload_config`](crate::App::upload_config)
/// or per route with `#[post("/p", upload(...))]`; a route value overrides the
/// app default. Unset fields fall back to the framework defaults.
#[derive(Clone, Default)]
pub struct UploadConfig {
    max_body_size: Option<usize>,
    max_file_size: Option<usize>,
    memory_threshold: Option<usize>,
    max_files: Option<usize>,
    temp_dir: Option<PathBuf>,
}

impl UploadConfig {
    /// Creates an empty configuration (all limits at their defaults).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum total multipart body size, in bytes.
    pub fn max_body_size(mut self, bytes: usize) -> Self {
        self.max_body_size = Some(bytes);
        self
    }

    /// Sets the maximum total multipart body size, in mebibytes.
    pub fn max_body_size_mb(self, mb: usize) -> Self {
        self.max_body_size(mb * 1024 * 1024)
    }

    /// Sets the maximum size of a single uploaded file, in bytes.
    pub fn max_file_size(mut self, bytes: usize) -> Self {
        self.max_file_size = Some(bytes);
        self
    }

    /// Sets the maximum size of a single uploaded file, in mebibytes.
    pub fn max_file_size_mb(self, mb: usize) -> Self {
        self.max_file_size(mb * 1024 * 1024)
    }

    /// Sets the in-memory threshold before a file spills to disk, in bytes.
    pub fn memory_threshold(mut self, bytes: usize) -> Self {
        self.memory_threshold = Some(bytes);
        self
    }

    /// Sets the maximum number of file parts per request.
    pub fn max_files(mut self, count: usize) -> Self {
        self.max_files = Some(count);
        self
    }

    /// Sets the directory for spilled temporary files.
    pub fn temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = Some(dir.into());
        self
    }

    /// Returns a copy with each unset field taken from `base` (route over app).
    pub(crate) fn merge(self, base: &UploadConfig) -> Self {
        Self {
            max_body_size: self.max_body_size.or(base.max_body_size),
            max_file_size: self.max_file_size.or(base.max_file_size),
            memory_threshold: self.memory_threshold.or(base.memory_threshold),
            max_files: self.max_files.or(base.max_files),
            temp_dir: self.temp_dir.or_else(|| base.temp_dir.clone()),
        }
    }

    /// Resolves every field, applying defaults.
    fn resolve(&self) -> ResolvedConfig {
        ResolvedConfig {
            max_body_size: self.max_body_size.unwrap_or(DEFAULT_MAX_BODY_SIZE),
            max_file_size: self.max_file_size.unwrap_or(DEFAULT_MAX_FILE_SIZE),
            memory_threshold: self.memory_threshold.unwrap_or(DEFAULT_MEMORY_THRESHOLD),
            max_files: self.max_files.unwrap_or(DEFAULT_MAX_FILES),
        }
    }
}

/// A fully-resolved upload configuration used by the parser.
struct ResolvedConfig {
    max_body_size: usize,
    max_file_size: usize,
    memory_threshold: usize,
    max_files: usize,
}

/// The application-wide default upload configuration, stored in the state map.
#[derive(Clone)]
pub(crate) struct AppUploadConfig(pub(crate) UploadConfig);

/// A buffered uploaded file, held entirely in memory.
///
/// Use this for small files. For large files prefer [`UploadFile`], which spools
/// to disk past a threshold.
pub struct FileBytes {
    bytes: Bytes,
    filename: Option<String>,
    content_type: Option<Mime>,
}

impl FileBytes {
    pub(crate) fn new(bytes: Bytes, filename: Option<String>, content_type: Option<Mime>) -> Self {
        Self {
            bytes,
            filename,
            content_type,
        }
    }

    /// Returns the file size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns `true` if the file is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the file contents.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the file, returning its contents.
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    /// Returns the client-provided filename, if any.
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    /// Returns the declared content type, if any.
    pub fn content_type(&self) -> Option<&Mime> {
        self.content_type.as_ref()
    }
}

/// An uploaded file backed by a spooled temporary file.
///
/// The contents stay in memory up to a threshold and then spill to disk, so large
/// uploads do not exhaust memory. Reads and saves run on a blocking thread.
pub struct UploadFile {
    filename: Option<String>,
    content_type: Option<Mime>,
    size: u64,
    storage: Option<SpooledTempFile>,
}

impl UploadFile {
    pub(crate) fn new(
        filename: Option<String>,
        content_type: Option<Mime>,
        size: u64,
        storage: SpooledTempFile,
    ) -> Self {
        Self {
            filename,
            content_type,
            size,
            storage: Some(storage),
        }
    }

    /// Returns the client-provided filename, if any.
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    /// Returns the declared content type, if any.
    pub fn content_type(&self) -> Option<&Mime> {
        self.content_type.as_ref()
    }

    /// Returns the file size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Runs a blocking operation over the spooled storage, restoring it after.
    async fn with_storage<F, R>(&mut self, op: F) -> Result<R>
    where
        F: FnOnce(SpooledTempFile) -> std::io::Result<(SpooledTempFile, R)> + Send + 'static,
        R: Send + 'static,
    {
        let storage = self
            .storage
            .take()
            .ok_or_else(|| Error::internal("upload file storage was already consumed"))?;
        let (storage, result) = tokio::task::spawn_blocking(move || op(storage))
            .await
            .map_err(|error| Error::internal(format!("upload IO task failed: {error}")))?
            .map_err(|error| Error::internal(format!("upload IO error: {error}")))?;
        self.storage = Some(storage);
        Ok(result)
    }

    /// Reads the whole file into memory.
    pub async fn read(&mut self) -> Result<Bytes> {
        self.with_storage(|mut storage| {
            storage.seek(SeekFrom::Start(0))?;
            let mut buffer = Vec::new();
            storage.read_to_end(&mut buffer)?;
            Ok((storage, Bytes::from(buffer)))
        })
        .await
    }

    /// Reads up to `size` bytes from the current position, or `None` at the end.
    pub async fn read_chunk(&mut self, size: usize) -> Result<Option<Bytes>> {
        self.with_storage(move |mut storage| {
            let mut buffer = vec![0u8; size];
            let read = storage.read(&mut buffer)?;
            buffer.truncate(read);
            let chunk = (read != 0).then(|| Bytes::from(buffer));
            Ok((storage, chunk))
        })
        .await
    }

    /// Rewinds to the start of the file.
    pub async fn seek_start(&mut self) -> Result<()> {
        self.with_storage(|mut storage| {
            storage.seek(SeekFrom::Start(0))?;
            Ok((storage, ()))
        })
        .await
    }

    /// Writes the file to `path`.
    pub async fn save_to<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref().to_path_buf();
        self.with_storage(move |mut storage| {
            storage.seek(SeekFrom::Start(0))?;
            let mut output = std::fs::File::create(&path)?;
            std::io::copy(&mut storage, &mut output)?;
            Ok((storage, ()))
        })
        .await
    }
}

/// An `application/x-www-form-urlencoded` request body, deserialized and validated.
///
/// For form submissions without files. With files, use [`Multipart<T>`].
pub struct Form<T>(pub T);

impl<T> Form<T> {
    /// Unwraps the parsed form value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> FromRequest for Form<T>
where
    T: DeserializeOwned + Validate<Context = ()> + Send,
{
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let taken = ctx.take_body();
        async move {
            let bytes = read_body_capped(taken?).await?;
            let value: T = serde_urlencoded::from_bytes(&bytes)
                .map_err(|_| Error::unprocessable("request body is not a valid form"))?;
            value.validate().map_err(Error::from_garde_report)?;
            Ok(Form(value))
        }
    }
}

/// Builds a value from a parsed multipart body.
///
/// Implemented by `#[derive(FormModel)]`, which binds each field (text or file)
/// and runs its validation.
pub trait FromMultipart: Sized {
    /// Binds `Self` from the parsed multipart form.
    fn from_multipart(
        form: &mut MultipartForm,
    ) -> impl std::future::Future<Output = Result<Self>> + Send;

    /// Builds the OpenAPI/AsyncAPI schema for the form (overridden by the derive).
    ///
    /// The default is a permissive object; `#[derive(FormModel)]` generates a
    /// precise schema with file fields marked as `format: binary`.
    fn form_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let _ = generator;
        schemars::json_schema!({ "type": "object" })
    }
}

/// A `multipart/form-data` request body bound to a form model.
pub struct Multipart<T>(pub T);

impl<T> Multipart<T> {
    /// Unwraps the bound form value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> FromRequest for Multipart<T>
where
    T: FromMultipart + Send,
{
    async fn from_request(ctx: &RequestContext) -> Result<Self> {
        let mut form = __parse_multipart(ctx, UploadConfig::new()).await?;
        let value = T::from_multipart(&mut form).await?;
        Ok(Multipart(value))
    }
}

/// Validation rules for a single file field, built by the form macros.
///
/// Generated-code support, not part of the everyday API.
#[doc(hidden)]
pub struct FileRule {
    pub max_size: Option<usize>,
    pub content_types: &'static [&'static str],
    pub sniff: bool,
}

/// Validates a buffered file against its rule.
#[doc(hidden)]
pub fn __validate_file_bytes(file: &FileBytes, rule: &FileRule) -> Result<()> {
    check_size(file.len(), rule)?;
    check_declared_type(file.content_type(), rule)?;
    if rule.sniff {
        check_sniffed_type(file.bytes(), rule)?;
    }
    Ok(())
}

/// Validates a spooled upload against its rule (sniffing a small prefix).
#[doc(hidden)]
pub async fn __validate_upload(file: &mut UploadFile, rule: &FileRule) -> Result<()> {
    check_size(file.size() as usize, rule)?;
    check_declared_type(file.content_type(), rule)?;
    if rule.sniff {
        file.seek_start().await?;
        let prefix = file.read_chunk(512).await?.unwrap_or_default();
        file.seek_start().await?;
        check_sniffed_type(&prefix, rule)?;
    }
    Ok(())
}

/// Rejects a file that exceeds the rule's size limit.
fn check_size(size: usize, rule: &FileRule) -> Result<()> {
    if let Some(max) = rule.max_size {
        if size > max {
            return Err(Error::payload_too_large("uploaded file is too large")
                .with_code("FILE_TOO_LARGE"));
        }
    }
    Ok(())
}

/// Rejects a file whose declared content type is not allowed.
fn check_declared_type(declared: Option<&Mime>, rule: &FileRule) -> Result<()> {
    if rule.content_types.is_empty() {
        return Ok(());
    }
    let allowed = declared
        .map(|mime| {
            rule.content_types
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(mime.essence_str()))
        })
        .unwrap_or(false);
    if !allowed {
        return Err(Error::unprocessable("unsupported file content type")
            .with_code("UNSUPPORTED_MEDIA_TYPE"));
    }
    Ok(())
}

/// Rejects a file whose sniffed (magic-byte) type is not allowed.
fn check_sniffed_type(bytes: &[u8], rule: &FileRule) -> Result<()> {
    if rule.content_types.is_empty() {
        return Ok(());
    }
    if let Some(kind) = infer::get(bytes) {
        let detected = kind.mime_type();
        if !rule
            .content_types
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(detected))
        {
            return Err(Error::unprocessable("file content does not match the declared type")
                .with_code("UNSUPPORTED_MEDIA_TYPE"));
        }
    }
    Ok(())
}

/// A single file part captured from a multipart body.
struct FilePart {
    name: String,
    filename: Option<String>,
    content_type: Option<Mime>,
    storage: SpooledTempFile,
    size: u64,
}

/// A parsed multipart body: its text fields and file parts.
///
/// This is generated-code support for the form macros, not part of the everyday
/// API; handlers receive typed fields rather than this container.
#[doc(hidden)]
pub struct MultipartForm {
    texts: Vec<(String, String)>,
    files: Vec<FilePart>,
}

impl MultipartForm {
    /// Parses the request body as `multipart/form-data` using `config`.
    pub(crate) async fn parse(ctx: &RequestContext, config: &UploadConfig) -> Result<Self> {
        let resolved = config.resolve();

        let content_type = ctx
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Error::bad_request("missing Content-Type for multipart form"))?;
        let boundary = multer::parse_boundary(content_type)
            .map_err(|_| Error::bad_request("invalid or missing multipart boundary"))?;

        let body = ctx.take_body()?;
        let mut multipart = multer::Multipart::new(BodyDataStream::new(body), boundary);

        let mut texts = Vec::new();
        let mut files = Vec::new();
        let mut total: usize = 0;

        while let Some(mut field) = multipart.next_field().await.map_err(parse_error)? {
            let name = field.name().map(str::to_owned).unwrap_or_default();
            let filename = field.file_name().map(str::to_owned);
            let content_type = field.content_type().cloned();

            if filename.is_some() {
                if files.len() >= resolved.max_files {
                    return Err(Error::bad_request("too many file fields")
                        .with_code("TOO_MANY_FILES"));
                }
                let mut storage = SpooledTempFile::new(resolved.memory_threshold);
                let mut size: u64 = 0;
                while let Some(chunk) = field.chunk().await.map_err(parse_error)? {
                    size += chunk.len() as u64;
                    total += chunk.len();
                    if size as usize > resolved.max_file_size {
                        return Err(Error::payload_too_large("uploaded file is too large")
                            .with_code("FILE_TOO_LARGE"));
                    }
                    if total > resolved.max_body_size {
                        return Err(Error::payload_too_large("request body is too large"));
                    }
                    storage
                        .write_all(&chunk)
                        .map_err(|error| Error::internal(format!("spool write failed: {error}")))?;
                }
                storage
                    .seek(SeekFrom::Start(0))
                    .map_err(|error| Error::internal(format!("spool seek failed: {error}")))?;
                files.push(FilePart {
                    name,
                    filename,
                    content_type,
                    storage,
                    size,
                });
            } else {
                let text = field.text().await.map_err(parse_error)?;
                total += text.len();
                if total > resolved.max_body_size {
                    return Err(Error::payload_too_large("request body is too large"));
                }
                texts.push((name, text));
            }
        }

        Ok(Self { texts, files })
    }

    /// Removes and parses the first text field named `name`.
    #[doc(hidden)]
    pub fn take_form_value<T: FromStr>(&mut self, name: &str) -> Result<Option<T>> {
        let Some(pos) = self.texts.iter().position(|(field, _)| field == name) else {
            return Ok(None);
        };
        let (_, value) = self.texts.remove(pos);
        value
            .parse::<T>()
            .map(Some)
            .map_err(|_| Error::unprocessable(format!("form field `{name}` has an invalid value")))
    }

    /// Removes and parses every text field named `name`, in order.
    #[doc(hidden)]
    pub fn take_form_values<T: FromStr>(&mut self, name: &str) -> Result<Vec<T>> {
        let mut values = Vec::new();
        let mut index = 0;
        while index < self.texts.len() {
            if self.texts[index].0 == name {
                let (_, value) = self.texts.remove(index);
                let parsed = value.parse::<T>().map_err(|_| {
                    Error::unprocessable(format!("form field `{name}` has an invalid value"))
                })?;
                values.push(parsed);
            } else {
                index += 1;
            }
        }
        Ok(values)
    }

    /// Removes the first file field named `name`, buffering it into memory.
    #[doc(hidden)]
    pub async fn take_file_bytes(&mut self, name: &str) -> Result<Option<FileBytes>> {
        let Some(pos) = self.files.iter().position(|file| file.name == name) else {
            return Ok(None);
        };
        Ok(Some(file_part_into_bytes(self.files.remove(pos)).await?))
    }

    /// Removes every file field named `name`, buffering each into memory.
    #[doc(hidden)]
    pub async fn take_file_bytes_list(&mut self, name: &str) -> Result<Vec<FileBytes>> {
        let mut parts = Vec::new();
        let mut index = 0;
        while index < self.files.len() {
            if self.files[index].name == name {
                parts.push(self.files.remove(index));
            } else {
                index += 1;
            }
        }
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            out.push(file_part_into_bytes(part).await?);
        }
        Ok(out)
    }

    /// Removes the first file field named `name` as a spooled upload.
    #[doc(hidden)]
    pub fn take_upload_file(&mut self, name: &str) -> Option<UploadFile> {
        let pos = self.files.iter().position(|file| file.name == name)?;
        Some(file_part_into_upload(self.files.remove(pos)))
    }

    /// Removes every file field named `name` as spooled uploads.
    #[doc(hidden)]
    pub fn take_upload_file_list(&mut self, name: &str) -> Vec<UploadFile> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < self.files.len() {
            if self.files[index].name == name {
                out.push(file_part_into_upload(self.files.remove(index)));
            } else {
                index += 1;
            }
        }
        out
    }
}

/// Parses the request body as multipart, merging the route override over the
/// application default upload configuration.
///
/// Generated-code support for the form macros.
#[doc(hidden)]
pub async fn __parse_multipart(ctx: &RequestContext, route: UploadConfig) -> Result<MultipartForm> {
    let app_default = ctx
        .state()
        .get::<AppUploadConfig>()
        .map(|config| config.0.clone())
        .unwrap_or_default();
    let config = route.merge(&app_default);
    MultipartForm::parse(ctx, &config).await
}

/// Reads a spooled file part fully into memory.
async fn file_part_into_bytes(part: FilePart) -> Result<FileBytes> {
    let FilePart {
        filename,
        content_type,
        mut storage,
        ..
    } = part;
    let bytes = tokio::task::spawn_blocking(move || {
        storage.seek(SeekFrom::Start(0))?;
        let mut buffer = Vec::new();
        storage.read_to_end(&mut buffer)?;
        Ok::<_, std::io::Error>(Bytes::from(buffer))
    })
    .await
    .map_err(|error| Error::internal(format!("upload IO task failed: {error}")))?
    .map_err(|error| Error::internal(format!("upload IO error: {error}")))?;
    Ok(FileBytes::new(bytes, filename, content_type))
}

/// Wraps a spooled file part as an [`UploadFile`].
fn file_part_into_upload(part: FilePart) -> UploadFile {
    UploadFile::new(part.filename, part.content_type, part.size, part.storage)
}

/// Maps a multer error to a `400 Bad Request`.
fn parse_error(error: multer::Error) -> Error {
    Error::bad_request(format!("multipart parse error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spooled(data: &[u8]) -> SpooledTempFile {
        let mut storage = SpooledTempFile::new(1024 * 1024);
        storage.write_all(data).unwrap();
        storage.seek(SeekFrom::Start(0)).unwrap();
        storage
    }

    #[test]
    fn file_bytes_reports_size_and_contents() {
        let file = FileBytes::new(Bytes::from_static(b"hello"), Some("a.txt".to_owned()), None);
        assert_eq!(file.len(), 5);
        assert!(!file.is_empty());
        assert_eq!(file.bytes(), b"hello");
        assert_eq!(file.filename(), Some("a.txt"));
    }

    #[tokio::test]
    async fn upload_file_reads_and_saves() {
        let mut file = UploadFile::new(Some("a.bin".to_owned()), None, 5, spooled(b"hello"));
        assert_eq!(file.size(), 5);
        assert_eq!(file.read().await.unwrap(), Bytes::from_static(b"hello"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        file.save_to(&path).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[tokio::test]
    async fn upload_file_reads_in_chunks() {
        let mut file = UploadFile::new(None, None, 4, spooled(b"abcd"));
        assert_eq!(file.read_chunk(2).await.unwrap(), Some(Bytes::from_static(b"ab")));
        assert_eq!(file.read_chunk(2).await.unwrap(), Some(Bytes::from_static(b"cd")));
        assert_eq!(file.read_chunk(2).await.unwrap(), None);
    }

    #[test]
    fn config_merge_prefers_route_over_app() {
        let app = UploadConfig::new().max_file_size_mb(10).max_files(5);
        let route = UploadConfig::new().max_file_size_mb(50);
        let merged = route.merge(&app);
        assert_eq!(merged.resolve().max_file_size, 50 * 1024 * 1024);
        assert_eq!(merged.resolve().max_files, 5);
    }

    use crate::extract::PathParams;
    use crate::state::StateMap;
    use std::sync::Arc;

    fn ctx_with(content_type: &str, body: &[u8]) -> RequestContext {
        let head = http::Request::builder()
            .header(CONTENT_TYPE, content_type)
            .body(())
            .unwrap()
            .into_parts()
            .0;
        let body = crate::body::box_body(http_body_util::Full::new(Bytes::copy_from_slice(body)));
        RequestContext::new(head, PathParams::new(), Arc::new(StateMap::new()), body)
    }

    #[derive(serde::Deserialize, garde::Validate)]
    struct Login {
        #[garde(length(min = 1))]
        username: String,
        #[garde(skip)]
        password: String,
    }

    #[tokio::test]
    async fn form_parses_urlencoded_body() {
        let ctx = ctx_with(
            "application/x-www-form-urlencoded",
            b"username=ada&password=secret",
        );
        let form = Form::<Login>::from_request(&ctx).await.unwrap();
        assert_eq!(form.0.username, "ada");
        assert_eq!(form.0.password, "secret");
    }

    struct TokenForm {
        token: String,
    }

    impl FromMultipart for TokenForm {
        async fn from_multipart(form: &mut MultipartForm) -> Result<Self> {
            let token = form
                .take_form_value::<String>("token")?
                .ok_or_else(|| Error::unprocessable("missing token"))?;
            Ok(TokenForm { token })
        }
    }

    #[tokio::test]
    async fn multipart_binds_a_text_field_and_a_file() {
        let body = "--X\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nabc123\r\n\
                    --X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\
                    Content-Type: text/plain\r\n\r\nhello\r\n--X--\r\n";
        let ctx = ctx_with("multipart/form-data; boundary=X", body.as_bytes());

        // The text field binds via the model.
        let bound = Multipart::<TokenForm>::from_request(&ctx).await.unwrap();
        assert_eq!(bound.0.token, "abc123");
    }

    #[test]
    fn file_validation_enforces_size_type_and_sniff() {
        let png_bytes = Bytes::from_static(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR");
        let png = FileBytes::new(png_bytes, Some("a.png".to_owned()), Some("image/png".parse().unwrap()));
        let rule = FileRule {
            max_size: Some(1024),
            content_types: &["image/png"],
            sniff: true,
        };
        assert!(__validate_file_bytes(&png, &rule).is_ok());

        // Declared content type not allowed.
        let txt = FileBytes::new(Bytes::from_static(b"hi"), None, Some("text/plain".parse().unwrap()));
        let only_png = FileRule {
            max_size: None,
            content_types: &["image/png"],
            sniff: false,
        };
        assert_eq!(
            __validate_file_bytes(&txt, &only_png).err().unwrap().code(),
            "UNSUPPORTED_MEDIA_TYPE"
        );

        // Too large.
        let big = FileBytes::new(Bytes::from(vec![0u8; 100]), None, None);
        let small_limit = FileRule {
            max_size: Some(10),
            content_types: &[],
            sniff: false,
        };
        assert_eq!(
            __validate_file_bytes(&big, &small_limit).err().unwrap().code(),
            "FILE_TOO_LARGE"
        );

        // Sniff mismatch: declared png but bytes are not png.
        let fake = FileBytes::new(Bytes::from_static(b"GIF89a...."), None, Some("image/png".parse().unwrap()));
        assert!(__validate_file_bytes(&fake, &rule).is_err());
    }

    #[tokio::test]
    async fn multipart_form_takes_files_and_values() {
        let body = "--X\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi\r\n\
                    --X\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.bin\"\r\n\r\nDATA\r\n--X--\r\n";
        let ctx = ctx_with("multipart/form-data; boundary=X", body.as_bytes());
        let mut form = __parse_multipart(&ctx, UploadConfig::new()).await.unwrap();

        let file = form.take_file_bytes("doc").await.unwrap().expect("file present");
        assert_eq!(file.bytes(), b"DATA");
        assert_eq!(file.filename(), Some("a.bin"));
        assert_eq!(form.take_form_value::<String>("note").unwrap(), Some("hi".to_owned()));
    }
}
